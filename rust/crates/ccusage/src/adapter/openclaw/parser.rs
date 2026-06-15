use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::{Map, Value};

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage,
    cli::CostMode,
    fast::{LinePrefilter, prefiltered_json_values},
    format_date_tz, json_value_u64, missing_pricing_model_for_usage, non_empty_json_string,
};

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
    let mut current_model = None::<String>;
    let mut current_provider = None::<String>;
    let mut entries = Vec::new();
    let prefilter =
        LinePrefilter::any(&[br#""model_change""#, br#""model-snapshot""#, br#""usage""#]);
    for value in prefiltered_json_values(&content, &prefilter) {
        let Some(record) = value.as_object() else {
            continue;
        };
        if is_model_change(record) {
            let source = record
                .get("data")
                .and_then(Value::as_object)
                .unwrap_or(record);
            if let Some(model) = non_empty_json_string(source.get("modelId"))
                .or_else(|| non_empty_json_string(source.get("model")))
            {
                current_model = Some(model);
            }
            if let Some(provider) = non_empty_json_string(source.get("provider")) {
                current_provider = Some(provider);
            }
            continue;
        }
        if let Some(entry) = parse_message_entry(
            record,
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

fn is_model_change(record: &Map<String, Value>) -> bool {
    if record.get("type").and_then(Value::as_str) == Some("model_change") {
        return true;
    }
    record.get("type").and_then(Value::as_str) == Some("custom")
        && record.get("customType").and_then(Value::as_str) == Some("model-snapshot")
}

fn parse_message_entry(
    record: &Map<String, Value>,
    session_id: &str,
    current_model: Option<&str>,
    current_provider: Option<&str>,
    fallback_timestamp: TimestampMs,
) -> Option<OpenClawEntry> {
    if record.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = record.get("message")?.as_object()?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = message.get("usage")?.as_object()?;
    let input_tokens = json_value_u64(usage.get("input"));
    let output_tokens = json_value_u64(usage.get("output"));
    let cache_read_tokens = json_value_u64(usage.get("cacheRead"));
    let cache_creation_tokens = json_value_u64(usage.get("cacheWrite"));
    let total_tokens = json_value_u64(usage.get("totalTokens"));
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
    let timestamp =
        timestamp_from_value(message.get("timestamp").or_else(|| record.get("timestamp")))
            .unwrap_or(fallback_timestamp);
    let model = non_empty_json_string(message.get("modelId"))
        .or_else(|| non_empty_json_string(message.get("model")))
        .or_else(|| current_model.map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let provider = non_empty_json_string(message.get("provider"))
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
        cost: usage
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64),
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
    use super::*;

    #[test]
    fn falls_back_to_total_tokens_when_openclaw_parts_are_missing() {
        let record = serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "gpt-5.2",
                "usage": {
                    "totalTokens": 222
                }
            }
        });
        let entry = parse_message_entry(
            record.as_object().unwrap(),
            "session-a",
            None,
            None,
            TimestampMs::UNIX_EPOCH,
        )
        .unwrap();

        assert_eq!(entry.output_tokens, 222);
        assert_eq!(entry.total_tokens, 222);
    }
}
