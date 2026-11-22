use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{collections::HashMap, path::Path, sync::Arc};

use ccusage_adapter_common::{
    jsonl,
    units::{UnitError, read_json},
};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{Diagnostics, LoadedEntry, Skip, SkipKind, TimestampMs, TokenUsageRaw, date_at};
use ccusage_core::AdditionalTokens;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpThread {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_vec")]
    messages: Vec<AmpMessage>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage_ledger: Option<AmpUsageLedger>,
}

#[derive(Debug, Default, Deserialize)]
struct AmpUsageLedger {
    #[serde(default, deserialize_with = "jsonl::lenient_array")]
    events: Option<Vec<AmpLedgerEvent>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpLedgerEvent {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    tokens: Option<AmpLedgerTokens>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    to_message_id: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    credits: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct AmpLedgerTokens {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpMessage {
    #[serde(rename = "role", default, deserialize_with = "is_assistant_role")]
    from_assistant: bool,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<AmpMessageUsage>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "message_id")]
    message_id: Option<AmpMessageId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmpMessageUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    credits: Option<f64>,
}

#[derive(Debug)]
enum AmpMessageId {
    Number(i64),
    Text(String),
}

impl AmpMessageId {
    fn number(&self) -> Option<i64> {
        match self {
            Self::Number(number) => Some(*number),
            Self::Text(_) => None,
        }
    }

    fn to_text(&self) -> String {
        match self {
            Self::Number(number) => number.to_string(),
            Self::Text(text) => text.clone(),
        }
    }
}

fn message_id<'de, D>(deserializer: D) -> std::result::Result<Option<AmpMessageId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::Number(number)) => number.as_i64().map(AmpMessageId::Number),
        Some(Value::String(text)) => Some(AmpMessageId::Text(text)),
        _ => None,
    })
}

fn is_assistant_role<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?
        .as_ref()
        .and_then(Value::as_str)
        == Some("assistant"))
}

struct AmpUsage {
    usage: TokenUsageRaw,
    additional: AdditionalTokens,
    model: String,
    timestamp: TimestampMs,
    id: Option<String>,
    credits: Option<f64>,
}

fn counted_usage(
    usage: TokenUsageRaw,
    total_tokens: u64,
) -> Option<(TokenUsageRaw, AdditionalTokens)> {
    let additional = AdditionalTokens::from_total(usage, 0, total_tokens);
    let counts_tokens = usage.input_tokens != 0
        || usage.output_tokens != 0
        || usage.cache_creation_input_tokens != 0
        || usage.cache_read_input_tokens != 0
        || additional.report_remainder != 0;
    counts_tokens.then_some((usage, additional))
}

struct ThreadUnit<'a> {
    path: &'a Path,
    diagnostics: &'a Diagnostics,
}

impl ThreadUnit<'_> {
    fn invalid_timestamp(&self, index: usize) {
        self.diagnostics.record(Skip::record(
            self.path,
            Skip::line_number(index),
            SkipKind::InvalidTimestamp,
            "missing or unsupported timestamp",
        ));
    }

    fn missing_model(&self, index: usize) {
        self.diagnostics.record(Skip::record(
            self.path,
            Skip::line_number(index),
            SkipKind::InvalidData,
            "Amp usage record has no model",
        ));
    }
}

pub(crate) fn read_thread(
    path: &Path,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let thread = read_json(path)?;
    parse_thread(thread, &ThreadUnit { path, diagnostics }, tz, pricing)
}

fn parse_thread(
    thread: AmpThread,
    unit: &ThreadUnit<'_>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let messages = &thread.messages;
    // NOTE(2026-05-25, #1145): Newer Amp threads store usage on assistant messages without a `usageLedger`. Older ledgers and their corresponding message usage describe the same calls.
    let ledger_events = thread
        .usage_ledger
        .as_ref()
        .and_then(|ledger| ledger.events.as_ref());
    let Some(thread_id) = thread.id else {
        let counts_usage = match ledger_events {
            Some(events) => {
                let cache_tokens = cache_tokens_by_message_id(messages);
                events
                    .iter()
                    .any(|event| ledger_event_usage(event, &cache_tokens).is_some())
            }
            None => messages
                .iter()
                .any(|message| message_usage(message).is_some()),
        };
        if counts_usage {
            return Err(UnitError::new(
                SkipKind::InvalidData,
                "Amp usage thread has no id",
            ));
        }
        return Ok(Vec::new());
    };

    if let Some(events) = ledger_events {
        let cache_tokens = cache_tokens_by_message_id(messages);
        return Ok(parse_ledger_events(
            events,
            &cache_tokens,
            &thread_id,
            unit,
            tz,
            pricing,
        ));
    }

    Ok(parse_message_usage(messages, &thread_id, unit, tz, pricing))
}

