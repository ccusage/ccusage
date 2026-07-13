use std::{collections::HashMap, fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use super::super::jsonl;
use super::paths::GrokSessionFiles;
use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_pricing_candidates, cli::CostMode, fast::LinePrefilter, format_date_tz,
    missing_pricing_model_for_pricing_candidates,
};

/// Top-level Grok session update line.
#[derive(Debug, Default, Deserialize)]
struct GrokLine {
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    params: Option<GrokParams>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokParams {
    #[serde(
        rename = "sessionId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    update: Option<GrokUpdate>,
    #[serde(rename = "_meta", default, deserialize_with = "jsonl::lenient_object")]
    meta: Option<GrokMeta>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokUpdate {
    #[serde(
        rename = "sessionUpdate",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_update: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<GrokUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokMeta {
    #[serde(
        rename = "eventId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    event_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokUsage {
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
        rename = "cachedReadTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cached_read_tokens: u64,
    #[serde(
        rename = "reasoningTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    reasoning_tokens: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
    #[serde(rename = "modelUsage", default)]
    model_usage: HashMap<String, GrokModelUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokModelUsage {
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
        rename = "cachedReadTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cached_read_tokens: u64,
    #[serde(
        rename = "reasoningTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    reasoning_tokens: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummary {
    #[serde(default)]
    info: Option<GrokSummaryInfo>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    git_root_dir: Option<String>,
    #[serde(
        rename = "current_model_id",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    current_model_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummaryInfo {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
}

pub(super) fn parse_session_files(
    files: &GrokSessionFiles,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let session_meta = session_metadata(files);
    let content = fs::read(&files.updates)?;
    // Prefer turn_completed lines that carry usage; admit either marker so the
    // prefilter stays cheap without dropping multi-model rows.
    let prefilter = LinePrefilter::any(&[br#""turn_completed""#, br#""modelUsage""#]);
    let mut entries = Vec::new();
    for record in jsonl::records::<GrokLine>(&content, Some(&prefilter)) {
        entries.extend(parse_turn_completed(
            &record,
            &session_meta,
            tz,
            mode,
            pricing,
        ));
    }
    Ok(entries)
}

struct SessionMeta {
    session_id: String,
    project: String,
    project_path: String,
    fallback_model: Option<String>,
}

fn session_metadata(files: &GrokSessionFiles) -> SessionMeta {
    let session_id = files
        .updates
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut project_path = decode_project_path_from_updates(&files.updates);
    let mut fallback_model = None;
    if let Some(summary_path) = files.summary.as_ref()
        && let Ok(bytes) = fs::read(summary_path)
        && let Ok(summary) = serde_json::from_slice::<GrokSummary>(&bytes)
    {
        if let Some(cwd) = summary
            .info
            .as_ref()
            .and_then(|info| info.cwd.clone())
            .or(summary.git_root_dir.clone())
        {
            project_path = cwd.trim_end_matches('/').to_string();
        }
        if let Some(id) = summary
            .info
            .as_ref()
            .and_then(|info| info.id.clone())
            .filter(|id| !id.is_empty())
        {
            // Prefer explicit summary session id when present.
            let _ = id;
        }
        fallback_model = summary.current_model_id;
    }

    let project = project_path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("grok")
        .to_string();

    SessionMeta {
        session_id,
        project,
        project_path,
        fallback_model,
    }
}

fn decode_project_path_from_updates(updates: &Path) -> String {
    // sessions/<url-encoded-project>/<uuid>/updates.jsonl
    let mut components = updates
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    // Drop file name and session uuid when present.
    if components
        .last()
        .is_some_and(|name| *name == "updates.jsonl")
    {
        components.pop();
    }
    if components.len() >= 2 {
        let encoded = components[components.len() - 2];
        if encoded != "sessions" {
            return percent_decode(encoded);
        }
    }
    for (index, component) in components.iter().enumerate() {
        if *component == "sessions" {
            if let Some(encoded) = components.get(index + 1) {
                return percent_decode(encoded);
            }
        }
    }
    "Grok".to_string()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (from_hex(bytes[index + 1]), from_hex(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_turn_completed(
    record: &GrokLine,
    session_meta: &SessionMeta,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    let Some(params) = record.params.as_ref() else {
        return Vec::new();
    };
    let Some(update) = params.update.as_ref() else {
        return Vec::new();
    };
    if update.session_update.as_deref() != Some("turn_completed") {
        return Vec::new();
    }
    let Some(usage) = update.usage.as_ref() else {
        return Vec::new();
    };

    let session_id = params
        .session_id
        .clone()
        .unwrap_or_else(|| session_meta.session_id.clone());
    let event_id = params.meta.as_ref().and_then(|meta| meta.event_id.clone());
    let timestamp = parse_timestamp(record.timestamp.as_ref()).unwrap_or(TimestampMs::UNIX_EPOCH);
    let timestamp_text = crate::format_rfc3339_millis(timestamp);

    let model_rows = model_usage_rows(usage, session_meta.fallback_model.as_deref());
    let mut entries = Vec::with_capacity(model_rows.len());
    for (raw_model, model_usage) in model_rows {
        let input_tokens = model_usage.input_tokens;
        let output_tokens = model_usage.output_tokens;
        let cache_read = model_usage.cached_read_tokens;
        let reasoning_tokens = model_usage.reasoning_tokens;
        if input_tokens == 0 && output_tokens == 0 && cache_read == 0 && reasoning_tokens == 0 {
            // Skip zero-token rows (including empty totalTokens-only metadata).
            if model_usage.total_tokens == 0 {
                continue;
            }
            // totalTokens alone without billable breakdown is ignored.
            continue;
        }

        let usage_raw = TokenUsageRaw {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cache_read,
            speed: None,
            cache_creation: None,
        };
        let display_model = format!("[grok] {raw_model}");
        let candidates = pricing_candidates(&display_model, &raw_model);
        let cost = calculate_grok_cost(&candidates, usage_raw, reasoning_tokens, mode, pricing);
        let missing_pricing_model = missing_grok_pricing(
            &display_model,
            &candidates,
            usage_raw,
            reasoning_tokens,
            mode,
            pricing,
        );
        let message_id = event_id
            .as_ref()
            .map(|id| format!("{id}:{raw_model}"))
            .or_else(|| Some(format!("{session_id}:{timestamp_text}:{raw_model}")));
        let data = UsageEntry {
            session_id: Some(session_id.clone()),
            timestamp: timestamp_text.clone(),
            version: None,
            message: UsageMessage {
                usage: usage_raw,
                model: Some(display_model.clone()),
                id: message_id,
            },
            cost_usd: None,
            request_id: event_id.clone(),
            is_api_error_message: None,
            is_sidechain: None,
        };
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from(session_meta.project.as_str()),
            session_id: Arc::from(session_id.as_str()),
            project_path: Arc::from(session_meta.project_path.as_str()),
            cost,
            credits: None,
            model: Some(display_model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            extra_total_tokens: reasoning_tokens,
            message_count: None,
            data,
        });
    }
    entries
}

struct ModelUsageRow {
    input_tokens: u64,
    output_tokens: u64,
    cached_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
}

fn model_usage_rows(
    usage: &GrokUsage,
    fallback_model: Option<&str>,
) -> Vec<(String, ModelUsageRow)> {
    if !usage.model_usage.is_empty() {
        let mut rows = usage
            .model_usage
            .iter()
            .map(|(model, row)| {
                (
                    model.clone(),
                    ModelUsageRow {
                        input_tokens: row.input_tokens,
                        output_tokens: row.output_tokens,
                        cached_read_tokens: row.cached_read_tokens,
                        reasoning_tokens: row.reasoning_tokens,
                        total_tokens: row.total_tokens,
                    },
                )
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        return rows;
    }
    let model = fallback_model.unwrap_or("unknown").to_string();
    vec![(
        model,
        ModelUsageRow {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            total_tokens: usage.total_tokens,
        },
    )]
}

fn parse_timestamp(value: Option<&Value>) -> Option<TimestampMs> {
    let value = value?;
    if let Some(raw) = value.as_i64() {
        let millis = if raw > 1_000_000_000_000 {
            raw
        } else {
            raw.checked_mul(1_000)?
        };
        return (millis > 0).then(|| TimestampMs::from_millis(millis));
    }
    if let Some(raw) = value.as_u64() {
        let millis = if raw > 1_000_000_000_000 {
            i64::try_from(raw).ok()?
        } else {
            i64::try_from(raw.checked_mul(1_000)?).ok()?
        };
        return (millis > 0).then(|| TimestampMs::from_millis(millis));
    }
    crate::parse_ts_timestamp(value.as_str()?)
}

fn pricing_candidates(display_model: &str, raw_model: &str) -> Vec<String> {
    let mut candidates = vec![
        display_model.to_string(),
        raw_model.to_string(),
        format!("xai/{raw_model}"),
        format!("x-ai/{raw_model}"),
    ];
    candidates.dedup();
    candidates
}

/// Bill reasoning at the output rate while leaving displayed output unchanged.
fn calculate_grok_cost(
    candidates: &[String],
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(reasoning_tokens),
        cache_creation: None,
        ..usage
    };
    calculate_cost_for_pricing_candidates(
        candidates.iter().map(String::as_str),
        cost_usage,
        None,
        mode,
        pricing,
    )
}

fn missing_grok_pricing(
    display_model: &str,
    candidates: &[String],
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(reasoning_tokens),
        cache_creation: None,
        ..usage
    };
    missing_pricing_model_for_pricing_candidates(
        display_model,
        candidates.iter().map(String::as_str),
        crate::total_usage_tokens(cost_usage),
        None,
        mode,
        pricing,
    )
}

pub(super) fn entry_id(entry: &LoadedEntry) -> String {
    if let Some(request_id) = entry.data.request_id.as_ref()
        && let Some(model) = entry.model.as_ref()
    {
        return format!("grok:{request_id}:{model}");
    }
    if let Some(message_id) = entry.data.message.id.as_ref() {
        return format!("grok:{message_id}");
    }
    let usage = entry.data.message.usage;
    [
        "grok".to_string(),
        entry.session_id.to_string(),
        entry.data.timestamp.clone(),
        entry.model.clone().unwrap_or_default(),
        usage.input_tokens.to_string(),
        usage.output_tokens.to_string(),
        usage.cache_read_input_tokens.to_string(),
        entry.extra_total_tokens.to_string(),
    ]
    .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    fn turn_line(
        timestamp: i64,
        session_id: &str,
        event_id: &str,
        model_usage_json: &str,
    ) -> String {
        format!(
            r#"{{"timestamp":{timestamp},"method":"_x.ai/session/update","params":{{"sessionId":"{session_id}","update":{{"sessionUpdate":"turn_completed","usage":{{"inputTokens":1,"outputTokens":1,"modelUsage":{model_usage_json}}}}},"_meta":{{"eventId":"{event_id}"}}}}}}"#
        )
    }

    #[test]
    fn parses_happy_path_tokens_and_utc_date() {
        let line = turn_line(
            1_783_903_216,
            "sess-1",
            "evt-1",
            r#"{"grok-4.5":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":130}}"#,
        );
        let fixture = fs_fixture!({
            "sessions/%2Fproj%2Fdemo/sess-1/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture
                .root()
                .join("sessions/%2Fproj%2Fdemo/sess-1/updates.jsonl"),
            summary: None,
        };
        let entries = parse_session_files(&files, None, CostMode::Calculate, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-07-13");
        assert_eq!(entries[0].session_id.as_ref(), "sess-1");
        assert_eq!(entries[0].model.as_deref(), Some("[grok] grok-4.5"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 40);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert!(entries[0].project_path.as_ref().contains("proj"));
    }

    #[test]
    fn multi_model_emits_one_entry_per_model() {
        let line = turn_line(
            1_783_903_216,
            "sess-2",
            "evt-2",
            r#"{"grok-4.5":{"inputTokens":10,"outputTokens":1,"reasoningTokens":0},"grok-composer-2.5-fast":{"inputTokens":5,"outputTokens":2,"reasoningTokens":1}}"#,
        );
        let fixture = fs_fixture!({
            "s/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture.root().join("s/updates.jsonl"),
            summary: None,
        };
        let entries = parse_session_files(&files, None, CostMode::Calculate, None).unwrap();
        assert_eq!(entries.len(), 2);
        let models: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.model.clone())
            .collect();
        assert!(models.iter().any(|m| m == "[grok] grok-4.5"));
        assert!(models.iter().any(|m| m == "[grok] grok-composer-2.5-fast"));
    }

    #[test]
    fn skips_zero_token_and_non_turn_completed() {
        let content = [
            r#"{"timestamp":1783903216,"params":{"sessionId":"s","update":{"sessionUpdate":"tool_call_update","usage":{"totalTokens":99999}},"_meta":{"eventId":"e0"}}}"#,
            r#"{"timestamp":1783903216,"params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":0,"outputTokens":0,"modelUsage":{"m":{"inputTokens":0,"outputTokens":0}}}},"_meta":{"eventId":"e1"}}}"#,
            &turn_line(
                1_783_903_216,
                "s",
                "e2",
                r#"{"m":{"inputTokens":3,"outputTokens":1}}"#,
            ),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "s/updates.jsonl": content,
        });
        let files = GrokSessionFiles {
            updates: fixture.root().join("s/updates.jsonl"),
            summary: None,
        };
        let entries = parse_session_files(&files, None, CostMode::Calculate, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 3);
    }

    #[test]
    fn timestamp_seconds_group_to_calendar_day() {
        // 1783901683 seconds ≈ 2026-07-13 UTC
        let line = turn_line(
            1_783_901_683,
            "s",
            "e",
            r#"{"m":{"inputTokens":1,"outputTokens":1}}"#,
        );
        let fixture = fs_fixture!({
            "s/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture.root().join("s/updates.jsonl"),
            summary: None,
        };
        let entries = parse_session_files(&files, None, CostMode::Calculate, None).unwrap();
        assert_eq!(entries[0].date, "2026-07-13");
    }

    #[test]
    fn summary_cwd_preferred_for_project_path() {
        let line = turn_line(
            1_783_903_216,
            "uuid-1",
            "e",
            r#"{"m":{"inputTokens":1,"outputTokens":1}}"#,
        );
        let fixture = fs_fixture!({
            "sessions/%2Fencoded/uuid-1/updates.jsonl": line,
            "sessions/%2Fencoded/uuid-1/summary.json": r#"{"info":{"id":"uuid-1","cwd":"/Users/rk/Projects/real"},"git_root_dir":"/Users/rk/Projects/real/"}"#,
        });
        let files = GrokSessionFiles {
            updates: fixture
                .root()
                .join("sessions/%2Fencoded/uuid-1/updates.jsonl"),
            summary: Some(
                fixture
                    .root()
                    .join("sessions/%2Fencoded/uuid-1/summary.json"),
            ),
        };
        let entries = parse_session_files(&files, None, CostMode::Calculate, None).unwrap();
        assert_eq!(entries[0].project_path.as_ref(), "/Users/rk/Projects/real");
        assert_eq!(entries[0].project.as_ref(), "real");
    }

    #[test]
    fn reasoning_billed_as_output_for_cost_display_output_unchanged() {
        let line = turn_line(
            1_783_903_216,
            "s",
            "e",
            r#"{"priced-model":{"inputTokens":0,"outputTokens":100,"reasoningTokens":50}}"#,
        );
        let fixture = fs_fixture!({
            "s/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture.root().join("s/updates.jsonl"),
            summary: None,
        };
        let mut shared = crate::cli::SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            ..crate::cli::SharedArgs::default()
        };
        shared.pricing_overrides.insert(
            "priced-model".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(0.0),
                output_cost_per_token: Some(1e-3),
                ..Default::default()
            },
        );
        let pricing = PricingMap::load_with_overrides(true, false, shared.pricing_overrides.iter());
        let entries =
            parse_session_files(&files, None, CostMode::Calculate, Some(&pricing)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 100);
        assert_eq!(entries[0].extra_total_tokens, 50);
        // 150 output units * 0.001
        assert!((entries[0].cost - 0.15).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_sets_missing_pricing() {
        let line = turn_line(
            1_783_903_216,
            "s",
            "e",
            r#"{"totally-unknown-model-xyz":{"inputTokens":10,"outputTokens":5}}"#,
        );
        let fixture = fs_fixture!({
            "s/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture.root().join("s/updates.jsonl"),
            summary: None,
        };
        let pricing = PricingMap::load_with_overrides(true, false, std::iter::empty());
        let entries =
            parse_session_files(&files, None, CostMode::Calculate, Some(&pricing)).unwrap();
        assert_eq!(entries[0].cost, 0.0);
        assert!(entries[0].missing_pricing_model.is_some());
    }
}
