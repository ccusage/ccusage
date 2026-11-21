use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{collections::HashMap, fmt, path::Path, sync::Arc};

use ccusage_adapter_common::{jsonl, units::UnitError};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::{
    Deserialize,
    de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer},
};
use serde_json::{Map, Value};

use crate::{LoadedEntry, TimestampMs, TokenUsageRaw, date_at};
use ccusage_core::AdditionalTokens;

const DEFAULT_MODEL: &str = "unknown";
const PROVIDER_PREFIXES: [&str; 4] = ["google", "gemini", "vertex_ai", "openrouter/google"];

const INPUT_KEYS: &[&str] = &["input", "prompt", "input_tokens", "prompt_tokens"];
const OUTPUT_KEYS: &[&str] = &["output", "candidates", "output_tokens", "candidates_tokens"];
const CACHED_KEYS: &[&str] = &["cached", "cached_tokens"];
const THOUGHTS_KEYS: &[&str] = &[
    "thoughts",
    "reasoning",
    "thoughts_tokens",
    "reasoning_tokens",
];
const TOOL_KEYS: &[&str] = &["tool", "tool_tokens"];
const TOTAL_KEYS: &[&str] = &["total", "total_tokens"];
const TOKEN_KEYS: [&[&str]; 6] = [
    INPUT_KEYS,
    OUTPUT_KEYS,
    CACHED_KEYS,
    THOUGHTS_KEYS,
    TOOL_KEYS,
    TOTAL_KEYS,
];

#[derive(Debug, Default, Deserialize)]
struct GeminiRecord {
    #[serde(default, deserialize_with = "lenient_untrimmed_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    #[serde(rename = "sessionId")]
    session_id_camel: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "lenient_untrimmed_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "lenient_untrimmed_string")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient_untrimmed_string")]
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    #[serde(default, deserialize_with = "lenient_untrimmed_string")]
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_array")]
    messages: Option<Vec<GeminiRecord>>,
    // NOTE(2026-05-19, #1021): Gemini token blocks accept multiple aliases and can contain floating-point counts. Raw values let the reader choose the first numeric alias and truncate fractional counts.
    tokens: Option<Value>,
    stats: Option<Value>,
    result: Option<Value>,
}

fn lenient_untrimmed_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value
        .as_ref()
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

impl GeminiRecord {
    fn session_id(&self) -> Option<String> {
        self.session_id_camel
            .clone()
            .or_else(|| self.session_id.clone())
    }

    fn stats(&self) -> Option<&Value> {
        self.stats
            .as_ref()
            .or_else(|| self.result.as_ref().and_then(|result| result.get("stats")))
    }
}

// NOTE(2026-11-16): Gemini CLI also writes `logs.json` prompt arrays beside session documents. A valid array from discovery carries no chat usage.
struct GeminiDocument(Option<GeminiRecord>);

impl<'de> Deserialize<'de> for GeminiDocument {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DocumentVisitor;

        impl<'de> Visitor<'de> for DocumentVisitor {
            type Value = GeminiDocument;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON document")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                GeminiRecord::deserialize(MapAccessDeserializer::new(map))
                    .map(|record| GeminiDocument(Some(record)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(GeminiDocument(None))
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }

            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }

            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }

            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(GeminiDocument(None))
            }
        }

        deserializer.deserialize_any(DocumentVisitor)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GeminiTokens {
    input: u64,
    output: u64,
    cached: u64,
    thoughts: u64,
    tool: u64,
    total: Option<u64>,
}

