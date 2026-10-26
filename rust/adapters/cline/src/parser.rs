use std::fs;

use serde_json::Value;

use crate::{LoadedEntry, PricingMap, Result, TokenUsageRaw, cli::CostMode};

use super::{
    loader::{EntryInput, loaded_entry},
    paths::{SourceFile, SourceKind},
};

pub(super) fn read_file(
    source: &SourceFile,
    mode: CostMode,
    pricing: &PricingMap,
    timezone: Option<&jiff::tz::TimeZone>,
) -> Result<Vec<LoadedEntry>> {
    let content = fs::read(&source.path)?;
    let root = serde_json::from_slice::<Value>(&content)?;
    match source.kind {
        SourceKind::Extension | SourceKind::CliTask => {
            parse_extension_messages(&root, source, mode, pricing, timezone)
        }
        SourceKind::Cli => parse_cli_messages(&root, source, mode, pricing, timezone),
    }
}

fn parse_extension_messages(
    root: &Value,
    source: &SourceFile,
    mode: CostMode,
    pricing: &PricingMap,
    timezone: Option<&jiff::tz::TimeZone>,
) -> Result<Vec<LoadedEntry>> {
    let Some(messages) = message_list(root) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    let model_usage = read_task_model_usage(&source.path);

    for message in messages {
        if string_field(message, "type") != Some("say") {
            continue;
        }
        let Some(say) = string_field(message, "say") else {
            continue;
        };
        if !matches!(
            say,
            "api_req_started" | "deleted_api_reqs" | "subagent_usage"
        ) {
            continue;
        }
        let Some(event_timestamp) = timestamp_field(message, &["ts", "timestamp"]) else {
            continue;
        };
        let Some(text) = message.get("text") else {
            continue;
        };
        let Some(payload) = parse_json_text(text) else {
            continue;
        };
        let Some(metrics) = metric_objects(&payload) else {
            continue;
        };
        let event_model = if say == "api_req_started" {
            model_info(message.get("modelInfo")).filter(|(model, _)| model.is_some())
        } else {
            None
        };

        for metrics in metrics {
            let Some((usage, reported_cost)) = extension_usage(metrics) else {
                continue;
            };
            let timestamp =
                timestamp_field(metrics, &["ts", "timestamp"]).unwrap_or(event_timestamp);
            let timeline_model = (say == "api_req_started")
                .then(|| model_usage_at(&model_usage, timestamp))
                .flatten();
            let (model, provider) = event_model.clone().or(timeline_model).unwrap_or_default();
            let request_id = string_field(metrics, "requestId")
                .or_else(|| string_field(metrics, "id"))
                .or_else(|| string_field(message, "requestId"))
                .or_else(|| string_field(message, "id"))
                .map(str::to_string);
            entries.push(loaded_entry(
                source,
                EntryInput {
                    timestamp,
                    model,
                    provider,
                    usage,
                    reported_cost,
                    request_id,
                },
                mode,
                pricing,
                timezone,
            ));
        }
    }
    Ok(entries)
}

fn parse_cli_messages(
    root: &Value,
    source: &SourceFile,
    mode: CostMode,
    pricing: &PricingMap,
    timezone: Option<&jiff::tz::TimeZone>,
) -> Result<Vec<LoadedEntry>> {
    let Some(messages) = root.get("messages").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();

    for message in messages {
        let role = string_field(message, "role").or_else(|| string_field(message, "type"));
        if role != Some("assistant") {
            continue;
        }
        let Some(timestamp) = timestamp_field(message, &["timestamp", "ts"]) else {
            continue;
        };
        let Some(metrics) = message.get("metrics").and_then(Value::as_object) else {
            continue;
        };
        let Some((usage, reported_cost)) = cli_usage(metrics) else {
            continue;
        };
        let (model, provider) = model_info(message.get("modelInfo")).unwrap_or_default();
        let request_id = string_field(message, "requestId")
            .or_else(|| string_field(message, "id"))
            .or_else(|| string_field(message, "messageId"))
            .map(str::to_string);
        entries.push(loaded_entry(
            source,
            EntryInput {
                timestamp,
                model,
                provider,
                usage,
                reported_cost,
                request_id,
            },
            mode,
            pricing,
            timezone,
        ));
    }
    Ok(entries)
}

