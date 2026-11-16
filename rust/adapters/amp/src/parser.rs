use std::{collections::HashMap, fs, path::Path, sync::Arc};

use ccusage_adapter_common::units::UnitError;
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    Diagnostics, LoadedEntry, PricingMap, Result, Skip, SkipKind, TimestampMs, TokenUsageRaw,
    UsageEntry, UsageMessage, apply_total_token_fallback, calculate_cost, cli::CostMode,
    format_date_tz, json_value_u64, missing_pricing_model_for_usage, non_empty_json_string,
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

/// Where a thread's records come from and where the ones that cannot be
/// counted are recorded.
struct ThreadUnit<'a> {
    path: &'a Path,
    diagnostics: &'a Diagnostics,
}

impl ThreadUnit<'_> {
    /// Parses a record's timestamp, recording a record whose timestamp the
    /// report cannot place on a day. `record` is its 1-based position.
    fn timestamp(&self, record: usize, text: Option<&str>) -> Option<TimestampMs> {
        let timestamp = text.and_then(crate::parse_ts_timestamp);
        if timestamp.is_none() {
            self.diagnostics.record(Skip::record(
                self.path,
                u32::try_from(record).unwrap_or(u32::MAX),
                SkipKind::InvalidTimestamp,
                "missing or unsupported timestamp",
            ));
        }
        timestamp
    }
}

/// Reads one thread file without reporting what it skipped; the loader goes
/// through [`read_thread`].
pub fn read_thread_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    read_thread(path, tz, mode, pricing, &Diagnostics::new("amp"))
        .map_err(|error| crate::cli_error(error.to_string()))
}

pub(crate) fn read_thread(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let content = fs::read(path)?;
    parse_thread(
        &content,
        &ThreadUnit { path, diagnostics },
        tz,
        mode,
        pricing,
    )
}

/// Fails when the content is not a thread object at all, so the caller skips
/// the file as a unit instead of taking it for a thread without usage.
fn parse_thread(
    content: &[u8],
    unit: &ThreadUnit<'_>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let thread = serde_json::from_slice::<AmpThread>(content)?;
    let Some(thread_id) = thread.id else {
        if thread
            .messages
            .iter()
            .any(|message| message.usage.is_some())
            || thread
                .usage_ledger
                .as_ref()
                .and_then(|ledger| ledger.events.as_ref())
                .is_some_and(|events| !events.is_empty())
        {
            return Err(UnitError::new(
                SkipKind::InvalidData,
                "Amp usage thread has no id",
            ));
        }
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
            unit,
            tz,
            mode,
            pricing,
        ));
    }

    Ok(parse_message_usage(
        messages, &thread_id, unit, tz, mode, pricing,
    ))
}

fn parse_ledger_events(
    events: &[AmpLedgerEvent],
    cache_tokens: &HashMap<i64, (u64, u64)>,
    thread_id: &str,
    unit: &ThreadUnit<'_>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for (index, event) in events.iter().enumerate() {
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
        let Some(model) = event.model.clone() else {
            unit.diagnostics.record(Skip::record(
                unit.path,
                u32::try_from(index + 1).unwrap_or(u32::MAX),
                SkipKind::InvalidData,
                "Amp usage record has no model",
            ));
            continue;
        };
        let Some(timestamp) = unit.timestamp(index + 1, event.timestamp.as_deref()) else {
            continue;
        };
        let timestamp_text = event.timestamp.clone().unwrap();
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
            stored_cost: None,
        });
    }
    entries
}