impl GeminiTokens {
    fn component_total(self) -> Option<u64> {
        self.input
            .checked_add(self.output)?
            .checked_add(self.cached)?
            .checked_add(self.thoughts)?
            .checked_add(self.tool)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TokenRejection {
    Missing,
    Unrepresentable,
}

#[derive(Debug, Clone)]
pub(super) struct GeminiUsageEvent {
    pub(super) timestamp: TimestampMs,

    session_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
    message_id: Option<String>,
}

#[derive(Debug)]
struct GeminiUsage {
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
    message_id: Option<String>,
}

impl GeminiUsage {
    fn at(self, session_id: &str, timestamp: TimestampMs) -> GeminiUsageEvent {
        GeminiUsageEvent {
            timestamp,

            session_id: session_id.to_string(),
            model: self.model,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            reasoning_tokens: self.reasoning_tokens,
            total_tokens: self.total_tokens,
            message_id: self.message_id,
        }
    }
}

pub(super) fn parse_json_file(
    path: &Path,
    diagnostics: &ccusage_core::Diagnostics,
) -> std::result::Result<Vec<GeminiUsageEvent>, UnitError> {
    let fallback_timestamp = ccusage_adapter_common::timestamps::file_mtime(path);
    let GeminiDocument(Some(record)) =
        ccusage_adapter_common::units::read_json::<GeminiDocument>(path)?
    else {
        return Ok(Vec::new());
    };
    let session_id = record.session_id().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string()
    });
    let session_timestamp = record
        .start_time
        .as_deref()
        .and_then(crate::parse_ts_timestamp)
        .or_else(|| {
            record
                .last_updated
                .as_deref()
                .and_then(crate::parse_ts_timestamp)
        })
        .or(fallback_timestamp);
    if let Some(messages) = &record.messages {
        return Ok(messages
            .iter()
            .filter(|message| message.r#type.as_deref() == Some("gemini"))
            .filter_map(|message| {
                parse_direct_event_record(
                    message,
                    None,
                    &session_id,
                    &GeminiRecordTime {
                        fallback: session_timestamp,
                        path,
                        line: None,
                        diagnostics,
                    },
                )
            })
            .collect());
    }
    if record.r#type.as_deref() == Some("gemini") {
        return Ok(parse_direct_event_record(
            &record,
            None,
            &session_id,
            &GeminiRecordTime {
                fallback: fallback_timestamp,
                path,
                line: None,
                diagnostics,
            },
        )
        .into_iter()
        .collect());
    }
    Ok(parse_stats_events(
        &record,
        record.model.as_deref(),
        &session_id,
        &GeminiRecordTime {
            fallback: fallback_timestamp,
            path,
            line: None,
            diagnostics,
        },
    ))
}

pub(super) fn parse_jsonl_file(
    path: &Path,
    diagnostics: &ccusage_core::Diagnostics,
) -> std::result::Result<Vec<GeminiUsageEvent>, UnitError> {
    let mut session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut current_model = None::<String>;
    let mut events = Vec::new();
    let mut direct_event_indexes = HashMap::<String, usize>::new();
    jsonl::read_records::<GeminiRecord>(
        path,
        None,
        diagnostics,
        is_usage_envelope,
        |number, record, diagnostics| {
            if let Some(value) = record.session_id() {
                session_id = value;
            }
            if let Some(model) = record.model.clone() {
                current_model = Some(model);
            }
            if record.r#type.as_deref() == Some("gemini") {
                let Some(event) = parse_direct_event_record(
                    &record,
                    current_model.as_deref(),
                    &session_id,
                    &GeminiRecordTime {
                        fallback: None,
                        path,
                        line: Some(number),
                        diagnostics,
                    },
                ) else {
                    return;
                };
                if let Some(id) = record.id.clone() {
                    if let Some(index) = direct_event_indexes.get(&id).copied() {
                        events[index] = event;
                    } else {
                        direct_event_indexes.insert(id, events.len());
                        events.push(event);
                    }
                } else {
                    events.push(event);
                }
                return;
            }
            events.extend(parse_stats_events(
                &record,
                current_model.as_deref(),
                &session_id,
                &GeminiRecordTime {
                    fallback: None,
                    path,
                    line: Some(number),
                    diagnostics,
                },
            ));
        },
    )?;
    Ok(events)
}

