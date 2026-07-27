use std::{collections::HashMap, fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, Result, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost, cli::CostMode, format_date_tz, json_value_u64,
    missing_pricing_model_for_usage, non_empty_json_string,
};

/// A single Amp thread file: one JSON object holding the thread id, the chat
/// messages, and an optional usage ledger.
///
/// Polymorphic and leniency-sensitive leaves (token blocks, ids, credits) stay
/// as raw [`Value`]s so navigation matches the historical `Value::get` parsing
/// exactly instead of failing the whole file on an unexpected field type.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpThread {
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::non_empty_string"
    )]
    id: Option<String>,
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::lenient_vec"
    )]
    messages: Vec<AmpMessage>,
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::lenient_object"
    )]
    usage_ledger: Option<AmpUsageLedger>,
}

/// Wrapper around the ledger's `events` array.
#[derive(Debug, Default, Deserialize)]
struct AmpUsageLedger {
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::lenient_array"
    )]
    events: Option<Vec<AmpLedgerEvent>>,
}

/// A single ledger event carrying token counts and pricing metadata.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpLedgerEvent {
    #[serde(default)]
    id: Option<Value>,
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::non_empty_string"
    )]
    timestamp: Option<String>,
    #[serde(
        default,
        deserialize_with = "ccusage_adapter_common::jsonl::non_empty_string"
    )]
    model: Option<String>,
    #[serde(default)]
    tokens: Option<Value>,
    #[serde(default)]
    to_message_id: Option<Value>,
    #[serde(default)]
    credits: Option<Value>,
}

/// A single chat message in the thread.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpMessage {
    #[serde(default)]
    role: Option<Value>,
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    model: Option<Value>,
    #[serde(default)]
    message_id: Option<Value>,
}

pub fn read_thread_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let content = fs::read(path)?;
    let Ok(thread) = serde_json::from_slice::<AmpThread>(&content) else {
        return Ok(Vec::new());
    };
    let Some(thread_id) = thread.id else {
        return Ok(Vec::new());
    };
    let messages = &thread.messages;

    if let Some(ledger) = thread.usage_ledger.as_ref()
        && let Some(events) = ledger.events.as_ref()
    {
        let cache_tokens = cache_tokens_by_message_id(messages);
        return Ok(parse_ledger_events(
            events,
            &cache_tokens,
            &thread_id,
            tz,
            mode,
            pricing,
        ));
    }

    Ok(parse_message_usage(messages, &thread_id, tz, mode, pricing))
}

fn parse_ledger_events(
    events: &[AmpLedgerEvent],
    cache_tokens: &HashMap<i64, (u64, u64)>,
    thread_id: &str,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for event in events {
        let Some(timestamp_text) = event.timestamp.clone() else {
            continue;
        };
        let Some(timestamp) = crate::parse_ts_timestamp(&timestamp_text) else {
            continue;
        };
        let Some(model) = event.model.clone() else {
            continue;
        };
        let Some(tokens) = event.tokens.as_ref() else {
            continue;
        };
        let cache = event
            .to_message_id
            .as_ref()
            .and_then(Value::as_i64)
            .and_then(|id| cache_tokens.get(&id).copied())
            .unwrap_or_default();
        let usage = TokenUsageRaw {
            input_tokens: json_value_u64(tokens.get("input")),
            output_tokens: json_value_u64(tokens.get("output")),
            cache_creation_input_tokens: cache.0,
            cache_read_input_tokens: cache.1,
            speed: None,
            cache_creation: None,
        };
        let total_tokens = json_value_u64(tokens.get("total"));
        let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, total_tokens);
        if usage.input_tokens == 0
            && usage.output_tokens == 0
            && usage.cache_creation_input_tokens == 0
            && usage.cache_read_input_tokens == 0
            && extra_total_tokens == 0
        {
            continue;
        }
        let data = UsageEntry {
            session_id: Some(thread_id.to_string()),
            timestamp: timestamp_text,
            version: None,
            message: UsageMessage {
                usage,
                model: Some(model.clone()),
                id: non_empty_json_string(event.id.as_ref()),
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        let cost_data = UsageEntry {
            message: UsageMessage {
                usage: TokenUsageRaw {
                    output_tokens: data
                        .message
                        .usage
                        .output_tokens
                        .saturating_add(extra_total_tokens),
                    cache_creation: None,
                    ..data.message.usage
                },
                ..data.message.clone()
            },
            ..data.clone()
        };
        let cost = calculate_cost(&cost_data, mode, pricing);
        let missing_pricing_model = missing_pricing_model_for_usage(
            Some(&model),
            cost_data.message.usage,
            None,
            mode,
            pricing,
        );
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from("amp"),
            session_id: Arc::from(thread_id),
            project_path: Arc::from("Amp"),
            cost,
            extra_total_tokens,
            credits: json_value_f64(event.credits.as_ref()),
            message_count: None,
            model: Some(model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            data,
        });
    }
    entries
}