fn message_list(root: &Value) -> Option<&[Value]> {
    root.as_array()
        .or_else(|| root.get("messages").and_then(Value::as_array))
        .map(Vec::as_slice)
}

fn parse_json_text(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => serde_json::from_str(text).ok(),
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        _ => None,
    }
}

fn metric_objects(value: &Value) -> Option<Vec<&Value>> {
    let mut metrics = Vec::new();
    if value.as_object().is_some_and(has_metric_fields) {
        metrics.push(value);
    } else if let Some(items) = value.as_array() {
        metrics.extend(
            items
                .iter()
                .filter(|item| item.as_object().is_some_and(has_metric_fields)),
        );
    }
    (!metrics.is_empty()).then_some(metrics)
}

fn has_metric_fields(object: &serde_json::Map<String, Value>) -> bool {
    ["tokensIn", "tokensOut", "cacheReads", "cacheWrites", "cost"]
        .iter()
        .any(|key| object.contains_key(*key))
}

fn extension_usage(metrics: &Value) -> Option<(TokenUsageRaw, Option<f64>)> {
    metrics.as_object()?;
    if !["tokensIn", "tokensOut", "cacheReads", "cacheWrites"]
        .iter()
        .any(|key| integer_field(metrics, key).is_some())
        && number_field(metrics, "cost").is_none()
    {
        return None;
    }
    let usage = TokenUsageRaw {
        input_tokens: integer_field(metrics, "tokensIn").unwrap_or_default(),
        output_tokens: integer_field(metrics, "tokensOut").unwrap_or_default(),
        cache_creation_input_tokens: integer_field(metrics, "cacheWrites").unwrap_or_default(),
        cache_read_input_tokens: integer_field(metrics, "cacheReads").unwrap_or_default(),
        speed: None,
        cache_creation: None,
    };
    Some((usage, number_field(metrics, "cost")))
}

fn cli_usage(metrics: &serde_json::Map<String, Value>) -> Option<(TokenUsageRaw, Option<f64>)> {
    if ![
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
    ]
    .iter()
    .any(|key| integer_field_from(metrics, key).is_some())
        && number_field_from(metrics, "cost").is_none()
    {
        return None;
    }
    let cache_reads = integer_field_from(metrics, "cacheReadTokens").unwrap_or_default();
    let cache_writes = integer_field_from(metrics, "cacheWriteTokens").unwrap_or_default();
    let inclusive_input = integer_field_from(metrics, "inputTokens").unwrap_or_default();
    let usage = TokenUsageRaw {
        input_tokens: inclusive_input.saturating_sub(cache_reads.saturating_add(cache_writes)),
        output_tokens: integer_field_from(metrics, "outputTokens").unwrap_or_default(),
        cache_creation_input_tokens: cache_writes,
        cache_read_input_tokens: cache_reads,
        speed: None,
        cache_creation: None,
    };
    Some((usage, number_field_from(metrics, "cost")))
}

fn model_info(value: Option<&Value>) -> Option<(Option<String>, Option<String>)> {
    let value = value?.as_object()?;
    let model = ["modelId", "id", "model"]
        .iter()
        .find_map(|key| string_field_from(value, key))
        .map(str::to_string);
    let provider = ["providerId", "provider", "providerName"]
        .iter()
        .find_map(|key| string_field_from(value, key))
        .map(str::to_string);
    (model.is_some() || provider.is_some()).then_some((model, provider))
}

#[derive(Debug, Clone)]
struct ModelUsage {
    timestamp: crate::TimestampMs,
    model: String,
    provider: Option<String>,
}

fn read_task_model_usage(source_path: &std::path::Path) -> Vec<ModelUsage> {
    let Some(metadata_path) = source_path
        .parent()
        .map(|directory| directory.join("task_metadata.json"))
    else {
        return Vec::new();
    };
    let Ok(content) = fs::read(metadata_path) else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_slice::<Value>(&content) else {
        return Vec::new();
    };
    let Some(records) = metadata.get("model_usage").and_then(Value::as_array) else {
        return Vec::new();
    };

    records
        .iter()
        .filter_map(|record| {
            let timestamp = timestamp_field(record, &["ts"])?;
            let model = string_field(record, "model_id")?.to_string();
            let provider = string_field(record, "model_provider_id").map(str::to_string);
            Some(ModelUsage {
                timestamp,
                model,
                provider,
            })
        })
        .collect()
}