fn is_usage_envelope(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) == Some("gemini") {
        return true;
    }

    let stats = value
        .get("stats")
        .or_else(|| value.get("result").and_then(|result| result.get("stats")));
    let Some(stats) = stats.and_then(Value::as_object) else {
        return false;
    };
    let has_token_count = |tokens: &Map<String, Value>| {
        TOKEN_KEYS
            .iter()
            .flat_map(|keys| keys.iter())
            .any(|key| tokens.contains_key(*key))
    };
    has_token_count(stats)
        || stats
            .get("models")
            .and_then(Value::as_object)
            .is_some_and(|models| {
                models
                    .values()
                    .filter_map(|model| model.get("tokens").and_then(Value::as_object))
                    .any(&has_token_count)
            })
}

struct GeminiRecordTime<'a> {
    fallback: Option<TimestampMs>,
    path: &'a Path,
    line: Option<u32>,
    diagnostics: &'a ccusage_core::Diagnostics,
}

impl GeminiRecordTime<'_> {
    fn resolve(&self, record: &GeminiRecord) -> Option<TimestampMs> {
        let timestamp = record
            .timestamp
            .as_deref()
            .and_then(crate::parse_ts_timestamp)
            .or_else(|| {
                record
                    .created_at
                    .as_deref()
                    .and_then(crate::parse_ts_timestamp)
            })
            .or(self.fallback);
        if timestamp.is_none() {
            self.record(
                ccusage_core::SkipKind::InvalidTimestamp,
                "Gemini usage has no valid timestamp",
            );
        }
        timestamp
    }

    fn unrepresentable_tokens(&self) {
        self.record(
            ccusage_core::SkipKind::InvalidData,
            "Gemini token counts exceed the u64 range",
        );
    }

    fn record(&self, kind: ccusage_core::SkipKind, detail: &str) {
        self.diagnostics.record(match self.line {
            Some(number) => ccusage_core::Skip::record(self.path, number, kind, detail),
            None => ccusage_core::Skip::unit(self.path, kind, detail),
        });
    }
}

fn parse_direct_event_record(
    record: &GeminiRecord,
    model_hint: Option<&str>,
    session_id: &str,
    time: &GeminiRecordTime<'_>,
) -> Option<GeminiUsageEvent> {
    let tokens = parse_usage_tokens(record.tokens.as_ref(), time)?;
    let usage = build_usage(
        record.model.as_deref().or(model_hint),
        tokens,
        normalize_session_input,
        record.id.clone(),
    )?;
    Some(usage.at(session_id, time.resolve(record)?))
}

fn parse_stats_events(
    record: &GeminiRecord,
    model_hint: Option<&str>,
    session_id: &str,
    time: &GeminiRecordTime<'_>,
) -> Vec<GeminiUsageEvent> {
    let usages = parse_stats_usages(record.stats(), model_hint, time);
    if usages.is_empty() {
        return Vec::new();
    }
    let Some(timestamp) = time.resolve(record) else {
        return Vec::new();
    };
    usages
        .into_iter()
        .map(|usage| usage.at(session_id, timestamp))
        .collect()
}

fn parse_stats_usages(
    stats: Option<&Value>,
    model_hint: Option<&str>,
    time: &GeminiRecordTime<'_>,
) -> Vec<GeminiUsage> {
    let Some(stats) = stats.and_then(Value::as_object) else {
        return Vec::new();
    };
    if let Some(models) = stats.get("models").and_then(Value::as_object) {
        let events = models
            .iter()
            .filter_map(|(model, data)| {
                let data = data.as_object()?;
                let tokens = parse_usage_tokens(data.get("tokens"), time)?;
                build_usage(Some(model), tokens, subtract_cached_overlap_tokens, None)
            })
            .collect::<Vec<_>>();
        if !events.is_empty() {
            return events;
        }
    }
    let Some(tokens) = parse_usage_tokens(Some(&Value::Object(stats.clone())), time) else {
        return Vec::new();
    };
    build_usage(
        model_hint.or(Some(DEFAULT_MODEL)),
        tokens,
        subtract_cached_overlap_tokens,
        None,
    )
    .into_iter()
    .collect()
}