fn ledger_event_usage(
    event: &AmpLedgerEvent,
    cache_tokens: &HashMap<i64, (u64, u64)>,
) -> Option<(TokenUsageRaw, AdditionalTokens)> {
    let tokens = event.tokens.as_ref()?;
    let (cache_creation, cache_read) = event
        .to_message_id
        .and_then(|id| cache_tokens.get(&id).copied())
        .unwrap_or_default();
    let usage = TokenUsageRaw {
        input_tokens: tokens.input,
        output_tokens: tokens.output,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    };
    counted_usage(usage, tokens.total)
}

fn message_usage(
    message: &AmpMessage,
) -> Option<(&AmpMessageUsage, TokenUsageRaw, AdditionalTokens)> {
    if !message.from_assistant {
        return None;
    }
    let record = message.usage.as_ref()?;
    let usage = TokenUsageRaw {
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_creation_input_tokens: record.cache_creation_input_tokens,
        cache_read_input_tokens: record.cache_read_input_tokens,
        speed: None,
        cache_creation: None,
    };
    let (usage, additional) = counted_usage(usage, record.total_tokens)?;
    Some((record, usage, additional))
}

fn parse_ledger_events(
    events: &[AmpLedgerEvent],
    cache_tokens: &HashMap<i64, (u64, u64)>,
    thread_id: &str,
    unit: &ThreadUnit<'_>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let Some((usage, additional)) = ledger_event_usage(event, cache_tokens) else {
            continue;
        };
        let Some(model) = event.model.clone() else {
            unit.missing_model(index);
            continue;
        };
        let Some(timestamp) = event
            .timestamp
            .as_deref()
            .and_then(crate::parse_ts_timestamp)
        else {
            unit.invalid_timestamp(index);
            continue;
        };
        entries.push(amp_entry(
            AmpUsage {
                usage,
                additional,
                model,
                timestamp,
                id: event.id.clone(),
                credits: event.credits,
            },
            thread_id,
            tz,
            pricing,
        ));
    }
    entries
}

fn parse_message_usage(
    messages: &[AmpMessage],
    thread_id: &str,
    unit: &ThreadUnit<'_>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let Some((record, usage, additional)) = message_usage(message) else {
            continue;
        };
        let Some(model) = record.model.clone().or_else(|| message.model.clone()) else {
            unit.missing_model(index);
            continue;
        };
        let timestamp = record
            .timestamp
            .as_deref()
            .and_then(crate::parse_ts_timestamp)
            .or_else(|| {
                message
                    .timestamp
                    .as_deref()
                    .and_then(crate::parse_ts_timestamp)
            });
        let Some(timestamp) = timestamp else {
            unit.invalid_timestamp(index);
            continue;
        };
        entries.push(amp_entry(
            AmpUsage {
                usage,
                additional,
                model,
                timestamp,
                id: message.message_id.as_ref().map(AmpMessageId::to_text),
                credits: record.credits,
            },
            thread_id,
            tz,
            pricing,
        ));
    }
    entries
}

