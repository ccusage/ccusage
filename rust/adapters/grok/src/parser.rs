use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_candidates, total_usage_tokens,
};
use ccusage_adapter_common::jsonl;
use ccusage_core::fast::LinePrefilter;

use super::paths::GrokSessionFiles;

#[derive(Debug, Default, Deserialize)]
struct GrokUpdateLine {
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    params: Option<GrokParams>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokParams {
    #[serde(
        default,
        rename = "sessionId",
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_id: Option<String>,
    #[serde(default)]
    update: Option<GrokUpdate>,
    #[serde(default, rename = "_meta")]
    meta: Option<GrokMeta>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokUpdate {
    #[serde(
        default,
        rename = "sessionUpdate",
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_update: Option<String>,
    #[serde(default)]
    usage: Option<GrokUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokMeta {
    #[serde(
        default,
        rename = "eventId",
        deserialize_with = "jsonl::non_empty_string"
    )]
    event_id: Option<String>,
    #[serde(default, rename = "agentTimestampMs")]
    agent_timestamp_ms: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_tokens: u64,
    #[serde(default)]
    model_usage: Option<HashMap<String, GrokModelUsage>>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokModelUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummary {
    #[serde(default)]
    info: Option<GrokSummaryInfo>,
    #[serde(
        default,
        rename = "git_root_dir",
        deserialize_with = "jsonl::non_empty_string"
    )]
    git_root_dir: Option<String>,
    #[serde(
        default,
        rename = "current_model_id",
        deserialize_with = "jsonl::non_empty_string"
    )]
    current_model_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummaryInfo {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionMeta {
    session_id: String,
    project_path: String,
    default_model: Option<String>,
}

/// Split OpenAI-style input that includes cache: uncached = input − cache.
pub(super) fn split_tokens(input: u64, cached: u64) -> (u64, u64) {
    let cache = cached.min(input);
    let uncached = input.saturating_sub(cache);
    (uncached, cache)
}

/// Pricing lookup candidates for a raw Grok model id (e.g. `grok-4.5-build`).
pub(super) fn pricing_candidates(raw_model: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push = |value: String| {
        if !candidates.iter().any(|existing| existing == &value) {
            candidates.push(value);
        }
    };

    let stripped = raw_model
        .strip_prefix("[grok] ")
        .unwrap_or(raw_model)
        .trim();
    if stripped.is_empty() {
        return candidates;
    }

    let normalized = stripped
        .strip_suffix("-build")
        .unwrap_or(stripped)
        .to_string();

    push(stripped.to_string());
    push(format!("xai/{stripped}"));
    push(format!("x-ai/{stripped}"));
    push(normalized.clone());
    push(format!("xai/{normalized}"));
    push(format!("x-ai/{normalized}"));
    candidates
}

pub(super) fn parse_session_files(
    files: &GrokSessionFiles,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let meta = load_session_meta(files);
    let content = fs::read(&files.updates)?;
    let prefilter = LinePrefilter::all(&[br#""turn_completed""#]);
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for line in jsonl::records::<GrokUpdateLine>(&content, Some(&prefilter)) {
        let Some(params) = line.params.as_ref() else {
            continue;
        };
        let Some(update) = params.update.as_ref() else {
            continue;
        };
        if update.session_update.as_deref() != Some("turn_completed") {
            continue;
        }
        let Some(usage) = update.usage.as_ref() else {
            continue;
        };

        let event_id = params.meta.as_ref().and_then(|meta| meta.event_id.clone());
        let timestamp_ms = resolve_timestamp_ms(&line, params.meta.as_ref());
        let session_id = params
            .session_id
            .clone()
            .unwrap_or_else(|| meta.session_id.clone());

        let model_rows = model_usage_rows(usage, meta.default_model.as_deref());
        for (raw_model, model_usage) in model_rows {
            let (uncached, cache) =
                split_tokens(model_usage.input_tokens, model_usage.cached_read_tokens);
            let output_tokens = model_usage.output_tokens;
            let reasoning_tokens = model_usage.reasoning_tokens;
            if uncached == 0 && cache == 0 && output_tokens == 0 && reasoning_tokens == 0 {
                continue;
            }
            let usage_raw = TokenUsageRaw {
                input_tokens: uncached,
                output_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: cache,
                speed: None,
                cache_creation: None,
            };

            let dedupe_key = dedupe_key(
                event_id.as_deref(),
                &session_id,
                timestamp_ms,
                &raw_model,
                usage_raw,
                reasoning_tokens,
            );
            if !seen.insert(dedupe_key) {
                continue;
            }

            // Display the raw modelUsage key (e.g. grok-4.5-build); Agent column already
            // identifies the source in unified reports.
            let display_model = raw_model.clone();
            // Cost bills full output_tokens only; reasoning is never added to billable output.
            let cost = calculate_grok_cost(&raw_model, usage_raw, mode, pricing);
            let missing_pricing_model = missing_grok_pricing(&raw_model, usage_raw, mode, pricing);
            let timestamp_text = format_rfc3339_millis(timestamp_ms);
            let data = UsageEntry {
                session_id: Some(session_id.clone()),
                timestamp: timestamp_text,
                version: None,
                message: UsageMessage {
                    usage: usage_raw,
                    model: Some(display_model.clone()),
                    id: event_id.clone(),
                },
                cost_usd: None,
                request_id: event_id.clone(),
                is_api_error_message: None,
                is_sidechain: None,
            };
            entries.push(LoadedEntry {
                data,
                timestamp: timestamp_ms,
                date: format_date_tz(timestamp_ms, tz),
                project: Arc::from("grok"),
                session_id: Arc::from(session_id.clone()),
                project_path: Arc::from(meta.project_path.as_str()),
                cost,
                credits: None,
                model: Some(display_model),
                message_count: None,
                usage_limit_reset_time: None,
                missing_pricing_model,
                extra_total_tokens: reasoning_tokens,
            });
            let _ = model_usage.total_tokens;
        }
    }

    Ok(entries)
}

fn model_usage_rows(
    usage: &GrokUsage,
    default_model: Option<&str>,
) -> Vec<(String, GrokModelUsage)> {
    if let Some(map) = usage.model_usage.as_ref()
        && !map.is_empty()
    {
        let mut rows: Vec<_> = map
            .iter()
            .map(|(model, usage)| (model.clone(), *usage))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        return rows;
    }

    let model = default_model
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    vec![(
        model,
        GrokModelUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            total_tokens: usage.total_tokens,
        },
    )]
}

fn load_session_meta(files: &GrokSessionFiles) -> SessionMeta {
    let session_dir_name = files
        .updates
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string();
    let project_from_path = files
        .updates
        .parent()
        .and_then(|session| session.parent())
        .and_then(|project| project.file_name())
        .and_then(|name| name.to_str())
        .map(url_decode_lightweight)
        .unwrap_or_else(|| "unknown".to_string());

    let mut session_id = session_dir_name;
    let mut project_path = project_from_path;
    let mut default_model = None;

    if let Some(summary_path) = files.summary.as_ref()
        && let Ok(content) = fs::read_to_string(summary_path)
        && let Ok(summary) = serde_json::from_str::<GrokSummary>(&content)
    {
        if let Some(id) = summary.info.as_ref().and_then(|info| info.id.clone()) {
            session_id = id;
        }
        if let Some(cwd) = summary
            .info
            .as_ref()
            .and_then(|info| info.cwd.clone())
            .or(summary.git_root_dir)
        {
            project_path = cwd;
        }
        default_model = summary.current_model_id;
    }

    SessionMeta {
        session_id,
        project_path,
        default_model,
    }
}

fn resolve_timestamp_ms(line: &GrokUpdateLine, meta: Option<&GrokMeta>) -> TimestampMs {
    if let Some(ms) = meta
        .and_then(|meta| meta.agent_timestamp_ms.as_ref())
        .and_then(value_as_i64)
        && ms > 0
    {
        return TimestampMs::from_millis(ms);
    }
    if let Some(seconds) = line.timestamp.as_ref().and_then(value_as_i64)
        && seconds > 0
    {
        // Grok writes Unix seconds on the envelope timestamp field.
        return TimestampMs::from_millis(seconds.saturating_mul(1000));
    }
    TimestampMs::UNIX_EPOCH
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.as_f64().map(|n| n as i64))
}

fn dedupe_key(
    event_id: Option<&str>,
    session_id: &str,
    timestamp: TimestampMs,
    model: &str,
    usage: TokenUsageRaw,
    reasoning: u64,
) -> String {
    if let Some(event_id) = event_id {
        return format!("{event_id}|{model}");
    }
    format!(
        "{session_id}|{}|{model}|{}|{}|{}|{reasoning}",
        timestamp.as_millis(),
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
    )
}

fn calculate_grok_cost(
    raw_model: &str,
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: &PricingMap,
) -> f64 {
    match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => {
            for candidate in pricing_candidates(raw_model) {
                if pricing.find(&candidate).is_some() {
                    return calculate_cost_for_usage(
                        Some(&candidate),
                        usage,
                        None,
                        CostMode::Calculate,
                        Some(pricing),
                    );
                }
            }
            0.0
        }
    }
}