fn parse_usage_tokens(value: Option<&Value>, time: &GeminiRecordTime<'_>) -> Option<GeminiTokens> {
    match parse_tokens(value) {
        Ok(tokens) => Some(tokens),
        Err(TokenRejection::Missing) => None,
        Err(TokenRejection::Unrepresentable) => {
            time.unrepresentable_tokens();
            None
        }
    }
}

fn build_usage(
    model: Option<&str>,
    tokens: GeminiTokens,
    normalize_input: fn(GeminiTokens) -> (u64, u64),
    message_id: Option<String>,
) -> Option<GeminiUsage> {
    let model = model.filter(|model| !model.trim().is_empty())?;
    let (input_without_cache, cache_read_tokens) = normalize_input(tokens);
    let input_tokens = input_without_cache.saturating_add(tokens.tool);
    let total_tokens = tokens.total.unwrap_or_else(|| {
        input_tokens
            .saturating_add(tokens.output)
            .saturating_add(cache_read_tokens)
            .saturating_add(tokens.thoughts)
    });
    if input_tokens == 0
        && tokens.output == 0
        && cache_read_tokens == 0
        && tokens.thoughts == 0
        && total_tokens == 0
    {
        return None;
    }
    Some(GeminiUsage {
        model: model.to_string(),
        input_tokens,
        output_tokens: tokens.output,
        cache_read_tokens,
        reasoning_tokens: tokens.thoughts,
        total_tokens,
        message_id,
    })
}

fn parse_tokens(value: Option<&Value>) -> std::result::Result<GeminiTokens, TokenRejection> {
    let record = value
        .and_then(Value::as_object)
        .ok_or(TokenRejection::Missing)?;
    let tokens = GeminiTokens {
        input: token_number(record, INPUT_KEYS),
        output: token_number(record, OUTPUT_KEYS),
        cached: token_number(record, CACHED_KEYS),
        thoughts: token_number(record, THOUGHTS_KEYS),
        tool: token_number(record, TOOL_KEYS),
        total: value_u64(TOTAL_KEYS.iter().find_map(|key| record.get(*key))),
    };
    tokens
        .component_total()
        .map(|_| tokens)
        .ok_or(TokenRejection::Unrepresentable)
}

fn token_number(record: &Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value_u64(record.get(*key)))
        .unwrap_or(0)
}

fn value_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(integer) = value.as_u64() {
        return Some(integer);
    }
    let float = value.as_f64()?;
    if !float.is_finite() {
        return None;
    }
    Some(float.max(0.0).trunc() as u64)
}

fn subtract_cached_overlap_tokens(tokens: GeminiTokens) -> (u64, u64) {
    let cache_read = tokens.cached;
    let cached_portion = tokens.input.min(cache_read);
    (tokens.input.saturating_sub(cached_portion), cache_read)
}

fn normalize_session_input(tokens: GeminiTokens) -> (u64, u64) {
    let inclusive_total = tokens
        .input
        .saturating_add(tokens.output)
        .saturating_add(tokens.thoughts)
        .saturating_add(tokens.tool);
    let exclusive_total = inclusive_total.saturating_add(tokens.cached);
    if tokens.cached > 0
        && tokens.total == Some(inclusive_total)
        && tokens.total != Some(exclusive_total)
    {
        return subtract_cached_overlap_tokens(tokens);
    }
    (tokens.input, tokens.cached)
}