fn amp_entry(
    record: AmpUsage,
    thread_id: &str,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let candidates = [(record.model.as_str(), MatchKind::Fuzzy)];
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage: TokenUsageRaw {
                cache_creation: None,
                ..record.usage
            },
            reasoning_tokens: 0,
            reported_model: Some(&record.model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(record.timestamp),
        },
        pricing,
    );
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| crate::model_aliases::resolve_model_name(&record.model).into_owned());
    LoadedEntry {
        date: date_at(record.timestamp, tz),
        timestamp: record.timestamp,
        project: Arc::from("amp"),
        session_id: Arc::from(thread_id),
        project_path: Arc::from("Amp"),
        cost,
        extra_total_tokens: record.additional.total(),
        credits: record.credits,
        message_count: None,
        model: Some(record.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        usage: record.usage,
        message_id: record.id,
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

fn cache_tokens_by_message_id(messages: &[AmpMessage]) -> HashMap<i64, (u64, u64)> {
    messages
        .iter()
        .filter(|message| message.from_assistant)
        .filter_map(|message| {
            let id = message.message_id.as_ref()?.number()?;
            let usage = message.usage.as_ref();
            Some((
                id,
                (
                    usage.map_or(0, |usage| usage.cache_creation_input_tokens),
                    usage.map_or(0, |usage| usage.cache_read_input_tokens),
                ),
            ))
        })
        .collect()
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
        parse_thread(
            ccusage_adapter_common::units::decode_json(content)?,
            &unit,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
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
    fn reads_a_thread_without_id_whose_usage_is_not_an_object_as_empty() {
        let diagnostics = Diagnostics::new("amp");
        let content =
            br#"{"messages":[{"role":"assistant","model":"gpt-5","usage":"SECRET_CANARY"}]}"#;

        assert!(parse(content, &diagnostics).unwrap().is_empty());
    }

    #[test]
    fn reads_a_thread_without_id_whose_records_hold_no_counted_usage_as_empty() {
        let diagnostics = Diagnostics::new("amp");
        for content in [
            r#"{"usageLedger":{"events":[{}]}}"#,
            r#"{"usageLedger":{"events":[{"tokens":"SECRET_CANARY"}]}}"#,
            r#"{"messages":[{"role":"user","usage":{"inputTokens":5}}]}"#,
            r#"{"messages":[{"role":"assistant","model":"gpt-5","usage":{"inputTokens":0}}]}"#,
        ] {
            assert!(
                parse(content.as_bytes(), &diagnostics).unwrap().is_empty(),
                "{content}"
            );
        }
    }

    #[test]
    fn uses_message_timestamp_when_usage_timestamp_is_invalid() {
        let diagnostics = Diagnostics::new("amp");
        let content = br#"{"id":"thread","messages":[{"role":"assistant","model":"gpt-5","timestamp":"2026-01-31T11:42:10Z","usage":{"inputTokens":10,"timestamp":"SECRET_CANARY"}}]}"#;

        let entries = parse(content, &diagnostics).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].timestamp,
            crate::parse_ts_timestamp("2026-01-31T11:42:10Z").unwrap()
        );
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
            |file| {
                read_thread(
                    file,
                    &jiff::tz::TimeZone::UTC,
                    ccusage_core::cost::PriceTable::for_map(
                        ccusage_core::cli::CostMode::Auto,
                        &ccusage_core::PricingMap::default(),
                    ),
                    &diagnostics,
                )
            },
        );

        assert!(loaded[0].is_none());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn loads_amp_thread_usage_events() {
        let fixture = fs_fixture!({
            "threads/thread.json": r#"{"id":"thread-a","messages":[{"role":"assistant","messageId":2,"usage":{"cacheCreationInputTokens":20,"cacheReadInputTokens":10}}],"usageLedger":{"events":[{"id":"event-a","timestamp":"2026-05-01T01:02:03.000Z","model":"claude-sonnet-4-20250514","credits":1.25,"tokens":{"input":100,"output":50},"toMessageId":2}]}}"#,
        });

        let entries = read_thread(
            &fixture.path("threads/thread.json"),
            &ccusage_core::resolve_tz(Some(&ccusage_core::cli::ReportTimezone::UTC)),
            ccusage_core::cost::PriceTable::Display,
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2026-05-01");
        assert_eq!(entries[0].session_id.as_ref(), "thread-a");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].usage.input_tokens, 100);
        assert_eq!(entries[0].usage.output_tokens, 50);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].credits, Some(1.25));
    }

    #[test]
    fn keeps_total_only_usage_out_of_output_tokens() {
        let fixture = fs_fixture!({
            "thread.json": r#"{"id":"thread-a","usageLedger":{"events":[{"id":"event-a","timestamp":"2026-01-02T00:00:00.000Z","model":"gpt-5","tokens":{"total":345}}]}}"#,
        });
        let file = fixture.path("thread.json");

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.output_tokens, 0);
        assert_eq!(entries[0].extra_total_tokens, 345);
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[0].usage.output_tokens, 178);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 986);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 11372);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(entries[0].session_id.as_ref(), "T-thread-a");
        assert_eq!(entries[1].usage.input_tokens, 5);
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(entries[0].usage.input_tokens, 1);
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn keeps_total_only_message_usage_out_of_output_tokens() {
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.output_tokens, 0);
        assert_eq!(entries[0].extra_total_tokens, 345);
    }

    #[test]
    fn empty_usage_ledger_falls_back_to_message_usage() {
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[0].usage.output_tokens, 178);
    }

    #[test]
    fn malformed_message_element_does_not_drop_the_thread() {
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

        let entries = read_thread(
            &file,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
            &Diagnostics::new("amp"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[0].usage.output_tokens, 178);
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let fixture = ccusage_test_support::fs_fixture!({
            "thread.json": r#"{"id":"t","messages":[{"role":"assistant","timestamp":"2026-11-17T10:00:00Z","model":"gpt-5.2","usage":{"inputTokens":1000}}]}"#
        });
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let rows = read_thread(
            &fixture.path("thread.json"),
            &JiffTimeZone::UTC,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
            &Diagnostics::new("amp"),
        )
        .unwrap();
        assert_eq!(rows[0].model.as_deref(), Some("gpt-5.2"));
        assert!((rows[0].cost - 0.00175).abs() < 1e-12);
    }
}