fn model_usage_at(
    timeline: &[ModelUsage],
    timestamp: crate::TimestampMs,
) -> Option<(Option<String>, Option<String>)> {
    timeline
        .iter()
        .filter(|usage| usage.timestamp <= timestamp)
        .max_by_key(|usage| usage.timestamp)
        .map(|usage| (Some(usage.model.clone()), usage.provider.clone()))
}

fn timestamp_field(value: &Value, keys: &[&str]) -> Option<crate::TimestampMs> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(parse_timestamp))
}

fn parse_timestamp(value: &Value) -> Option<crate::TimestampMs> {
    match value {
        Value::String(value) => {
            if let Some(timestamp) = crate::parse_ts_timestamp(value) {
                return Some(timestamp);
            }
            parse_millisecond_timestamp(value.parse::<f64>().ok()?)
        }
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Some(crate::TimestampMs::from_millis(value))
            } else {
                parse_millisecond_timestamp(value.as_f64()?)
            }
        }
        _ => None,
    }
}

fn parse_millisecond_timestamp(value: f64) -> Option<crate::TimestampMs> {
    if !value.is_finite() {
        return None;
    }
    let millis = value.round();
    (i64::MIN as f64..=i64::MAX as f64)
        .contains(&millis)
        .then(|| crate::TimestampMs::from_millis(millis as i64))
}

fn integer_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(parse_integer)
}

fn integer_field_from(value: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    value.get(key).and_then(parse_integer)
}

fn parse_integer(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(number) => number.trim().parse().ok(),
        _ => None,
    }
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(parse_number)
}

fn number_field_from(value: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    value.get(key).and_then(parse_number)
}

