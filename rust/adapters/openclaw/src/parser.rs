use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::{AgentModel, MatchKind},
};
use std::{path::Path, sync::Arc};

use ccusage_adapter_common::{
    billing::{host_provider, price_entry_for_provider},
    jsonl,
    units::UnitError,
};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, fast::LinePrefilter,
    stored_cost_for_mode,
};

#[derive(Debug, Default, Deserialize)]
pub(super) struct OpenClawLine {
    #[serde(default)]
    r#type: Option<String>,
    // NOTE(2026-09-16, #1727): OpenClaw generates an event ID when inserting a SQLite transcript row without one. Older JSONL records can lack that ID, so the parser accepts both.
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(rename = "customType", default)]
    custom_type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
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
    #[serde(default, deserialize_with = "deserialize_message")]
    message: DecodedMessage,
    timestamp: Option<Value>,
}

#[derive(Debug, Default)]
struct DecodedMessage {
    value: Option<OpenClawMessage>,
    rejected: bool,
    usage_envelope: bool,
}

fn deserialize_message<'de, D>(deserializer: D) -> std::result::Result<DecodedMessage, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        Some(value @ Value::Object(_)) => {
            let usage_envelope = value.get("role").and_then(Value::as_str) == Some("assistant")
                && value.get("usage").is_some();
            Ok(match serde_json::from_value(value) {
                Ok(message) => DecodedMessage {
                    value: Some(message),
                    ..Default::default()
                },
                Err(_) => DecodedMessage {
                    rejected: true,
                    usage_envelope,
                    ..Default::default()
                },
            })
        }
        _ => Ok(DecodedMessage::default()),
    }
}

fn record_message_error(
    record: &OpenClawLine,
    path: &Path,
    number: u32,
    diagnostics: &Diagnostics,
) {
    if !record.message.rejected {
        return;
    }
    let kind = if record.r#type.as_deref() == Some("message") && record.message.usage_envelope {
        SkipKind::InvalidData
    } else {
        SkipKind::UndecodedUsage
    };
    diagnostics.record(Skip::record(
        path,
        number,
        kind,
        "message object did not decode",
    ));
}

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
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cost: Option<OpenClawCost>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenClawCost {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    total: Option<f64>,
}

#[derive(Debug, Clone)]
pub(super) struct OpenClawEntry {
    timestamp: TimestampMs,

    session_id: String,
    model: String,
    provider: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    cost: Option<f64>,
    event_id: Option<String>,
}

pub(super) struct TranscriptReader {
    session_id: String,
    keeps_event_ids: bool,
    model: Option<String>,
    provider: Option<String>,
}

impl TranscriptReader {
    fn jsonl(session_id: String) -> Self {
        Self::new(session_id, false)
    }

    pub(super) fn sqlite(session_id: String) -> Self {
        Self::new(session_id, true)
    }

    fn new(session_id: String, keeps_event_ids: bool) -> Self {
        Self {
            session_id,
            keeps_event_ids,
            model: None,
            provider: None,
        }
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(super) fn read(
        &mut self,
        record: &OpenClawLine,
        unit: &Path,
        number: u32,
        fallback_timestamp: Option<TimestampMs>,
        diagnostics: &Diagnostics,
    ) -> Option<OpenClawEntry> {
        record_message_error(record, unit, number, diagnostics);
        if is_model_change(record) {
            let (model, provider) = model_change_source(record);
            if model.is_some() {
                self.model = model;
            }
            if provider.is_some() {
                self.provider = provider;
            }
            return None;
        }
        let mut entry = match parse_transcript_message(
            record,
            &self.session_id,
            self.model.as_deref(),
            self.provider.as_deref(),
            fallback_timestamp,
        )? {
            Ok(entry) => entry,
            Err(TranscriptIssue::MissingTimestamp) => {
                diagnostics.record(Skip::record(
                    unit,
                    number,
                    SkipKind::InvalidTimestamp,
                    "OpenClaw usage record has no timestamp",
                ));
                return None;
            }
            Err(TranscriptIssue::UnrepresentableTokens) => {
                diagnostics.record(Skip::record(
                    unit,
                    number,
                    SkipKind::InvalidData,
                    "OpenClaw token counts exceed the u64 range",
                ));
                return None;
            }
        };
        if !self.keeps_event_ids {
            entry.event_id = None;
        }
        Some(entry)
    }
}

pub(super) fn parse_session_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let mut reader = TranscriptReader::jsonl(extract_session_id(path));
    let prefilter = record_prefilter();
    let mut entries = Vec::new();
    jsonl::read_records::<OpenClawLine>(
        path,
        Some(&prefilter),
        diagnostics,
        is_usage_envelope,
        |number, record, diagnostics| {
            entries.extend(
                reader
                    .read(&record, path, number, None, diagnostics)
                    .map(|entry| entry_to_loaded(entry, tz, mode, pricing)),
            );
        },
    )?;
    Ok(entries)
}

