use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, cli::CostMode, fast::LinePrefilter,
    format_date_tz, missing_pricing_model_for_usage,
};
use ccusage_adapter_common::jsonl;

/// A single parsed OpenClaw session line. Only the fields ccusage consumes are
/// declared; serde skips everything else. Both `model_change`/`model-snapshot`
/// records and assistant `message` records share this struct so the stateful
/// model/provider tracking can read either shape in order.
#[derive(Debug, Default, Deserialize)]
struct OpenClawLine {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(rename = "customType", default)]
    custom_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_model_source")]
    data: Option<OpenClawModelSource>,
    #[serde(
        rename = "modelId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    // A non-object `message` previously navigated to no usage without dropping
    // the line, so deserialize it leniently. Otherwise a malformed `message` on
    // a model-tracking line would fail the whole record and lose the
    // model/provider state update for subsequent usage entries.
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    message: Option<OpenClawMessage>,
    timestamp: Option<Value>,
}

/// Deserialize the `data` block of a model-change record, mirroring the
/// historical `Value::as_object` navigation: only JSON objects yield a source,
/// while any other shape (or a missing key) becomes `None` instead of failing
/// the whole line.
fn deserialize_model_source<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<OpenClawModelSource>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        value
            .is_object()
            .then(|| serde_json::from_value(value).ok())
            .flatten()
    }))
}

/// Model/provider fields carried either at the root of a `model_change` record
/// or nested under its `data` key.
#[derive(Debug, Default, Deserialize)]
struct OpenClawModelSource {
    #[serde(
        rename = "modelId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
}

/// Assistant message payload carrying token usage and per-message metadata.
#[derive(Debug, Default, Deserialize)]
struct OpenClawMessage {
    #[serde(default)]
    role: Option<String>,
    usage: Option<OpenClawUsage>,
    timestamp: Option<Value>,
    #[serde(
        rename = "modelId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
}

/// Token usage block carried by OpenClaw assistant messages.
#[derive(Debug, Default, Deserialize)]
struct OpenClawUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(rename = "cacheRead", default, deserialize_with = "jsonl::lenient_u64")]
    cache_read: u64,
    #[serde(
        rename = "cacheWrite",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_write: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_cost")]
    cost: Option<OpenClawCost>,
}

/// Deserialize the `cost` block, mirroring the historical
/// `usage.get("cost").and_then(|cost| cost.get("total"))` navigation: only JSON
/// objects yield a cost, while any other shape (or a missing key) becomes `None`
/// instead of failing the whole line.
fn deserialize_cost<'de, D>(deserializer: D) -> std::result::Result<Option<OpenClawCost>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        value
            .is_object()
            .then(|| serde_json::from_value(value).ok())
            .flatten()
    }))
}

/// Precomputed cost block carried alongside OpenClaw usage.
#[derive(Debug, Default, Deserialize)]
struct OpenClawCost {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    total: Option<f64>,
}

#[derive(Debug, Clone)]
struct OpenClawEntry {
    timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    model: String,
    provider: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    cost: Option<f64>,
}

pub(super) fn parse_session_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let session_id = extract_session_id(path);
    let fallback_timestamp = file_modified_timestamp(path);
    let content = fs::read(path)?;
    // OpenClaw lines that matter are either model-tracking records
    // (`model_change`/`model-snapshot`) or assistant records carrying `usage`,
    // so admit lines containing any of those substrings before JSON parsing.
    let prefilter =
        LinePrefilter::any(&[br#""model_change""#, br#""model-snapshot""#, br#""usage""#]);
    let mut current_model = None::<String>;
    let mut current_provider = None::<String>;
    let mut entries = Vec::new();
    for record in jsonl::records::<OpenClawLine>(&content, Some(&prefilter)) {
        if is_model_change(&record) {
            let (source_model_id, source_model, source_provider) = match record.data.as_ref() {
                Some(source) => (
                    source.model_id.clone(),
                    source.model.clone(),
                    source.provider.clone(),
                ),
                None => (
                    record.model_id.clone(),
                    record.model.clone(),
                    record.provider.clone(),
                ),
            };
            if let Some(model) = source_model_id.or(source_model) {
                current_model = Some(model);
            }
            if let Some(provider) = source_provider {
                current_provider = Some(provider);
            }
            continue;
        }
        if let Some(entry) = parse_message_entry(
            &record,
            &session_id,
            current_model.as_deref(),
            current_provider.as_deref(),
            fallback_timestamp,
        ) {
            entries.push(openclaw_entry_to_loaded(entry, tz, mode, pricing));
        }
    }
    Ok(entries)
}

fn is_model_change(record: &OpenClawLine) -> bool {
    if record.r#type.as_deref() == Some("model_change") {
        return true;
    }
    record.r#type.as_deref() == Some("custom")
        && record.custom_type.as_deref() == Some("model-snapshot")
}