fn parse_message_usage(
    messages: &[AmpMessage],
    thread_id: &str,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for message in messages {
        if message.role.as_ref().and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.usage.as_ref() else {
            continue;
        };
        let Some(timestamp_text) = non_empty_json_string(usage.get("timestamp"))
            .or_else(|| non_empty_json_string(message.timestamp.as_ref()))
        else {
            continue;
        };
        let Some(timestamp) = crate::parse_ts_timestamp(&timestamp_text) else {
            continue;
        };
        let Some(model) = non_empty_json_string(usage.get("model"))
            .or_else(|| non_empty_json_string(message.model.as_ref()))
        else {
            continue;
        };
        let usage_raw = TokenUsageRaw {
            input_tokens: json_value_u64(usage.get("inputTokens")),
            output_tokens: json_value_u64(usage.get("outputTokens")),
            cache_creation_input_tokens: json_value_u64(usage.get("cacheCreationInputTokens")),
            cache_read_input_tokens: json_value_u64(usage.get("cacheReadInputTokens")),
            speed: None,
            cache_creation: None,
        };
        let total_tokens = json_value_u64(usage.get("totalTokens"));
        let (usage_raw, extra_total_tokens) =
            apply_total_token_fallback(usage_raw, 0, total_tokens);
        if usage_raw.input_tokens == 0
            && usage_raw.output_tokens == 0
            && usage_raw.cache_creation_input_tokens == 0
            && usage_raw.cache_read_input_tokens == 0
            && extra_total_tokens == 0
        {
            continue;
        }
        let message_id = message.message_id.as_ref().and_then(|id| {
            id.as_i64()
                .map(|v| v.to_string())
                .or_else(|| id.as_str().map(str::to_string))
        });
        let data = UsageEntry {
            session_id: Some(thread_id.to_string()),
            timestamp: timestamp_text,
            version: None,
            message: UsageMessage {
                usage: usage_raw,
                model: Some(model.clone()),
                id: message_id,
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        let cost_data = UsageEntry {
            message: UsageMessage {
                usage: TokenUsageRaw {
                    output_tokens: data
                        .message
                        .usage
                        .output_tokens
                        .saturating_add(extra_total_tokens),
                    cache_creation: None,
                    ..data.message.usage
                },
                ..data.message.clone()
            },
            ..data.clone()
        };
        let cost = calculate_cost(&cost_data, mode, pricing);
        let missing_pricing_model = missing_pricing_model_for_usage(
            Some(&model),
            cost_data.message.usage,
            None,
            mode,
            pricing,
        );
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from("amp"),
            session_id: Arc::from(thread_id),
            project_path: Arc::from("Amp"),
            cost,
            extra_total_tokens,
            credits: json_value_f64(usage.get("credits")),
            message_count: None,
            model: Some(model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            data,
        });
    }
    entries
}

fn cache_tokens_by_message_id(messages: &[AmpMessage]) -> HashMap<i64, (u64, u64)> {
    let mut cache_tokens = HashMap::new();
    for message in messages {
        if message.role.as_ref().and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message_id) = message.message_id.as_ref().and_then(Value::as_i64) else {
            continue;
        };
        let usage = message.usage.as_ref();
        cache_tokens.insert(
            message_id,
            (
                json_value_u64(usage.and_then(|usage| usage.get("cacheCreationInputTokens"))),
                json_value_u64(usage.and_then(|usage| usage.get("cacheReadInputTokens"))),
            ),
        );
    }
    cache_tokens
}

fn json_value_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn falls_back_to_total_tokens_when_amp_parts_are_missing() {
        let fixture = fs_fixture!({
            "thread.json": r#"{"id":"thread-a","usageLedger":{"events":[{"id":"event-a","timestamp":"2026-01-02T00:00:00.000Z","model":"gpt-5","tokens":{"total":345}}]}}"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 345);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn reads_usage_from_messages_when_ledger_is_missing() {
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"T-thread-a",
                "messages":[
                    {"role":"user","content":"hi"},
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":10,
                        "outputTokens":178,
                        "cacheCreationInputTokens":986,
                        "cacheReadInputTokens":11372,
                        "totalInputTokens":12368,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }},
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":5,
                        "outputTokens":42,
                        "cacheCreationInputTokens":0,
                        "cacheReadInputTokens":12000,
                        "timestamp":"2026-01-19T11:43:00.000Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].data.message.usage.output_tokens, 178);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            986
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 11372);
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(entries[0].session_id.as_ref(), "T-thread-a");
        assert_eq!(entries[1].data.message.usage.input_tokens, 5);
    }

    #[test]
    fn ledger_events_take_precedence_over_messages_usage() {
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"thread-a",
                "usageLedger":{"events":[{
                    "id":"event-a",
                    "timestamp":"2026-01-02T00:00:00.000Z",
                    "model":"gpt-5",
                    "tokens":{"input":1,"output":2}
                }]},
                "messages":[
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":99,
                        "outputTokens":99,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.model.as_deref(), Some("gpt-5"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1);
    }

    #[test]
    fn skips_messages_with_no_usage_tokens() {
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"T-thread-a",
                "messages":[
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":0,
                        "outputTokens":0,
                        "cacheCreationInputTokens":0,
                        "cacheReadInputTokens":0,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn falls_back_to_total_tokens_in_messages_path() {
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"T-thread-a",
                "messages":[
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "totalTokens":345,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 345);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn empty_usage_ledger_falls_back_to_message_usage() {
        // A `usageLedger` object without a usable `events` array must not win
        // precedence over `messages`; the thread should still report message
        // usage instead of returning nothing.
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"T-thread-a",
                "usageLedger":{},
                "messages":[
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":10,
                        "outputTokens":178,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].data.message.usage.output_tokens, 178);
    }

    #[test]
    fn malformed_message_element_does_not_drop_the_thread() {
        // A non-object entry in `messages` (and a non-object `usageLedger`) must
        // be skipped gracefully rather than failing the whole thread.
        let fixture = fs_fixture!({
            "thread.json": r#"{
                "id":"T-thread-a",
                "usageLedger":"not-an-object",
                "messages":[
                    "garbage",
                    {"role":"assistant","usage":{
                        "model":"claude-haiku-4-5-20251001",
                        "inputTokens":10,
                        "outputTokens":178,
                        "timestamp":"2026-01-19T11:42:10.652Z"
                    }}
                ]
            }"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread_file(&file, None, CostMode::Auto, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].data.message.usage.output_tokens, 178);
    }
}