pub(super) fn record_prefilter() -> LinePrefilter {
    LinePrefilter::any(&[br#""model_change""#, br#""model-snapshot""#, br#""usage""#])
}

pub(super) fn is_usage_envelope(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("message")
        && value
            .get("message")
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
        && value
            .get("message")
            .and_then(|message| message.get("usage"))
            .is_some()
}

fn is_model_change(record: &OpenClawLine) -> bool {
    if record.r#type.as_deref() == Some("model_change") {
        return true;
    }
    record.r#type.as_deref() == Some("custom")
        && record.custom_type.as_deref() == Some("model-snapshot")
}

fn model_change_source(record: &OpenClawLine) -> (Option<String>, Option<String>) {
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
    (source_model_id.or(source_model), source_provider)
}

fn parse_transcript_message(
    record: &OpenClawLine,
    session_id: &str,
    current_model: Option<&str>,
    current_provider: Option<&str>,
    fallback_timestamp: Option<TimestampMs>,
) -> Option<Result<OpenClawEntry, TranscriptIssue>> {
    if record.r#type.as_deref() != Some("message") {
        return None;
    }
    let message = record.message.value.as_ref()?;
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
    let Some(counted_tokens) = ccusage_core::checked_total_usage_tokens(raw_usage, 0) else {
        return Some(Err(TranscriptIssue::UnrepresentableTokens));
    };
    let total_tokens = total_tokens.max(counted_tokens);
    if total_tokens == 0 {
        return None;
    }
    let Some(timestamp) = timestamp_from_value(message.timestamp.as_ref())
        .or_else(|| timestamp_from_value(record.timestamp.as_ref()))
        .or(fallback_timestamp)
    else {
        return Some(Err(TranscriptIssue::MissingTimestamp));
    };
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
    Some(Ok(OpenClawEntry {
        timestamp,

        session_id: session_id.to_string(),
        model,
        provider,
        input_tokens: raw_usage.input_tokens,
        output_tokens: raw_usage.output_tokens,
        cache_creation_tokens: raw_usage.cache_creation_input_tokens,
        cache_read_tokens: raw_usage.cache_read_input_tokens,
        total_tokens,
        cost: usage.cost.as_ref().and_then(|cost| cost.total),
        event_id: record.id.clone(),
    }))
}

pub(super) fn entry_to_loaded(
    entry: OpenClawEntry,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let identity = AgentModel::new("openclaw", &entry.model);
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let additional = ccusage_core::AdditionalTokens::from_total(usage, 0, entry.total_tokens);
    let host_model = entry
        .provider
        .as_deref()
        .and_then(host_provider)
        .map(|host| format!("{host}/{}", entry.model));
    let [display, raw] = identity.candidates();
    let candidates: Vec<_> = std::iter::once(display)
        .chain(host_model.as_deref().map(|name| (name, MatchKind::Exact)))
        .chain(std::iter::once(raw))
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Request,
        usage,
        reasoning_tokens: 0,
        reported_model: Some(identity.display()),
        candidates: &candidates,
        stored_cost: entry.cost,
        timestamp: Some(entry.timestamp),
    };
    let priced = price_entry_for_provider(entry_pricing, entry.provider.as_deref(), pricing);
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| crate::model_aliases::resolve_model_name(&entry.model).into_owned());
    let stored_cost = stored_cost_for_mode(entry.cost, mode);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("openclaw"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("OpenClaw"),
        cost,
        extra_total_tokens: additional.report_remainder,
        credits: None,
        message_count: None,
        model: Some(identity.into_display()),
        usage,
        message_id: entry.event_id,
        version: entry.provider.clone(),
        recorded_cost: entry.cost,
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        stored_cost,
    }
}

fn timestamp_from_value(value: Option<&Value>) -> Option<TimestampMs> {
    let value = value?;
    if let Some(raw) = value.as_i64() {
        return timestamp_from_millis(raw);
    }
    if let Some(raw) = value.as_u64() {
        return timestamp_from_millis(i64::try_from(raw).ok()?);
    }
    if let Some(raw) = value.as_f64() {
        return millis_from_f64(raw).and_then(timestamp_from_millis);
    }
    crate::parse_ts_timestamp(value.as_str()?)
}