fn parse_number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(number) => number.trim().parse().ok()?,
        _ => return None,
    };
    (number.is_finite() && number >= 0.0).then_some(number)
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn string_field_from<'a>(value: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CostMode;
    use serde_json::json;
    use std::path::PathBuf;

    fn source(kind: SourceKind) -> SourceFile {
        SourceFile {
            path: PathBuf::from("fixture.json"),
            kind,
            session_id: "task-a".to_string(),
        }
    }

    #[test]
    fn parses_completed_extension_requests_and_skips_partial_or_malformed_records() {
        let root = json!([
            {
                "ts": 1790000000000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514", "providerId": "anthropic", "mode": "act"},
                "text": r#"{"tokensIn":100,"tokensOut":25,"cacheReads":10,"cacheWrites":20,"cost":0.002}"#
            },
            {
                "ts": 1790000001000_i64,
                "type": "say",
                "say": "api_req_started",
                "text": r#"{"request":"request is still in progress"}"#
            },
            {
                "ts": 1790000002000_i64,
                "type": "say",
                "say": "api_req_started",
                "text": "{malformed"
            },
            {
                "ts": 1790000003000_i64,
                "type": "say",
                "say": "subagent_usage",
                "text": r#"[{"tokensIn":3,"tokensOut":4,"cacheReads":1,"cacheWrites":2}]"#
            }
        ]);
        let pricing = PricingMap::load_embedded();

        let entries = parse_extension_messages(
            &root,
            &source(SourceKind::Extension),
            CostMode::Auto,
            &pricing,
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 25);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].data.cost_usd, Some(0.002));
        assert_eq!(entries[1].data.message.usage.input_tokens, 3);
        assert_eq!(entries[1].data.message.model.as_deref(), None);
        assert_eq!(entries[1].model.as_deref(), Some("unknown"));
    }

    #[test]
    fn uses_only_timestamped_task_model_fallback_and_prefers_request_model_info() {
        let fixture = ccusage_test_support::fs_fixture!({
            "tasks/task-a/ui_messages.json": "[]",
            "tasks/task-a/task_metadata.json": r#"{
                "model_usage": [
                    {"ts": 1700000000000, "model_id": "earlier-model", "model_provider_id": "provider-a", "mode": "act"},
                    {"ts": 1700000002000, "model_id": "later-model", "model_provider_id": "provider-b", "mode": "act"}
                ]
            }"#,
        });
        let source = SourceFile {
            path: fixture.path("tasks/task-a/ui_messages.json"),
            kind: SourceKind::Extension,
            session_id: "task-a".to_string(),
        };
        let root = json!([
            {
                "ts": 1700000001000_i64,
                "type": "say",
                "say": "api_req_started",
                "text": r#"{"tokensIn":10,"tokensOut":5}"#
            },
            {
                "ts": 1700000003000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "request-model", "providerId": "provider-c", "mode": "act"},
                "text": r#"{"tokensIn":10,"tokensOut":5}"#
            },
            {
                "ts": 1700000004000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"providerId": "mismatched-provider", "mode": "act"},
                "text": r#"{"tokensIn":10,"tokensOut":5}"#
            },
            {
                "ts": 1700000003000_i64,
                "type": "say",
                "say": "deleted_api_reqs",
                "text": r#"{"tokensIn":10,"tokensOut":5}"#
            }
        ]);
        let pricing = PricingMap::load_embedded();

        let entries =
            parse_extension_messages(&root, &source, CostMode::Auto, &pricing, None).unwrap();

        assert_eq!(entries[0].model.as_deref(), Some("earlier-model"));
        assert_eq!(entries[1].model.as_deref(), Some("request-model"));
        assert_eq!(entries[2].model.as_deref(), Some("later-model"));
        assert_eq!(entries[3].model.as_deref(), Some("unknown"));
    }

    #[test]
    fn parses_cli_transcript_assistant_metrics_and_subtracts_cached_input() {
        let root = json!({
            "messages": [
                {"role": "user", "timestamp": "2026-10-01T12:00:00Z", "metrics": {"inputTokens": 10}},
                {
                    "role": "assistant",
                    "ts": 1790856060000_i64,
                    "id": "assistant-1",
                    "modelInfo": {"id": "claude-sonnet-4-20250514", "provider": "anthropic"},
                    "metrics": {"inputTokens": 100, "outputTokens": 30, "cacheReadTokens": 80, "cacheWriteTokens": 30, "cost": 0}
                },
                {
                    "role": "assistant",
                    "ts": 1790856060000_i64,
                    "id": "assistant-2",
                    "modelInfo": {"id": "claude-sonnet-4-20250514", "provider": "anthropic"},
                    "metrics": {"inputTokens": 90, "outputTokens": 12, "cacheReadTokens": 20, "cacheWriteTokens": 10}
                },
                {"role": "assistant", "metrics": {"inputTokens": 20}}
            ]
        });
        let pricing = PricingMap::load_embedded();

        let entries = parse_cli_messages(
            &root,
            &source(SourceKind::Cli),
            CostMode::Auto,
            &pricing,
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date, "2026-10-01");
        assert_eq!(entries[0].data.message.usage.input_tokens, 0);
        assert_eq!(entries[0].data.message.usage.output_tokens, 30);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 80);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            30
        );
        assert_eq!(entries[0].data.cost_usd, Some(0.0));
        assert_eq!(entries[0].stored_cost, Some(0.0));
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[1].data.message.id.as_deref(), Some("assistant-2"));
        assert_eq!(entries[1].timestamp, entries[0].timestamp);
        assert_eq!(entries[1].data.message.usage.input_tokens, 60);
        assert_eq!(entries[1].data.message.usage.output_tokens, 12);
    }

    #[test]
    fn cost_modes_preserve_reported_zero_and_calculate_known_models() {
        let source = source(SourceKind::Cli);
        let root = json!({
            "messages": [{
                "role": "assistant",
                "timestamp": 1790000000000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 1000, "outputTokens": 100, "cost": 0}
            }]
        });
        let pricing = PricingMap::load_embedded();

        let auto = parse_cli_messages(&root, &source, CostMode::Auto, &pricing, None).unwrap();
        let display =
            parse_cli_messages(&root, &source, CostMode::Display, &pricing, None).unwrap();
        let calculate =
            parse_cli_messages(&root, &source, CostMode::Calculate, &pricing, None).unwrap();

        assert_eq!(auto[0].cost, 0.0);
        assert_eq!(auto[0].stored_cost, Some(0.0));
        assert_eq!(display[0].cost, 0.0);
        assert_eq!(display[0].stored_cost, None);
        assert!(calculate[0].cost > 0.0);
        assert_eq!(calculate[0].stored_cost, None);
    }

    #[test]
    fn cost_modes_only_use_pricing_when_cost_was_not_reported() {
        let source = source(SourceKind::Cli);
        let root = json!({
            "messages": [{
                "role": "assistant",
                "ts": 1790000000000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 1000, "outputTokens": 100}
            }]
        });
        let pricing = PricingMap::load_embedded();

        let auto = parse_cli_messages(&root, &source, CostMode::Auto, &pricing, None).unwrap();
        let display =
            parse_cli_messages(&root, &source, CostMode::Display, &pricing, None).unwrap();
        let calculate =
            parse_cli_messages(&root, &source, CostMode::Calculate, &pricing, None).unwrap();

        assert!(auto[0].cost > 0.0);
        assert_eq!(display[0].cost, 0.0);
        assert_eq!(calculate[0].cost, auto[0].cost);
        assert_eq!(auto[0].data.cost_usd, None);
        assert_eq!(auto[0].stored_cost, None);
    }

    #[test]
    fn preserves_missing_cost_and_marks_unattributed_models_as_unknown() {
        let root = json!({
            "messages": [{
                "role": "assistant",
                "timestamp": 1790000000000_i64,
                "metrics": {"inputTokens": 10, "outputTokens": 5}
            }]
        });
        let pricing = PricingMap::load_embedded();

        let entries = parse_cli_messages(
            &root,
            &source(SourceKind::Cli),
            CostMode::Auto,
            &pricing,
            None,
        )
        .unwrap();

        assert_eq!(entries[0].data.cost_usd, None);
        assert_eq!(entries[0].stored_cost, None);
        assert_eq!(entries[0].model.as_deref(), Some("unknown"));
        assert_eq!(entries[0].missing_pricing_model.as_deref(), Some("unknown"));
    }

    #[test]
    fn malformed_task_metadata_does_not_drop_extension_usage() {
        let fixture = ccusage_test_support::fs_fixture!({
            "tasks/task-a/ui_messages.json": "[]",
            "tasks/task-a/task_metadata.json": "{malformed",
        });
        let source = SourceFile {
            path: fixture.path("tasks/task-a/ui_messages.json"),
            kind: SourceKind::Extension,
            session_id: "task-a".to_string(),
        };
        let root = json!([{
            "ts": 1790856000000_i64,
            "type": "say",
            "say": "api_req_started",
            "text": r#"{"tokensIn":10,"tokensOut":5}"#
        }]);

        let entries = parse_extension_messages(
            &root,
            &source,
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].model.as_deref(), Some("unknown"));
    }

    #[test]
    fn skips_invalid_or_incomplete_extension_documents() {
        let root = json!({"other": []});
        let pricing = PricingMap::load_embedded();

        let entries = parse_extension_messages(
            &root,
            &source(SourceKind::Extension),
            CostMode::Auto,
            &pricing,
            None,
        )
        .unwrap();

        assert!(entries.is_empty());
        assert!(serde_json::from_str::<Value>("not json").is_err());
    }

    #[test]
    fn parses_epoch_milliseconds_without_guessing_other_units() {
        assert_eq!(
            parse_timestamp(&json!(1790000000)),
            Some(crate::TimestampMs::from_millis(1_790_000_000))
        );
        assert_eq!(
            parse_timestamp(&json!(1790000000000_i64)),
            Some(crate::TimestampMs::from_millis(1_790_000_000_000))
        );
        assert_eq!(
            parse_timestamp(&json!(0)),
            Some(crate::TimestampMs::from_millis(0))
        );
        assert_eq!(parse_timestamp(&json!("not-a-time")), None);
    }
}