fn missing_grok_pricing(
    raw_model: &str,
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    missing_pricing_model_for_candidates(
        raw_model,
        pricing_candidates(raw_model),
        total_usage_tokens(usage),
        Some(pricing),
    )
}

fn url_decode_lightweight(value: &str) -> String {
    // Session parents are URL-encoded cwd paths (e.g. `D%3A%5Cproj`).
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push(char::from(hi * 16 + lo));
            i += 3;
            continue;
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    out
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    fn sample_turn_completed_line() -> String {
        r#"{"timestamp":1750000000,"method":"_x.ai/session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120}}}},"_meta":{"eventId":"evt-1"}}}"#.to_string()
    }

    #[test]
    fn splits_uncached_input_from_cache() {
        assert_eq!(split_tokens(100, 40), (60, 40));
        assert_eq!(split_tokens(10, 40), (0, 10));
        assert_eq!(split_tokens(0, 5), (0, 0));
    }

    #[test]
    fn pricing_candidates_strip_build_and_add_xai() {
        assert_eq!(
            pricing_candidates("grok-4.5-build"),
            vec![
                "grok-4.5-build".to_string(),
                "xai/grok-4.5-build".to_string(),
                "x-ai/grok-4.5-build".to_string(),
                "grok-4.5".to_string(),
                "xai/grok-4.5".to_string(),
                "x-ai/grok-4.5".to_string(),
            ]
        );
    }

    #[test]
    fn exact_raw_model_pricing_override_beats_normalized_fallback() {
        let model = "grok-4.3-build".to_string();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(2.0),
            ..crate::cli::PricingOverride::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&model, &pricing_override)]);
        let usage = TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 1,
            ..TokenUsageRaw::default()
        };

        assert_eq!(
            calculate_grok_cost(&model, usage, CostMode::Calculate, &pricing),
            3.0
        );
    }

    #[test]
    fn turn_completed_model_usage_maps_tokens_without_double_count() {
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": sample_turn_completed_line(),
            "sessions/proj/sess-1/summary.json": r#"{"info":{"id":"sess-1","cwd":"D:\\work\\proj"},"current_model_id":"grok-4.5"}"#,
        });
        let files = GrokSessionFiles {
            updates: fixture.path("sessions/proj/sess-1/updates.jsonl"),
            summary: Some(fixture.path("sessions/proj/sess-1/summary.json")),
        };
        let pricing = PricingMap::load_embedded();
        let entries = parse_session_files(&files, None, CostMode::Calculate, &pricing).unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.model.as_deref(), Some("grok-4.5-build"));
        assert_eq!(entry.data.message.usage.input_tokens, 60);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 40);
        assert_eq!(entry.data.message.usage.output_tokens, 20);
        assert_eq!(entry.extra_total_tokens, 10);
        assert!(entry.data.cost_usd.is_none());
        assert_eq!(entry.session_id.as_ref(), "sess-1");
        assert_eq!(entry.project_path.as_ref(), "D:\\work\\proj");
    }

    #[test]
    fn skips_turn_completed_without_usage_and_zero_rows() {
        let lines = [
            r#"{"timestamp":1750000001,"params":{"update":{"sessionUpdate":"tool_call"}}}"#,
            r#"{"timestamp":1750000002,"params":{"update":{"sessionUpdate":"turn_completed"},"_meta":{"eventId":"no-usage"}}}"#,
            r#"{"timestamp":1750000003,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":0,"outputTokens":0,"cachedReadTokens":0,"reasoningTokens":0,"modelUsage":{"grok-4.5":{"inputTokens":0,"outputTokens":0,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"zero"}}}"#,
            &sample_turn_completed_line(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": lines,
        });
        let files = GrokSessionFiles {
            updates: fixture.path("sessions/proj/sess-1/updates.jsonl"),
            summary: None,
        };
        let pricing = PricingMap::load_embedded();
        let entries = parse_session_files(&files, None, CostMode::Display, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 60);
    }

    #[test]
    fn multi_model_turn_emits_one_entry_per_model() {
        let line = r#"{"timestamp":1750000100,"params":{"sessionId":"sess-m","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"model-a":{"inputTokens":10,"outputTokens":2,"cachedReadTokens":0,"reasoningTokens":1},"model-b":{"inputTokens":20,"outputTokens":4,"cachedReadTokens":5,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-multi"}}}"#;
        let fixture = fs_fixture!({
            "sessions/proj/sess-m/updates.jsonl": line,
        });
        let files = GrokSessionFiles {
            updates: fixture.path("sessions/proj/sess-m/updates.jsonl"),
            summary: None,
        };
        let pricing = PricingMap::load_embedded();
        let mut entries = parse_session_files(&files, None, CostMode::Display, &pricing).unwrap();
        entries.sort_by(|a, b| a.model.cmp(&b.model));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model.as_deref(), Some("model-a"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 1);
        assert_eq!(entries[1].model.as_deref(), Some("model-b"));
        assert_eq!(entries[1].data.message.usage.input_tokens, 15);
        assert_eq!(entries[1].data.message.usage.cache_read_input_tokens, 5);
    }

    #[test]
    fn dedupes_same_event_id_and_model() {
        let line = sample_turn_completed_line();
        let content = format!("{line}\n{line}\n");
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": content,
        });
        let files = GrokSessionFiles {
            updates: fixture.path("sessions/proj/sess-1/updates.jsonl"),
            summary: None,
        };
        let pricing = PricingMap::load_embedded();
        let entries = parse_session_files(&files, None, CostMode::Display, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn display_mode_cost_is_zero() {
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": sample_turn_completed_line(),
        });
        let files = GrokSessionFiles {
            updates: fixture.path("sessions/proj/sess-1/updates.jsonl"),
            summary: None,
        };
        let pricing = PricingMap::load_embedded();
        let entries = parse_session_files(&files, None, CostMode::Display, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.0);
    }
}