// NOTE(2026-09-16, #1727): OpenClaw stores SQLite `created_at` and numeric message timestamps in Unix milliseconds. Both timestamp sources use the same conversion.
pub(super) fn timestamp_from_millis(raw: i64) -> Option<TimestampMs> {
    (raw >= 0)
        .then_some(raw)
        .and_then(TimestampMs::from_instant_millis)
}

fn millis_from_f64(raw: f64) -> Option<i64> {
    (raw.is_finite() && raw.abs() < i64::MAX as f64).then(|| raw.trunc() as i64)
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

#[derive(Debug)]
enum TranscriptIssue {
    MissingTimestamp,
    UnrepresentableTokens,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ContentId {
    migration: MigrationId,
    cost_bits: u64,
}

pub(super) fn content_id(entry: &LoadedEntry) -> ContentId {
    ContentId {
        migration: migration_id(entry),
        cost_bits: entry.cost.to_bits(),
    }
}

// NOTE(2026-09-16, #1727): An OpenClaw migration can correct the billed cost in SQLite. The cost difference must still allow the migrated event to match its JSONL copy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct MigrationId {
    session_id: Arc<str>,
    timestamp: TimestampMs,
    model: Option<String>,
    tokens: [u64; 5],
}

pub(super) fn migration_id(entry: &LoadedEntry) -> MigrationId {
    let usage = entry.usage;
    MigrationId {
        session_id: Arc::clone(&entry.session_id),
        timestamp: entry.timestamp,
        model: entry.model.clone(),
        tokens: [
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
            entry.extra_total_tokens,
        ],
    }
}

#[cfg(test)]
mod tests {

    use ccusage_test_support::fs_fixture;

    use super::*;

    #[test]
    fn diagnoses_jsonl_usage_without_a_timestamp() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":2}}}"#,
        });
        let diagnostics = Diagnostics::new("openclaw");
        let events = parse_session_file(
            &fixture.path("session.jsonl"),
            &JiffTimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &diagnostics,
        )
        .unwrap();
        assert!(events.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
    }

    #[test]
    fn uses_a_valid_envelope_timestamp_when_the_message_timestamp_is_invalid() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00Z","message":{"role":"assistant","timestamp":"invalid","usage":{"input":10,"output":2}}}"#,
        });
        let diagnostics = Diagnostics::new("openclaw");
        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &JiffTimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &diagnostics,
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp.as_millis(), 1_767_312_000_000);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 0);
    }
    #[test]
    fn diagnoses_nested_usage_failure_and_skips_lines_without_markers() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"toolResult","content":"\u001b[31mred\u001b[0m C:\\users"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","timestamp":"2026-01-02T00:00:00Z","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&ccusage_core::cli::SharedArgs {
                    debug: true,
                    ..Default::default()
                })
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }

    #[test]
    fn diagnoses_malformed_usage_line_and_keeps_later_usage() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"message","customType":{"secret":"SECRET_CANARY"},"message":{"role":"assistant","usage":{"input":1}}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20},"timestamp":1769753935279}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        let warnings = diagnostics
            .warnings(&ccusage_core::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn malformed_message_does_not_drop_model_tracking_line() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai","message":"not-an-object"}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","timestamp":"2026-01-02T00:00:00Z","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let file = fixture.path("session.jsonl");

        let entries = parse_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &Diagnostics::new("openclaw"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].version.as_deref(), Some("openai"));
        assert_eq!(entries[0].usage.input_tokens, 10);
    }

    #[test]
    fn malformed_message_object_keeps_model_state_and_its_own_diagnostic() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                "{}\n\n",
                r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai","message":{"role":1,"secret":"SECRET_CANARY"}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","timestamp":"2026-01-02T00:00:00Z","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");
        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].version.as_deref(), Some("openai"));
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::UndecodedUsage),
            1
        );
        let warnings = diagnostics
            .warnings(&ccusage_core::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(warnings.contains("session.jsonl:3:"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn skips_and_diagnoses_token_counts_that_exceed_the_u64_range() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":18446744073709551615,"output":1},"timestamp":"2026-08-17T01:00:00Z"}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":1},"timestamp":"2026-08-17T01:01:00Z"}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn keeps_total_only_usage_out_of_output_tokens() {
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
        let entry = parse_transcript_message(
            &record,
            "session-a",
            None,
            None,
            Some(TimestampMs::UNIX_EPOCH),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.total_tokens, 222);
        let loaded = entry_to_loaded(
            entry,
            &JiffTimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        );
        assert_eq!(loaded.usage.output_tokens, 0);
        assert_eq!(loaded.extra_total_tokens, 222);
    }

    #[test]
    fn falls_back_for_a_timestamp_beyond_the_millisecond_range() {
        let record_value = serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "gpt-5.2",
                "usage": { "input": 10, "output": 20 },
                "timestamp": null
            }
        });
        let fallback = TimestampMs::from_millis(1_769_753_935_279);

        for timestamp in [
            serde_json::json!(1e300),
            serde_json::json!(9e18),
            serde_json::json!(9_000_000_000_000_000_000_i64),
        ] {
            let mut record = record_value.clone();
            record["message"]["timestamp"] = timestamp.clone();
            let record = serde_json::from_value::<OpenClawLine>(record).unwrap();

            let entry = parse_transcript_message(&record, "session-a", None, None, Some(fallback))
                .unwrap()
                .unwrap();

            assert_eq!(entry.timestamp, fallback, "{timestamp}");
        }
    }

    #[test]
    fn calculates_decorated_deepseek_models_with_the_raw_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "session.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"deepseek-v4-flash","usage":{"input":1000000,"output":0,"totalTokens":1000000},"timestamp":"2026-08-17T01:00:00Z"}}"#,
        });
        let pricing = ccusage_core::PricingMap::load_embedded();

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            &Diagnostics::new("openclaw"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert_eq!(
            entries[0].model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert!((entries[0].cost - 0.44).abs() < 1e-12);
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = ccusage_test_support::fs_fixture!({
            "s.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"deepseek.v4.flash","usage":{"input":1000,"output":0,"totalTokens":1000},"timestamp":"2026-11-17T10:00:00Z"}}"#
        });
        let pricing = ccusage_core::PricingMap::load_embedded();
        let rows = parse_session_file(
            &fixture.path("s.jsonl"),
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            &Diagnostics::new("openclaw"),
        )
        .unwrap();
        assert_eq!(
            rows[0].model.as_deref(),
            Some("[openclaw] deepseek.v4.flash")
        );
        assert!((rows[0].cost - 0.00015).abs() < 1e-12);
    }

    #[test]
    fn prices_a_foreign_router_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let fixture = ccusage_test_support::fs_fixture!({
            "router.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"deepseek-v4-flash","provider":"openrouter","usage":{"input":1000,"output":1000,"totalTokens":2000},"timestamp":"2026-11-17T10:00:00Z"}}"#,
            "deepseek.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"deepseek-v4-flash","provider":"deepseek","usage":{"input":1000,"output":1000,"totalTokens":2000},"timestamp":"2026-11-17T10:00:00Z"}}"#
        });
        for (name, expected) in [("router.jsonl", 0.0015), ("deepseek.jsonl", 0.00075)] {
            let rows = parse_session_file(
                &fixture.path(name),
                &JiffTimeZone::UTC,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing),
                &Diagnostics::new("openclaw"),
            )
            .unwrap();
            assert_eq!(rows.len(), 1, "{name}");
            assert!(
                (rows[0].cost - expected).abs() < 1e-12,
                "{name}: {}",
                rows[0].cost
            );
            assert_eq!(rows[0].missing_pricing_model, None, "{name}");
        }
    }

    #[test]
    fn a_vertex_provider_bills_the_vertex_listing_in_every_spelling() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "gemini/gemini-4-argon": {"litellm_provider": "gemini", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5},
                "vertex_ai/gemini-4-argon": {"litellm_provider": "vertex_ai-language-models", "input_cost_per_token": 2.5e-6, "output_cost_per_token": 1.2e-5}
            }"#,
        );
        for logged in [
            "vertex",
            "vertex_ai",
            "Vertex-AI",
            "google-vertex",
            "google_vertex",
        ] {
            let entry = OpenClawEntry {
                timestamp: ccusage_core::parse_ts_timestamp("2026-11-02T00:00:00Z").unwrap(),
                session_id: "session".to_string(),
                model: "gemini-4-argon".to_string(),
                provider: Some(logged.to_string()),
                input_tokens: 1_000_000,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                total_tokens: 1_000_000,
                cost: None,
                event_id: None,
            };
            let loaded = entry_to_loaded(
                entry,
                &JiffTimeZone::UTC,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing),
            );
            assert!(
                (loaded.cost - 2.5).abs() < 1e-9,
                "{logged}: {}",
                loaded.cost
            );
        }
    }
}