pub(super) fn event_to_loaded(
    event: GeminiUsageEvent,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: event.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let additional =
        AdditionalTokens::from_total(usage, event.reasoning_tokens, event.total_tokens);
    let (report_usage, _) = additional.for_report(usage);
    let extra_total_tokens = event
        .total_tokens
        .saturating_sub(ccusage_core::total_usage_tokens(report_usage));
    let names = model_candidates(&event.model);
    let candidates: Vec<_> = names
        .iter()
        .map(|name| (name.as_str(), MatchKind::Fuzzy))
        .collect();
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage,
            reasoning_tokens: event.reasoning_tokens,
            residual_tokens: additional.residual,
            reported_model: Some(&event.model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(event.timestamp),
        },
        pricing,
    );
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| crate::model_aliases::resolve_model_name(&event.model).into_owned());
    LoadedEntry {
        date: date_at(event.timestamp, tz),
        timestamp: event.timestamp,
        project: Arc::from("gemini"),
        session_id: Arc::from(event.session_id),
        project_path: Arc::from("Gemini"),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(event.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        usage: report_usage,
        message_id: event.message_id,
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

fn model_candidates(model: &str) -> Vec<String> {
    let mut candidates = Vec::with_capacity(PROVIDER_PREFIXES.len() + 1);
    candidates.extend(
        PROVIDER_PREFIXES
            .iter()
            .map(|prefix| format!("{prefix}/{model}")),
    );
    candidates.push(model.to_string());
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

#[cfg(test)]
mod tests {

    use super::*;
    use ccusage_core::Diagnostics;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn whole_file_usage_can_use_its_modification_time() {
        let fixture = fs_fixture!({
            "chat.json": r#"{"type":"gemini","model":"gemini-3-flash-preview","tokens":{"input":10,"output":2}}"#,
        });
        let path = fixture.path("chat.json");
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_312_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let diagnostics = Diagnostics::new("gemini");
        let events = parse_json_file(&path, &diagnostics).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp.as_millis(), 1_767_312_000_000);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            0
        );
    }

    #[test]
    fn diagnoses_jsonl_usage_without_a_timestamp() {
        let fixture = ccusage_test_support::fs_fixture!({
            "chat.jsonl": r#"{"type":"gemini","model":"gemini-3-flash-preview","tokens":{"input":10,"output":2}}"#,
        });
        let diagnostics = ccusage_core::Diagnostics::new("gemini");
        let events = parse_jsonl_file(&fixture.path("chat.jsonl"), &diagnostics).unwrap();
        assert!(events.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            1
        );
    }

    #[test]
    fn undated_jsonl_records_without_countable_usage_are_not_diagnosed() {
        let fixture = ccusage_test_support::fs_fixture!({
            "chat.jsonl": concat!(
                r#"{"type":"gemini","tokens":{"input":10,"output":2}}"#,
                "\n",
                r#"{"type":"gemini","model":"gemini-3-flash-preview","tokens":{"input":0,"output":0}}"#,
                "\n",
                r#"{"stats":{"models":{"gemini-3-flash-preview":{"tokens":{"input":0}}}}}"#,
                "\n",
            ),
        });
        let diagnostics = ccusage_core::Diagnostics::new("gemini");
        let events = parse_jsonl_file(&fixture.path("chat.jsonl"), &diagnostics).unwrap();
        assert!(events.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            0
        );
    }

    #[test]
    fn diagnoses_malformed_whole_file_json() {
        let fixture = fs_fixture!({"chat.json": "{\"SECRET_CANARY\":}"});
        let diagnostics = Diagnostics::new("gemini");

        let loaded = ccusage_adapter_common::units::read_units(
            &[fixture.path("chat.json")],
            true,
            &diagnostics,
            |path| parse_json_file(path, &diagnostics),
        );

        assert!(loaded[0].is_none());
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }

    #[test]
    fn reads_json_that_is_not_an_object_as_holding_no_usage() {
        let fixture = fs_fixture!({
            "logs.json": r#"[{"sessionId":"s1","messageId":0,"type":"user","message":"hi","timestamp":"2026-05-17T11:07:32.000Z"}]"#,
            "empty-logs.json": "[]",
            "scalar.json": "7",
            "broken-logs.json": r#"[{"sessionId":"s1"},]"#,
        });
        let files = [
            "logs.json",
            "empty-logs.json",
            "scalar.json",
            "broken-logs.json",
        ]
        .map(|name| fixture.path(name));
        let diagnostics = Diagnostics::new("gemini");

        let loaded =
            ccusage_adapter_common::units::read_units(&files, true, &diagnostics, |path| {
                parse_json_file(path, &diagnostics)
            });

        assert!(
            loaded[..3]
                .iter()
                .all(|events| events.as_ref().is_some_and(Vec::is_empty))
        );
        assert!(loaded[3].is_none());
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }

    #[test]
    fn diagnoses_malformed_jsonl_and_keeps_later_usage() {
        let fixture = fs_fixture!({
            "session.jsonl": concat!(
                "{\"type\":\"gemini\",\"tokens\":}\n",
                r#"{"type":"gemini","timestamp":"2026-05-17T11:07:32.000Z","model":"gemini-3-flash-preview","tokens":{"input":100,"output":20}}"#,
                "\n",
            ),
        });
        let path = fixture.path("session.jsonl");
        let diagnostics = Diagnostics::new("gemini");

        let events = parse_jsonl_file(&path, &diagnostics).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }

    #[test]
    fn skips_and_diagnoses_token_counts_that_exceed_the_u64_range() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.jsonl": concat!(
                r#"{"type":"gemini","timestamp":"2026-05-17T11:07:32.000Z","model":"gemini-3-flash-preview","tokens":{"input":18446744073709551615,"output":1}}"#,
                "\n",
                r#"{"timestamp":"2026-05-17T11:07:32.000Z","stats":{"models":{"gemini-3-flash-preview":{"tokens":{"input":18446744073709551615,"output":1}}}}}"#,
                "\n",
                r#"{"type":"gemini","timestamp":"2026-05-17T11:07:33.000Z","model":"gemini-3-flash-preview","tokens":{"input":10,"output":2}}"#,
                "\n",
            ),
        });
        let diagnostics = ccusage_core::Diagnostics::new("gemini");

        let events = parse_jsonl_file(&fixture.path("session.jsonl"), &diagnostics).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 10);
        assert_eq!(events[0].output_tokens, 2);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 2);
    }

    #[test]
    fn falls_back_to_total_tokens_when_gemini_parts_are_missing() {
        let event = build_usage(
            Some("gemini-test"),
            GeminiTokens {
                total: Some(654),
                ..GeminiTokens::default()
            },
            normalize_session_input,
            None,
        )
        .unwrap();

        let entry = event_to_loaded(
            event.at("session", TimestampMs::UNIX_EPOCH),
            &JiffTimeZone::UTC,
            PriceTable::Display,
        );
        assert_eq!(entry.usage.output_tokens, 654);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn type_discriminator_is_untrimmed_and_tolerates_non_strings() {
        let exact = serde_json::from_str::<GeminiRecord>(r#"{"type":"gemini"}"#).unwrap();
        assert_eq!(exact.r#type.as_deref(), Some("gemini"));

        let padded = serde_json::from_str::<GeminiRecord>(r#"{"type":" gemini "}"#).unwrap();
        assert_eq!(padded.r#type.as_deref(), Some(" gemini "));
        assert_ne!(padded.r#type.as_deref(), Some("gemini"));

        let numeric = serde_json::from_str::<GeminiRecord>(r#"{"type":5,"stats":{}}"#).unwrap();
        assert_eq!(numeric.r#type, None);
        assert!(numeric.stats.is_some());
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            model_candidates("gemini-2.5-flash"),
            [
                "google/gemini-2.5-flash",
                "gemini/gemini-2.5-flash",
                "vertex_ai/gemini-2.5-flash",
                "openrouter/google/gemini-2.5-flash",
                "gemini-2.5-flash"
            ]
        );
    }
}