fn parse_message_entry(
    record: &OpenClawLine,
    session_id: &str,
    current_model: Option<&str>,
    current_provider: Option<&str>,
    fallback_timestamp: TimestampMs,
) -> Option<OpenClawEntry> {
    if record.r#type.as_deref() != Some("message") {
        return None;
    }
    let message = record.message.as_ref()?;
    if message.role.as_deref() != Some("assistant") {
        return None;
    }
    let usage = message.usage.as_ref()?;
    let input_tokens = usage.input;
    let output_tokens = usage.output;
    let cache_read_tokens = usage.cache_read;
    let cache_creation_tokens = usage.cache_write;
    let total_tokens = usage.total_tokens;
    let raw_usage = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: cache_creation_tokens,
        cache_read_input_tokens: cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let (raw_usage, extra_total_tokens) = apply_total_token_fallback(raw_usage, 0, total_tokens);
    if crate::total_usage_tokens(raw_usage) + extra_total_tokens == 0 {
        return None;
    }
    let total_tokens = total_tokens.max(crate::total_usage_tokens(raw_usage) + extra_total_tokens);
    let timestamp = timestamp_from_value(message.timestamp.as_ref().or(record.timestamp.as_ref()))
        .unwrap_or(fallback_timestamp);
    let model = message
        .model_id
        .clone()
        .or_else(|| message.model.clone())
        .or_else(|| current_model.map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let provider = message
        .provider
        .clone()
        .or_else(|| current_provider.map(str::to_string));
    Some(OpenClawEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: session_id.to_string(),
        model: format!("[openclaw] {model}"),
        provider,
        input_tokens: raw_usage.input_tokens,
        output_tokens: raw_usage.output_tokens,
        cache_creation_tokens: raw_usage.cache_creation_input_tokens,
        cache_read_tokens: raw_usage.cache_read_input_tokens,
        total_tokens,
        cost: usage.cost.as_ref().and_then(|cost| cost.total),
    })
}

fn openclaw_entry_to_loaded(
    entry: OpenClawEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: entry.provider.clone(),
        message: UsageMessage {
            usage,
            model: Some(entry.model.clone()),
            id: None,
        },
        cost_usd: entry.cost,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost = calculate_cost_for_usage(Some(&entry.model), usage, entry.cost, mode, pricing);
    let missing_pricing_model =
        missing_pricing_model_for_usage(Some(&entry.model), usage, entry.cost, mode, pricing);
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("openclaw"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("OpenClaw"),
        cost,
        extra_total_tokens: entry.total_tokens.saturating_sub(
            entry.input_tokens
                + entry.output_tokens
                + entry.cache_creation_tokens
                + entry.cache_read_tokens,
        ),
        credits: None,
        message_count: None,
        model: Some(entry.model),
        data,
        usage_limit_reset_time: None,
        missing_pricing_model,
    }
}

fn timestamp_from_value(value: Option<&Value>) -> Option<TimestampMs> {
    let value = value?;
    if let Some(raw) = value.as_i64() {
        return Some(TimestampMs::from_millis(raw));
    }
    crate::parse_ts_timestamp(value.as_str()?)
}

fn extract_session_id(path: &Path) -> String {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let Some(index) = filename.find(".jsonl") else {
        return filename.to_string();
    };
    let stem = &filename[..index];
    if stem.is_empty() {
        filename.to_string()
    } else {
        stem.to_string()
    }
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}

pub(super) fn entry_id(entry: &LoadedEntry) -> String {
    let usage = entry.data.message.usage;
    [
        "openclaw".to_string(),
        entry.session_id.to_string(),
        entry.data.timestamp.clone(),
        entry.model.clone().unwrap_or_default(),
        usage.input_tokens.to_string(),
        usage.output_tokens.to_string(),
        usage.cache_creation_input_tokens.to_string(),
        usage.cache_read_input_tokens.to_string(),
        entry.extra_total_tokens.to_string(),
        entry.cost.to_string(),
    ]
    .join(":")
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;

    use super::*;

    #[test]
    fn malformed_message_does_not_drop_model_tracking_line() {
        // A `model_change` line whose `message` field is a non-object must still
        // deserialize and update the tracked model/provider, instead of being
        // dropped by `records().ok()` and losing state for later usage entries.
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai","message":"not-an-object"}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let file = fixture.path("session.jsonl");

        let entries = parse_session_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].data.version.as_deref(), Some("openai"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
    }

    #[test]
    fn falls_back_to_total_tokens_when_openclaw_parts_are_missing() {
        let record = serde_json::from_value::<OpenClawLine>(serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "gpt-5.2",
                "usage": {
                    "totalTokens": 222
                }
            }
        }))
        .unwrap();
        let entry =
            parse_message_entry(&record, "session-a", None, None, TimestampMs::UNIX_EPOCH).unwrap();

        assert_eq!(entry.output_tokens, 222);
        assert_eq!(entry.total_tokens, 222);
    }
}
