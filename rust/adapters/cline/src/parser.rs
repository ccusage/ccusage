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
    let Some(messages) = root.as_array() else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();

    for message in messages {
        if string_field(message, "type") != Some("say") {
            continue;
        }
        let Some(say) = string_field(message, "say") else {
            continue;
        };
        if say != "api_req_started" {
            continue;
        }
        if message.get("partial").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(timestamp) = millisecond_timestamp(message.get("ts")) else {
            continue;
        };
        let Some(text) = message.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let Some(metrics) = payload.as_object() else {
            continue;
        };
        let Some((model, provider)) = extension_model_info(message.get("modelInfo")) else {
            continue;
        };
        let Some((usage, reported_cost)) = extension_usage(metrics) else {
            continue;
        };
        if is_empty_usage(usage) || !has_valid_total(usage) {
            continue;
        }
        let request_id = string_field_from(metrics, "requestId")
            .or_else(|| string_field_from(metrics, "id"))
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
        if string_field(message, "role") != Some("assistant") {
            continue;
        }
        let Some(timestamp) = cli_timestamp(message) else {
            continue;
        };
        let Some(metrics) = message.get("metrics").and_then(Value::as_object) else {
            continue;
        };
        let Some((usage, reported_cost)) = cli_usage(metrics) else {
            continue;
        };
        if is_empty_usage(usage) || !has_valid_total(usage) {
            continue;
        }
        let Some((model, provider)) = cli_model_info(message.get("modelInfo")) else {
            continue;
        };
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

fn extension_usage(
    metrics: &serde_json::Map<String, Value>,
) -> Option<(TokenUsageRaw, Option<f64>)> {
    let usage = TokenUsageRaw {
        input_tokens: integer_field_from(metrics, "tokensIn")?,
        output_tokens: integer_field_from(metrics, "tokensOut")?,
        cache_creation_input_tokens: integer_field_from(metrics, "cacheWrites")?,
        cache_read_input_tokens: integer_field_from(metrics, "cacheReads")?,
        speed: None,
        cache_creation: None,
    };
    Some((usage, number_field_from(metrics, "cost")))
}

fn cli_usage(metrics: &serde_json::Map<String, Value>) -> Option<(TokenUsageRaw, Option<f64>)> {
    let cache_reads = integer_field_from(metrics, "cacheReadTokens")?;
    let cache_writes = integer_field_from(metrics, "cacheWriteTokens")?;
    let inclusive_input = integer_field_from(metrics, "inputTokens")?;
    let cache_total = cache_reads.checked_add(cache_writes)?;
    let usage = TokenUsageRaw {
        input_tokens: inclusive_input.checked_sub(cache_total)?,
        output_tokens: integer_field_from(metrics, "outputTokens")?,
        cache_creation_input_tokens: cache_writes,
        cache_read_input_tokens: cache_reads,
        speed: None,
        cache_creation: None,
    };
    Some((usage, number_field_from(metrics, "cost")))
}

fn extension_model_info(value: Option<&Value>) -> Option<(String, Option<String>)> {
    let value = value?.as_object()?;
    let model = string_field_from(value, "modelId")?;
    valid_model_id(model).map(|model| {
        (
            model,
            string_field_from(value, "providerId").map(str::to_string),
        )
    })
}

fn cli_model_info(value: Option<&Value>) -> Option<(String, Option<String>)> {
    let value = value?.as_object()?;
    let model = string_field_from(value, "id")?;
    valid_model_id(model).map(|model| {
        (
            model,
            string_field_from(value, "provider").map(str::to_string),
        )
    })
}

fn valid_model_id(value: &str) -> Option<String> {
    (!value.eq_ignore_ascii_case("unknown")).then(|| value.to_string())
}

fn cli_timestamp(message: &Value) -> Option<crate::TimestampMs> {
    if let Some(timestamp) = message.get("ts") {
        return millisecond_timestamp(Some(timestamp));
    }
    let timestamp = crate::parse_ts_timestamp(string_field(message, "timestamp")?)?;
    jiff::Timestamp::from_millisecond(timestamp.as_millis()).ok()?;
    Some(timestamp)
}

fn millisecond_timestamp(value: Option<&Value>) -> Option<crate::TimestampMs> {
    let millis = i64::try_from(value?.as_u64()?).ok()?;
    jiff::Timestamp::from_millisecond(millis).ok()?;
    Some(crate::TimestampMs::from_millis(millis))
}

fn is_empty_usage(usage: TokenUsageRaw) -> bool {
    usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
}

fn has_valid_total(usage: TokenUsageRaw) -> bool {
    usage
        .input_tokens
        .checked_add(usage.output_tokens)
        .and_then(|total| total.checked_add(usage.cache_creation_input_tokens))
        .and_then(|total| total.checked_add(usage.cache_read_input_tokens))
        .is_some()
}

fn integer_field_from(value: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    value.get(key).and_then(parse_integer)
}

fn parse_integer(value: &Value) -> Option<u64> {
    value.as_u64()
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

        assert_eq!(entries.len(), 1);
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
    }

    #[test]
    fn skips_extension_request_when_model_is_only_in_task_metadata() {
        let fixture = ccusage_test_support::fs_fixture!({
            "tasks/task-a/ui_messages.json": "[]",
            "tasks/task-a/task_metadata.json": r#"{
                "model_usage": [
                    {"ts": 1700000000000, "model_id": "claude-sonnet-4-20250514", "model_provider_id": "anthropic", "mode": "act"}
                ]
            }"#,
        });
        let source = SourceFile {
            path: fixture.path("tasks/task-a/ui_messages.json"),
            kind: SourceKind::Extension,
            session_id: "task-a".to_string(),
        };
        let root = json!([{
            "ts": 1700000001000_i64,
            "type": "say",
            "say": "api_req_started",
            "text": r#"{"tokensIn":10,"tokensOut":5,"cacheReads":0,"cacheWrites":0}"#
        }]);

        let entries = parse_extension_messages(
            &root,
            &source,
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn extension_requires_all_integer_counters_and_drops_rollup_events() {
        let root = json!([
            {
                "ts": 1700000000000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":1,"tokensOut":0,"cacheReads":0,"cacheWrites":0}"#
            },
            {
                "ts": 1700000001000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":1,"tokensOut":1,"cacheReads":0}"#
            },
            {
                "ts": 1700000002000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":"1","tokensOut":1,"cacheReads":0,"cacheWrites":0}"#
            },
            {
                "ts": 1700000003000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":-1,"tokensOut":1,"cacheReads":0,"cacheWrites":0}"#
            },
            {
                "ts": 1700000004000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":1.5,"tokensOut":1,"cacheReads":0,"cacheWrites":0}"#
            },
            {
                "ts": 1700000005000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"cost":0.5}"#
            },
            {
                "ts": 1700000006000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":0,"tokensOut":0,"cacheReads":0,"cacheWrites":0,"cost":0.5}"#
            },
            {
                "ts": 1700000007000_i64,
                "type": "say",
                "say": "deleted_api_reqs",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":8,"tokensOut":2,"cacheReads":1,"cacheWrites":1,"cost":0.5}"#
            },
            {
                "ts": 1700000008000_i64,
                "type": "say",
                "say": "subagent_usage",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":8,"tokensOut":2,"cacheReads":1,"cacheWrites":1,"cost":0.5}"#
            },
            {
                "ts": 1700000009000_i64,
                "type": "say",
                "say": "api_req_started",
                "partial": true,
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":8,"tokensOut":2,"cacheReads":1,"cacheWrites":1,"cost":0.5}"#
            },
            {
                "ts": 1700000010000_i64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "vendor/custom-model"},
                "text": r#"{"tokensIn":18446744073709551615,"tokensOut":1,"cacheReads":0,"cacheWrites":0}"#
            }
        ]);

        let entries = parse_extension_messages(
            &root,
            &source(SourceKind::Extension),
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("vendor/custom-model"));
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("vendor/custom-model")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 0);
    }

    #[test]
    fn extension_requires_its_own_valid_timestamp_and_known_model_id() {
        let complete_text =
            r#"{"tokensIn":1,"tokensOut":1,"cacheReads":0,"cacheWrites":0,"cost":0.5}"#;
        let root = json!([
            {
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514"},
                "text": complete_text
            },
            {
                "ts": "1700000000000",
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514"},
                "text": complete_text
            },
            {
                "ts": 1700000000000.5,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514"},
                "text": complete_text
            },
            {
                "ts": 9999999999999999999_u64,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514"},
                "text": complete_text
            },
            {
                "ts": 0,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "UNKNOWN"},
                "text": complete_text
            },
            {
                "ts": 0,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "   "},
                "text": complete_text
            },
            {
                "ts": 0,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "text": complete_text
            },
            {
                "ts": 0,
                "type": "say",
                "say": "api_req_started",
                "modelInfo": {"modelId": "claude-sonnet-4-20250514"},
                "text": complete_text
            }
        ]);

        let entries = parse_extension_messages(
            &root,
            &source(SourceKind::Extension),
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, crate::TimestampMs::UNIX_EPOCH);
    }

    #[test]
    fn cli_requires_complete_usage_model_and_valid_request_timestamp() {
        let root = json!({
            "messages": [
                {
                    "role": "assistant",
                    "ts": 0,
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "ts": 1,
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3}
                },
                {
                    "role": "assistant",
                    "ts": 2,
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": "15", "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4, "cost": 0.5}
                },
                {
                    "role": "assistant",
                    "ts": 3,
                    "modelInfo": {"id": "UNKNOWN"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "ts": 4,
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "ts": "1700000000000",
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "ts": "invalid",
                    "timestamp": "1970-01-01T00:00:00Z",
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 15, "outputTokens": 2, "cacheReadTokens": 3, "cacheWriteTokens": 4}
                },
                {
                    "role": "assistant",
                    "timestamp": "1970-01-01T00:00:00Z",
                    "modelInfo": {"id": "vendor/custom-model"},
                    "metrics": {"inputTokens": 0, "outputTokens": 0, "cacheReadTokens": 0, "cacheWriteTokens": 0, "cost": 0.5}
                }
            ]
        });

        let entries = parse_cli_messages(
            &root,
            &source(SourceKind::Cli),
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, crate::TimestampMs::UNIX_EPOCH);
        assert_eq!(entries[0].model.as_deref(), Some("vendor/custom-model"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 8);
        assert_eq!(entries[0].data.message.usage.output_tokens, 2);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 3);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 4);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("vendor/custom-model")
        );
    }

    #[test]
    fn cli_rejects_inconsistent_cache_totals_instead_of_saturating() {
        let root = json!({
            "messages": [{
                "role": "assistant",
                "ts": 1700000000000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 5, "outputTokens": 2, "cacheReadTokens": 4, "cacheWriteTokens": 2, "cost": 0.5}
            }, {
                "role": "assistant",
                "ts": 1700000001000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 1, "outputTokens": 18446744073709551615_u64, "cacheReadTokens": 0, "cacheWriteTokens": 0}
            }, {
                "role": "assistant",
                "ts": 1700000002000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 18446744073709551615_u64, "outputTokens": 2, "cacheReadTokens": 18446744073709551615_u64, "cacheWriteTokens": 1}
            }]
        });

        let entries = parse_cli_messages(
            &root,
            &source(SourceKind::Cli),
            CostMode::Auto,
            &PricingMap::load_embedded(),
            None,
        )
        .unwrap();

        assert!(entries.is_empty());
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
                    "metrics": {"inputTokens": 110, "outputTokens": 30, "cacheReadTokens": 80, "cacheWriteTokens": 30, "cost": 0}
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
                "ts": 1790000000000_i64,
                "modelInfo": {"id": "claude-sonnet-4-20250514"},
                "metrics": {"inputTokens": 1000, "outputTokens": 100, "cacheReadTokens": 0, "cacheWriteTokens": 0, "cost": 0}
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
                "metrics": {"inputTokens": 1000, "outputTokens": 100, "cacheReadTokens": 0, "cacheWriteTokens": 0}
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
    fn preserves_custom_unpriced_model_ids_when_cost_is_missing() {
        let root = json!({
            "messages": [{
                "role": "assistant",
                "ts": 1790000000000_i64,
                "modelInfo": {"id": "vendor/custom-model"},
                "metrics": {"inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0, "cacheWriteTokens": 0}
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
        assert_eq!(entries[0].model.as_deref(), Some("vendor/custom-model"));
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("vendor/custom-model")
        );
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
            "modelInfo": {"modelId": "vendor/custom-model"},
            "text": r#"{"tokensIn":10,"tokensOut":5,"cacheReads":0,"cacheWrites":0}"#
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
        assert_eq!(entries[0].model.as_deref(), Some("vendor/custom-model"));
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
    fn parses_only_valid_integer_millisecond_or_documented_cli_timestamps() {
        assert_eq!(
            millisecond_timestamp(Some(&json!(1790000000000_i64))),
            Some(crate::TimestampMs::from_millis(1_790_000_000_000))
        );
        assert_eq!(
            millisecond_timestamp(Some(&json!(0))),
            Some(crate::TimestampMs::UNIX_EPOCH)
        );
        assert_eq!(millisecond_timestamp(Some(&json!(1.5))), None);
        assert_eq!(millisecond_timestamp(Some(&json!("1790000000000"))), None);
        assert_eq!(millisecond_timestamp(Some(&json!(-1))), None);
        assert_eq!(
            millisecond_timestamp(Some(&json!(9999999999999999999_u64))),
            None
        );
        assert_eq!(
            cli_timestamp(&json!({"timestamp": "1970-01-01T00:00:00Z"})),
            Some(crate::TimestampMs::UNIX_EPOCH)
        );
    }
}
