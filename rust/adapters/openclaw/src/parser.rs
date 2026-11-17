use std::{fs, path::Path, sync::Arc};

use ccusage_adapter_common::{jsonl, units::UnitError};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage_at, cli::CostMode, fast::LinePrefilter,
    format_date_tz, missing_pricing_model_for_usage, stored_cost_for_mode,
};

/// A single parsed OpenClaw session line. Only the fields ccusage consumes are
/// declared; serde skips everything else. Both `model_change`/`model-snapshot`
/// records and assistant `message` records share this struct so the stateful
/// model/provider tracking can read either shape in order.
#[derive(Debug, Default, Deserialize)]
pub(super) struct OpenClawLine {
    #[serde(default)]
    r#type: Option<String>,
    // Stable transcript event id. SQLite-era rows always carry one (the insert
    // path generates it when the caller omits it); JSONL-era records predate
    // it, so it stays optional.
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
    // Message failures must not discard the model/provider state on the same
    // record; readers diagnose the rejected field after decoding the envelope.
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
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cost: Option<OpenClawCost>,
}

/// Precomputed cost block carried alongside OpenClaw usage.
#[derive(Debug, Default, Deserialize)]
struct OpenClawCost {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    total: Option<f64>,
}

#[derive(Debug, Clone)]
pub(super) struct OpenClawEntry {
    timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    pricing_model: String,
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
        fallback_timestamp: TimestampMs,
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
        let mut entry = parse_transcript_message(
            record,
            &self.session_id,
            self.model.as_deref(),
            self.provider.as_deref(),
            fallback_timestamp,
        )?;
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
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let mut reader = TranscriptReader::jsonl(extract_session_id(path));
    let fallback_timestamp = file_modified_timestamp(path);
    let content = fs::read(path).map_err(UnitError::from)?;
    let prefilter = record_prefilter();
    Ok(
        jsonl::numbered_records_with_usage_diagnostics::<OpenClawLine>(
            &content,
            Some(&prefilter),
            path,
            diagnostics,
            is_usage_envelope,
        )
        .filter_map(|(number, record)| {
            reader.read(&record, path, number, fallback_timestamp, diagnostics)
        })
        .map(|entry| entry_to_loaded(entry, tz, mode, pricing))
        .collect(),
    )
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
    fallback_timestamp: TimestampMs,
) -> Option<OpenClawEntry> {
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
        pricing_model: model.clone(),
        model: format!("[openclaw] {model}"),
        provider,
        input_tokens: raw_usage.input_tokens,
        output_tokens: raw_usage.output_tokens,
        cache_creation_tokens: raw_usage.cache_creation_input_tokens,
        cache_read_tokens: raw_usage.cache_read_input_tokens,
        total_tokens,
        cost: usage.cost.as_ref().and_then(|cost| cost.total),
        event_id: record.id.clone(),
    })
}

pub(super) fn entry_to_loaded(
    entry: OpenClawEntry,
    tz: &JiffTimeZone,
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
            id: entry.event_id,
        },
        cost_usd: entry.cost,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let pricing_model = if pricing.is_some_and(|pricing| pricing.find_exact(&entry.model).is_some())
    {
        &entry.model
    } else {
        &entry.pricing_model
    };
    let cost = calculate_cost_for_usage_at(
        Some(pricing_model),
        usage,
        entry.cost,
        Some(entry.timestamp),
        mode,
        pricing,
    );
    let missing_pricing_model =
        missing_pricing_model_for_usage(Some(pricing_model), usage, entry.cost, mode, pricing);
    let stored_cost = stored_cost_for_mode(entry.cost, mode);
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

fn timestamp_from_millis(raw: i64) -> Option<TimestampMs> {
    (raw >= 0).then(|| TimestampMs::from_millis(raw))
}

pub(super) fn millis_from_f64(raw: f64) -> Option<i64> {
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

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
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

/// Identity shared by a legacy JSONL event and its migrated SQLite copy.
///
/// SQLite can carry a corrected provider-billed cost, so cost must not prevent
/// the migrated copy from replacing the legacy event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct MigrationId {
    session_id: Arc<str>,
    timestamp: String,
    model: Option<String>,
    tokens: [u64; 5],
}

pub(super) fn migration_id(entry: &LoadedEntry) -> MigrationId {
    let usage = entry.data.message.usage;
    MigrationId {
        session_id: Arc::clone(&entry.session_id),
        timestamp: entry.data.timestamp.clone(),
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
    fn diagnoses_nested_usage_failure_and_skips_lines_without_markers() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"toolResult","content":"\u001b[31mred\u001b[0m C:\\users"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            None,
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
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
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            None,
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

        let entries = parse_session_file(
            &file,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            None,
            &Diagnostics::new("openclaw"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].data.version.as_deref(), Some("openai"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
    }

    #[test]
    fn malformed_message_object_keeps_model_state_and_its_own_diagnostic() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                "{}\n\n",
                r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai","message":{"role":1,"secret":"SECRET_CANARY"}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":20}}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("openclaw");
        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            None,
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].data.version.as_deref(), Some("openai"));
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
            parse_transcript_message(&record, "session-a", None, None, TimestampMs::UNIX_EPOCH)
                .unwrap();

        assert_eq!(entry.output_tokens, 222);
        assert_eq!(entry.total_tokens, 222);
    }

    #[test]
    fn falls_back_for_a_float_timestamp_beyond_the_millisecond_range() {
        let record = serde_json::from_value::<OpenClawLine>(serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "gpt-5.2",
                "usage": { "input": 10, "output": 20 },
                "timestamp": 1e300
            }
        }))
        .unwrap();
        let fallback = TimestampMs::from_millis(1_769_753_935_279);

        let entry = parse_transcript_message(&record, "session-a", None, None, fallback).unwrap();

        assert_eq!(entry.timestamp, fallback);
    }

    #[test]
    fn calculates_decorated_deepseek_models_with_the_raw_pricing_identity() {
        let fixture = fs_fixture!({
            "session.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"deepseek-v4-flash","usage":{"input":1000000,"output":0,"totalTokens":1000000},"timestamp":"2026-08-17T01:00:00Z"}}"#,
        });
        let pricing = PricingMap::load_embedded();

        let entries = parse_session_file(
            &fixture.path("session.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Calculate,
            Some(&pricing),
            &Diagnostics::new("openclaw"),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert!((entries[0].cost - 0.44).abs() < 1e-12);
    }
}