fn parse_message_usage(
    messages: &[AmpMessage],
    thread_id: &str,
    unit: &ThreadUnit<'_>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role.as_ref().and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.usage.as_ref() else {
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
        let Some(model) = non_empty_json_string(usage.get("model"))
            .or_else(|| non_empty_json_string(message.model.as_ref()))
        else {
            unit.diagnostics.record(Skip::record(
                unit.path,
                u32::try_from(index + 1).unwrap_or(u32::MAX),
                SkipKind::InvalidData,
                "Amp usage record has no model",
            ));
            continue;
        };
        let parsed_timestamp =
            |text: String| crate::parse_ts_timestamp(&text).map(|timestamp| (text, timestamp));
        let timestamp = non_empty_json_string(usage.get("timestamp"))
            .and_then(parsed_timestamp)
            .or_else(|| {
                non_empty_json_string(message.timestamp.as_ref()).and_then(parsed_timestamp)
            });
        let Some((timestamp_text, timestamp)) = timestamp else {
            unit.timestamp(index + 1, None);
            continue;
        };
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
            stored_cost: None,
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
    use ccusage_test_support::{fs_fixture, mutation::assert_parser_survives_mutations};

    const LEDGER_THREAD: &str = r#"{"id":"T-ledger","usageLedger":{"events":[
{"id":"e1","timestamp":"2026-01-30T23:30:00.000Z","model":"claude-sonnet-4-20250514","tokens":{"input":1200,"output":340},"toMessageId":1,"credits":1.25},
{"id":"e2","timestamp":"2026-01-31T10:00:00+09:00","model":"gpt-5","tokens":{"total":1500},"toMessageId":2}
]},"messages":[
{"role":"user","content":"hi"},
{"role":"assistant","messageId":1,"usage":{"cacheCreationInputTokens":500,"cacheReadInputTokens":7000}},
{"role":"assistant","messageId":2,"usage":{"cacheReadInputTokens":12000}}
]}"#;

    const MESSAGES_THREAD: &str = r#"{"id":"T-messages","messages":[
{"role":"assistant","messageId":10,"timestamp":"2026-01-31T11:42:10.652Z","model":"gpt-5","usage":{"inputTokens":10,"outputTokens":178,"cacheCreationInputTokens":986,"cacheReadInputTokens":11372,"credits":0.75}},
{"role":"assistant","messageId":"m11","usage":{"model":"claude-haiku-4-5-20251001","totalTokens":345,"timestamp":"2026-02-01T23:59:59Z"}}
]}"#;

    fn parse(
        content: &[u8],
        diagnostics: &Diagnostics,
    ) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
        let unit = ThreadUnit {
            path: Path::new("/threads/T.json"),
            diagnostics,
        };
        parse_thread(content, &unit, None, CostMode::Auto, None)
    }

    #[test]
    fn returns_on_every_damaged_thread() {
        let diagnostics = Diagnostics::new("amp");
        for fixture in [LEDGER_THREAD, MESSAGES_THREAD] {
            assert_eq!(parse(fixture.as_bytes(), &diagnostics).unwrap().len(), 2);
            assert_parser_survives_mutations(fixture.as_bytes(), |content| {
                let _ = parse(content, &diagnostics);
            });
        }
    }

    #[test]
    fn records_usage_whose_timestamp_cannot_be_placed() {
        let diagnostics = Diagnostics::new("amp");
        // Microsecond precision is a shape the timestamp parser does not read.
        let ledger = LEDGER_THREAD.replace("23:30:00.000Z", "23:30:00.000000Z");
        let messages = MESSAGES_THREAD.replace("2026-02-01T23:59:59Z", "yesterday");

        assert_eq!(parse(ledger.as_bytes(), &diagnostics).unwrap().len(), 1);
        assert_eq!(parse(messages.as_bytes(), &diagnostics).unwrap().len(), 1);

        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    }

    #[test]
    fn records_usage_without_a_timestamp_and_keeps_the_healthy_record() {
        for content in [
            LEDGER_THREAD.replace("\"timestamp\":\"2026-01-30T23:30:00.000Z\",", ""),
            MESSAGES_THREAD.replace("\"timestamp\":\"2026-01-31T11:42:10.652Z\",", ""),
        ] {
            let diagnostics = Diagnostics::new("amp");
            assert_eq!(parse(content.as_bytes(), &diagnostics).unwrap().len(), 1);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
        }
    }

    #[test]
    fn diagnoses_usage_without_an_identifier_or_model() {
        let diagnostics = Diagnostics::new("amp");
        let no_id = MESSAGES_THREAD.replace("\"id\":\"T-messages\",", "");
        assert!(parse(no_id.as_bytes(), &diagnostics).is_err());

        let no_model = MESSAGES_THREAD.replace("\"model\":\"gpt-5\",", "");
        assert_eq!(parse(no_model.as_bytes(), &diagnostics).unwrap().len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn uses_message_timestamp_when_usage_timestamp_is_invalid() {
        let diagnostics = Diagnostics::new("amp");
        let content = br#"{"id":"thread","messages":[{"role":"assistant","model":"gpt-5","timestamp":"2026-01-31T11:42:10Z","usage":{"inputTokens":10,"timestamp":"SECRET_CANARY"}}]}"#;

        let entries = parse(content, &diagnostics).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.timestamp, "2026-01-31T11:42:10Z");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 0);
    }

    #[test]
    fn fails_content_that_is_not_a_thread() {
        let diagnostics = Diagnostics::new("amp");

        assert!(parse(&LEDGER_THREAD.as_bytes()[..80], &diagnostics).is_err());
        assert!(parse(b"7", &diagnostics).is_err());
        assert!(
            parse(br#"{"messages":[]}"#, &diagnostics)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_thread_file_not_written_yet_is_not_reported_as_skipped() {
        let fixture = fs_fixture!({"threads/T-new.json": ""});
        let diagnostics = Diagnostics::new("amp");

        let loaded = ccusage_adapter_common::read_units(
            &[fixture.path("threads/T-new.json")],
            true,
            &diagnostics,
            |file| read_thread(file, None, CostMode::Auto, None, &diagnostics),
        );

        assert!(loaded[0].is_none());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

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
