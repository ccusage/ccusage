use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage_at, cli::CostMode, fast::byte_lines,
    format_date_tz, missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

use super::paths::is_compressed;

const UNKNOWN_MODEL: &str = "unknown";

/// First line of a session log.
#[derive(Debug, Deserialize)]
struct HeaderLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(rename = "createdAt", default, deserialize_with = "jsonl::lenient_i64")]
    created_at: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
    #[serde(rename = "isSeeded", default)]
    is_seeded: bool,
}

/// Every line after the header. Only the fields ccusage consumes are declared.
#[derive(Debug, Deserialize)]
struct EventLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    seq: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    time: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    data: Option<EventData>,
}

/// The union of the `data` fields read from the event types below. `provider`
/// and `model` sit directly on `request/context` and `compaction/summary`.
#[derive(Debug, Default, Deserialize)]
struct EventData {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    header: Option<RequestHeader>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<TokenUsage>,
    #[serde(default, deserialize_with = "jsonl::lenient_vec")]
    stream: Vec<StreamRecord>,
    #[serde(default)]
    inherited: bool,
}

impl EventData {
    /// The usage dsh's own token meter reads for an assistant settlement: the
    /// settled `usage`, else the last usage chunk of the stream.
    fn settled_usage(self) -> Option<TokenUsage> {
        self.usage.or_else(|| {
            self.stream
                .into_iter()
                .rev()
                .filter(|record| record.r#type.as_deref() == Some("chunk"))
                .filter_map(|record| record.chunk)
                .find(|chunk| chunk.r#type.as_deref() == Some("usage"))
                .and_then(|chunk| chunk.usage)
        })
    }
}

#[derive(Debug, Deserialize)]
struct StreamRecord {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    chunk: Option<StreamChunk>,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<TokenUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct RequestHeader {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    config: Option<Route>,
}

#[derive(Debug, Default, Deserialize)]
struct Route {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

/// dsh's `TokenUsage`: the counts are disjoint, so `inputTokens` excludes cache reads and writes.
#[derive(Debug, Default, Deserialize)]
struct TokenUsage {
    #[serde(
        rename = "inputTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    input_tokens: u64,
    #[serde(
        rename = "outputTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    output_tokens: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
    #[serde(
        rename = "cacheReadTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_read_tokens: u64,
    #[serde(
        rename = "cacheWriteTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_write_tokens: u64,
}

#[derive(Debug, Clone)]
pub(super) struct DshUsageEntry {
    timestamp: TimestampMs,
    session_id: String,
    cwd: Option<Arc<str>>,
    seq: Option<i64>,
    provider: Option<String>,
    model: String,
    usage: TokenUsageRaw,
    extra_total_tokens: u64,
}

pub(super) fn read_session_file(path: &Path) -> Result<Vec<DshUsageEntry>> {
    let content = read_session_bytes(path)?;
    Ok(parse_session(&content, path))
}

fn read_session_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut content = Vec::new();
    if !is_compressed(path) {
        file.read_to_end(&mut content)?;
        return Ok(content);
    }
    // dsh appends one zstd frame per write batch, so a session that is still
    // being written can end in an incomplete frame; keep the decoded prefix.
    let mut decoder = zstd::stream::read::Decoder::new(file)?;
    if let Err(error) = decoder.read_to_end(&mut content)
        && content.is_empty()
    {
        return Err(error);
    }
    Ok(content)
}

fn parse_session(content: &[u8], path: &Path) -> Vec<DshUsageEntry> {
    let mut lines = byte_lines(content);
    let Some(header) = lines
        .next()
        .and_then(|line| serde_json::from_slice::<HeaderLine>(line).ok())
        .filter(|header| header.r#type.as_deref() == Some("session"))
    else {
        return Vec::new();
    };
    let session_id = header.id.unwrap_or_else(|| session_id_from_path(path));
    let fallback_timestamp = TimestampMs::from_millis(header.created_at.unwrap_or_default());
    let cwd = header.cwd.map(Arc::<str>::from);

    // dsh rejects a seeded log without its inherited marker, so one missing it
    // was torn while copying the parent and holds only parent usage.
    let mut awaiting_seed_end = header.is_seeded;
    let mut route = Route::default();
    let mut entries = Vec::new();
    for line in lines {
        let Ok(event) = serde_json::from_slice::<EventLine>(line) else {
            continue;
        };
        let Some(data) = event.data else {
            continue;
        };
        let usage = match event.r#type.as_deref() {
            Some("request/header") => {
                if let Some(config) = data.header.and_then(|header| header.config) {
                    update_route(&mut route, config.provider, config.model);
                }
                continue;
            }
            Some("request/context") => {
                update_route(&mut route, data.provider, data.model);
                continue;
            }
            // A fork copies its parent's log up to this marker, so usage before
            // the last inherited marker belongs to the parent session.
            Some("session/end-seed") => {
                if data.inherited {
                    entries.clear();
                    awaiting_seed_end = false;
                }
                continue;
            }
            // A failed or aborted stream settles as `assistant/attempt`; dsh still
            // counts the usage its provider reported before the failure.
            Some("assistant/message" | "assistant/attempt") => data
                .settled_usage()
                .map(|usage| (usage, route.provider.clone(), route.model.clone())),
            // Summarisation calls name their own route instead of the request route.
            Some("compaction/summary") => {
                data.usage.map(|usage| (usage, data.provider, data.model))
            }
            _ => None,
        };
        let Some((usage, provider, model)) = usage else {
            continue;
        };
        let (usage, extra_total_tokens) = apply_total_token_fallback(
            TokenUsageRaw {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_write_tokens,
                cache_read_input_tokens: usage.cache_read_tokens,
                speed: None,
                cache_creation: None,
            },
            0,
            usage.total_tokens,
        );
        if crate::total_usage_tokens(usage).saturating_add(extra_total_tokens) == 0 {
            continue;
        }
        entries.push(DshUsageEntry {
            timestamp: event
                .time
                .filter(|millis| *millis > 0)
                .map(TimestampMs::from_millis)
                .unwrap_or(fallback_timestamp),
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            seq: event.seq,
            provider,
            model: model.unwrap_or_else(|| UNKNOWN_MODEL.to_string()),
            usage,
            extra_total_tokens,
        });
    }
    if awaiting_seed_end {
        return Vec::new();
    }
    entries
}

fn update_route(route: &mut Route, provider: Option<String>, model: Option<String>) {
    if let Some(model) = model {
        route.provider = provider;
        route.model = Some(model);
    }
}

fn session_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(UNKNOWN_MODEL)
        .to_string()
}

pub(super) fn dsh_entry_key(entry: &DshUsageEntry) -> String {
    match entry.seq {
        Some(seq) => format!("{}:{seq}", entry.session_id),
        None => format!(
            "{}:{}:{}:{}:{}:{}:{}",
            entry.session_id,
            entry.timestamp.as_millis(),
            entry.model,
            entry.usage.input_tokens,
            entry.usage.output_tokens,
            entry.usage.cache_creation_input_tokens,
            entry.usage.cache_read_input_tokens,
        ),
    }
}

pub(super) fn dsh_entry_to_loaded(
    entry: DshUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let candidates = model_candidates(entry.provider.as_deref(), &entry.model);
    let total_tokens =
        crate::total_usage_tokens(entry.usage).saturating_add(entry.extra_total_tokens);
    let (cost, missing_pricing_model) = match mode {
        CostMode::Display => (0.0, None),
        CostMode::Auto | CostMode::Calculate => (
            candidates
                .iter()
                .find(|candidate| pricing.find(candidate).is_some())
                .map_or(0.0, |candidate| {
                    calculate_cost_for_usage_at(
                        Some(candidate),
                        entry.usage,
                        None,
                        Some(entry.timestamp),
                        CostMode::Calculate,
                        Some(pricing),
                    )
                }),
            missing_pricing_model_for_candidates(
                &entry.model,
                candidates,
                total_tokens,
                Some(pricing),
            ),
        ),
    };
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: crate::format_rfc3339_millis(entry.timestamp),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: None,
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("dsh"),
        session_id: Arc::from(entry.session_id),
        project_path: entry.cwd.unwrap_or_else(|| Arc::from("DeepSeek Harness")),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
        stored_cost: None,
    }
}

/// dsh's first-party providers are DeepSeek's API-key and account routes; any
/// other provider id comes from pi-ai and is tried as a models.dev provider.
///
/// First-party routes try the bare model id first because shared pricing keys
/// DeepSeek's time-of-day schedule on the direct model id.
fn model_candidates(provider: Option<&str>, model: &str) -> Vec<String> {
    if let Some("deepseek-official" | "deepseek-account") = provider {
        return vec![model.to_string(), format!("deepseek/{model}")];
    }
    let mut candidates = Vec::new();
    if let Some(provider) = provider
        && !model.starts_with(&format!("{provider}/"))
    {
        candidates.push(format!("{provider}/{model}"));
    }
    candidates.push(model.to_string());
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(lines: &[&str]) -> Vec<u8> {
        lines.join("\n").into_bytes()
    }

    fn parse(content: &[u8]) -> Vec<DshUsageEntry> {
        parse_session(
            content,
            Path::new("sessions/--p--/fallback/session.v4.jsonl"),
        )
    }

    #[test]
    fn maps_disjoint_usage_and_tracks_the_request_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1785015039278,"cwd":"/home/u/proj","isSeeded":false,"delegationDepth":0}"#,
            r#"{"type":"request/header","seq":3,"time":1785015040000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"},"tools":[]},"reason":"initial"}}"#,
            r#"{"type":"assistant/message","seq":5,"time":1785015045000,"data":{"turn":1,"step":1,"message":{"role":"assistant","content":[]},"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":999}}}],"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}}"#,
            r#"{"type":"request/context","seq":6,"time":1785015046000,"data":{"provider":"openrouter","model":"anthropic/claude-sonnet-4.5","contextWindow":200000}}"#,
            r#"{"type":"assistant/message","seq":7,"time":1785015047000,"data":{"turn":1,"step":2,"message":{},"stream":[],"usage":{"inputTokens":10,"outputTokens":5,"cacheWriteTokens":20}}}"#,
        ]));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "sid");
        assert_eq!(entries[0].seq, Some(5));
        assert_eq!(entries[0].timestamp.as_millis(), 1_785_015_045_000);
        assert_eq!(entries[0].provider.as_deref(), Some("deepseek-official"));
        assert_eq!(entries[0].model, "deepseek-v4-pro");
        assert_eq!(entries[0].usage.input_tokens, 1200);
        assert_eq!(entries[0].usage.output_tokens, 300);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 8000);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 0);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[1].provider.as_deref(), Some("openrouter"));
        assert_eq!(entries[1].model, "anthropic/claude-sonnet-4.5");
        assert_eq!(entries[1].usage.cache_creation_input_tokens, 20);
    }

    #[test]
    fn falls_back_to_the_last_stream_usage_chunk_for_attempts_and_interrupted_messages() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"assistant/attempt","seq":2,"time":3,"data":{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":5,"outputTokens":0}}},{"type":"chunk","chunk":{"type":"text","text":"x"}},{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":50,"outputTokens":3}}}]}}"#,
            r#"{"type":"assistant/message","seq":3,"time":4,"data":{"interrupted":true,"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":40,"outputTokens":2}}}]}}"#,
            r#"{"type":"assistant/attempt","seq":4,"time":5,"data":{"stream":[]}}"#,
        ]));
        let usage = entries
            .iter()
            .map(|entry| {
                (
                    entry.seq,
                    entry.usage.input_tokens,
                    entry.usage.output_tokens,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(usage, [(Some(2), 50, 3), (Some(3), 40, 2)]);
    }

    #[test]
    fn drops_a_seeded_log_torn_before_its_inherited_marker() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"fork","createdAt":1,"isSeeded":true}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTokens":10,"outputTokens":1}}}"#,
        ]));
        assert!(entries.is_empty());
    }

    #[test]
    fn counts_compaction_summaries_under_their_own_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"compaction/summary","seq":2,"time":3,"data":{"provider":"deepseek-account","model":"deepseek-flash","usage":{"inputTokens":40,"outputTokens":4}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider.as_deref(), Some("deepseek-account"));
        assert_eq!(entries[0].model, "deepseek-flash");
    }

    #[test]
    fn skips_usage_inherited_by_a_fork_but_keeps_its_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"child","createdAt":1,"isSeeded":true}"#,
            r#"{"type":"request/header","seq":1,"time":2,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-flash"}}}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTokens":100,"outputTokens":1}}}"#,
            r#"{"type":"session/end-seed","seq":3,"time":4,"data":{}}"#,
            r#"{"type":"assistant/message","seq":4,"time":5,"data":{"usage":{"inputTokens":200,"outputTokens":2}}}"#,
            r#"{"type":"session/end-seed","seq":5,"time":6,"data":{"inherited":true}}"#,
            r#"{"type":"assistant/message","seq":6,"time":7,"data":{"usage":{"inputTokens":300,"outputTokens":3}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, Some(6));
        assert_eq!(entries[0].model, "deepseek-flash");
    }

    #[test]
    fn ignores_files_without_a_session_header_and_zero_usage() {
        assert!(
            parse(br#"{"type":"assistant/message","seq":1,"data":{"usage":{"inputTokens":1}}}"#)
                .is_empty()
        );
        let entries = parse(&lines(&[
            r#"{"type":"session","createdAt":1700000000000}"#,
            "not json",
            r#"{"type":"assistant/message","seq":1,"data":{"usage":{"inputTokens":0,"outputTokens":0}}}"#,
            r#"{"type":"assistant/message","seq":2,"data":{"usage":{"totalTokens":7}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "fallback");
        assert_eq!(entries[0].model, UNKNOWN_MODEL);
        assert_eq!(entries[0].timestamp.as_millis(), 1_700_000_000_000);
        assert_eq!(entries[0].usage.output_tokens, 7);
    }

    #[test]
    fn orders_pricing_candidates_by_route() {
        assert_eq!(
            model_candidates(Some("deepseek-official"), "deepseek-v4-pro"),
            ["deepseek-v4-pro", "deepseek/deepseek-v4-pro"]
        );
        assert_eq!(
            model_candidates(Some("openrouter"), "deepseek/deepseek-v4-flash"),
            [
                "openrouter/deepseek/deepseek-v4-flash",
                "deepseek/deepseek-v4-flash"
            ]
        );
        assert_eq!(
            model_candidates(Some("openrouter"), "openrouter/auto"),
            ["openrouter/auto"]
        );
        assert_eq!(model_candidates(None, "deepseek-flash"), ["deepseek-flash"]);
    }

    #[test]
    fn prices_default_deepseek_models_offline() {
        let pricing = PricingMap::load_embedded();
        for model in ["deepseek-v4-pro", "deepseek-flash"] {
            let entry = DshUsageEntry {
                timestamp: TimestampMs::from_millis(1_785_015_045_000),
                session_id: "sid".to_string(),
                cwd: None,
                seq: Some(1),
                provider: Some("deepseek-official".to_string()),
                model: model.to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    speed: None,
                    cache_creation: None,
                },
                extra_total_tokens: 0,
            };
            let loaded = dsh_entry_to_loaded(entry, None, CostMode::Auto, &pricing);
            assert!(loaded.cost > 0.0, "{model} should be priced");
            assert_eq!(loaded.missing_pricing_model, None);
        }
    }

    #[test]
    fn applies_the_deepseek_time_of_day_schedule_to_first_party_routes() {
        let pricing = PricingMap::load_embedded();
        let cost_at = |millis: i64| {
            let entry = DshUsageEntry {
                timestamp: TimestampMs::from_millis(millis),
                session_id: "sid".to_string(),
                cwd: None,
                seq: Some(1),
                provider: Some("deepseek-official".to_string()),
                model: "deepseek-v4-pro".to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    speed: None,
                    cache_creation: None,
                },
                extra_total_tokens: 0,
            };
            dsh_entry_to_loaded(entry, None, CostMode::Auto, &pricing).cost
        };
        // Tuesday 2026-11-17 02:00Z is a peak window and 12:00Z is off-peak.
        let peak = cost_at(1_794_880_800_000);
        let off_peak = cost_at(1_794_916_800_000);
        assert!(
            (peak - off_peak * 2.0).abs() < 1e-9,
            "peak {peak} should be twice off-peak {off_peak}"
        );
    }
}
