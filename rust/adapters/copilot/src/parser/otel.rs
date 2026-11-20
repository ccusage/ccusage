use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value};

use super::{CallIds, CallKey, CopilotUsageEntry, copilot_model_names};
use crate::{TimestampMs, TokenUsageRaw, apply_total_token_fallback, fast::LinePrefilter};
use ccusage_adapter_common::{jsonl, units::UnitError};
use ccusage_core::Diagnostics;

// NOTE(2026-05-19, #1023): Copilot OpenTelemetry exporters write timestamps as scalars or `[seconds, nanos]` arrays. Keep both representations readable.
#[derive(Debug, Deserialize)]
struct CopilotRecord {
    #[serde(rename = "type")]
    record_type: Option<Value>,
    name: Option<Value>,
    #[serde(rename = "spanId")]
    span_id: Option<Value>,
    #[serde(rename = "traceId")]
    trace_id: Option<Value>,
    #[serde(rename = "spanContext")]
    span_context: Option<Value>,
    #[serde(rename = "startTime")]
    start_time: Option<Value>,
    #[serde(rename = "endTime")]
    end_time: Option<Value>,
    duration: Option<Value>,
    kind: Option<Value>,
    #[serde(rename = "hrTime")]
    hr_time: Option<Value>,
    #[serde(rename = "_hrTime")]
    underscore_hr_time: Option<Value>,
    time: Option<Value>,
    timestamp: Option<Value>,
    #[serde(rename = "observedTimestamp")]
    observed_timestamp: Option<Value>,
    #[serde(rename = "timeUnixNano")]
    time_unix_nano: Option<Value>,
    body: Option<Value>,
    #[serde(rename = "_body")]
    underscore_body: Option<Value>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CopilotUsageSource {
    ChatSpan,
    InferenceLog,
    AgentTurnLog,
    AgentSummarySpan,
}

#[derive(Default)]
struct TraceContext {
    model: Option<String>,
    session_id: Option<String>,
    session_id_priority: u8,
}

struct CopilotUsageCandidate {
    source: CopilotUsageSource,
    trace_id: Option<String>,
    response_id: Option<String>,
    service_request_id: Option<String>,
    model: String,
    logged_model: Option<String>,
    session_id: String,
    timestamp: TimestampMs,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_output_tokens: u64,
    extra_total_tokens: u64,
    dedup_key: String,
    call_key: Option<CallKey>,
}

pub(crate) fn parse_otel_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> Result<Vec<CopilotUsageEntry>, UnitError> {
    let prefilter = LinePrefilter::all(&[br#""attributes""#]);
    let mut numbers = Vec::new();
    let mut records = Vec::new();
    jsonl::read_records::<CopilotRecord>(
        path,
        Some(&prefilter),
        diagnostics,
        is_otel_usage_envelope,
        |number, record, _| {
            numbers.push(number);
            records.push(record);
        },
    )?;
    let trace_contexts = collect_trace_contexts(&records);
    let candidates = records
        .iter()
        .enumerate()
        .zip(numbers)
        .filter_map(|((index, record), number)| {
            to_candidate(record, index, &trace_contexts, path, number, diagnostics)
        })
        .collect::<Vec<_>>();
    let sets = CandidateSets::new(&candidates);
    Ok(candidates
        .into_iter()
        .enumerate()
        .filter(|(index, candidate)| should_emit_candidate(*index, candidate, &sets))
        .map(|(_, candidate)| CopilotUsageEntry {
            timestamp: candidate.timestamp,
            session_id: candidate.session_id,
            model: candidate.model,
            logged_model: candidate.logged_model,
            input_tokens: candidate.input_tokens,
            output_tokens: candidate.output_tokens,
            cache_creation_tokens: candidate.cache_creation_tokens,
            cache_read_tokens: candidate.cache_read_tokens,
            reasoning_output_tokens: candidate.reasoning_output_tokens,
            extra_total_tokens: candidate.extra_total_tokens,
            request_count: 1,
            nano_aiu: None,
            dedup_key: candidate.dedup_key,
            call_key: candidate.call_key,
            call_ids: CallIds {
                response_id: candidate.response_id,
                service_request_id: candidate.service_request_id,
            },
        })
        .collect())
}

const INPUT_TOKENS_ATTRIBUTE: &str = "gen_ai.usage.input_tokens";
const OUTPUT_TOKENS_ATTRIBUTE: &str = "gen_ai.usage.output_tokens";
const CACHE_READ_ATTRIBUTE: &str = "gen_ai.usage.cache_read.input_tokens";
const CACHE_WRITE_ATTRIBUTES: [&str; 2] = [
    "gen_ai.usage.cache_write.input_tokens",
    "gen_ai.usage.cache_creation.input_tokens",
];
const REASONING_ATTRIBUTES: [&str; 2] = [
    "gen_ai.usage.reasoning.output_tokens",
    "gen_ai.usage.reasoning_tokens",
];
const TOTAL_TOKENS_ATTRIBUTES: [&str; 2] = [
    "gen_ai.usage.total_tokens",
    "gen_ai.usage.total.token_count",
];

pub(super) fn is_otel_usage_envelope(value: &Value) -> bool {
    value
        .get("attributes")
        .and_then(Value::as_object)
        .is_some_and(|attributes| {
            [
                INPUT_TOKENS_ATTRIBUTE,
                OUTPUT_TOKENS_ATTRIBUTE,
                CACHE_READ_ATTRIBUTE,
            ]
            .into_iter()
            .chain(CACHE_WRITE_ATTRIBUTES)
            .chain(REASONING_ATTRIBUTES)
            .chain(TOTAL_TOKENS_ATTRIBUTES)
            .any(|key| attributes.contains_key(key))
        })
}

fn collect_trace_contexts(records: &[CopilotRecord]) -> HashMap<String, TraceContext> {
    let mut contexts = HashMap::new();
    for record in records {
        let Some(trace_id) = trace_id_from_record(record) else {
            continue;
        };
        let Some(attributes) = record.attributes.as_ref() else {
            continue;
        };
        let context = contexts
            .entry(trace_id)
            .or_insert_with(TraceContext::default);
        if context.model.is_none() {
            context.model = first_non_empty_model_attr(attributes);
        }
        if let Some((session_id, priority)) = best_session_attr(attributes)
            && priority > context.session_id_priority
        {
            context.session_id = Some(session_id);
            context.session_id_priority = priority;
        }
    }
    contexts
}

fn to_candidate(
    record: &CopilotRecord,
    index: usize,
    trace_contexts: &HashMap<String, TraceContext>,
    path: &Path,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<CopilotUsageCandidate> {
    let attributes = record.attributes.as_ref()?;
    let source = if is_chat_span_record(record, attributes) {
        CopilotUsageSource::ChatSpan
    } else if is_inference_log_record(record, attributes) {
        CopilotUsageSource::InferenceLog
    } else if is_agent_turn_log_record(record, attributes) {
        CopilotUsageSource::AgentTurnLog
    } else if is_agent_summary_span_record(record, attributes) {
        CopilotUsageSource::AgentSummarySpan
    } else {
        return None;
    };
    let input = attr_number(attributes, INPUT_TOKENS_ATTRIBUTE);
    let output = attr_number(attributes, OUTPUT_TOKENS_ATTRIBUTE);
    let cache_read = attr_number(attributes, CACHE_READ_ATTRIBUTE);
    let cache_creation = attr_number_first(attributes, &CACHE_WRITE_ATTRIBUTES);
    let reasoning = attr_number_first(attributes, &REASONING_ATTRIBUTES);
    let total = attr_number_first(attributes, &TOTAL_TOKENS_ATTRIBUTES);
    // NOTE(2026-10-10, #1858): Copilot session-state and OpenTelemetry input counts already include cache reads and writes. Carve those buckets out of the reported input to avoid double counting.
    let usage = TokenUsageRaw {
        input_tokens: input.saturating_sub(cache_read.saturating_add(cache_creation)),
        output_tokens: output,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    };
    let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, total);
    if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
        return None;
    }
    let trace_id = trace_id_from_record(record);
    let trace_context = trace_id.as_ref().and_then(|id| trace_contexts.get(id));
    let response_id = attr_string(attributes, "gen_ai.response.id");
    let service_request_id = attr_string(attributes, "github.copilot.service_request_id");
    let logged_model = first_non_empty_model_attr(attributes)
        .or_else(|| trace_context.and_then(|context| context.model.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let (model, logged_model) = copilot_model_names(logged_model);
    let session_id = best_session_attr(attributes)
        .map(|(session_id, _)| session_id)
        .or_else(|| trace_context.and_then(|context| context.session_id.clone()))
        .or_else(|| trace_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let Some(timestamp) = timestamp_from_record(record) else {
        diagnostics.record(ccusage_core::Skip::record(
            path,
            number,
            ccusage_core::SkipKind::InvalidTimestamp,
            "Copilot usage has no valid timestamp",
        ));
        return None;
    };
    let (dedup_key, call_key) = dedup_key_for_record(
        source,
        record,
        attributes,
        &trace_id,
        &session_id,
        timestamp,
        index,
    );
    Some(CopilotUsageCandidate {
        source,
        trace_id,
        response_id,
        service_request_id,
        model,
        logged_model,
        session_id,
        timestamp,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: reasoning,
        extra_total_tokens,
        dedup_key,
        call_key,
    })
}

impl CopilotUsageSource {
    fn rank(self) -> u8 {
        match self {
            Self::ChatSpan => 0,
            Self::InferenceLog => 1,
            Self::AgentTurnLog => 2,
            Self::AgentSummarySpan => 3,
        }
    }

    fn outranks(self, other: Self) -> bool {
        self.rank() < other.rank()
    }
}

struct CandidateSets {
    best_source_by_trace: HashMap<String, CopilotUsageSource>,
    best_source_by_response: HashMap<String, CopilotUsageSource>,
    repeated_chat_spans: HashSet<usize>,
}

impl CandidateSets {
    fn new(candidates: &[CopilotUsageCandidate]) -> Self {
        let mut best_source_by_trace = HashMap::new();
        let mut best_source_by_response = HashMap::new();
        for candidate in candidates {
            for (best_sources, id) in [
                (&mut best_source_by_trace, &candidate.trace_id),
                (&mut best_source_by_response, &candidate.response_id),
            ] {
                if let Some(id) = id {
                    keep_best_source(best_sources, id, candidate.source);
                }
            }
        }
        Self {
            best_source_by_trace,
            best_source_by_response,
            repeated_chat_spans: repeated_chat_spans(candidates),
        }
    }
}

fn keep_best_source(
    best_sources: &mut HashMap<String, CopilotUsageSource>,
    id: &str,
    source: CopilotUsageSource,
) {
    match best_sources.get_mut(id) {
        Some(best) if source.outranks(*best) => *best = source,
        Some(_) => {}
        None => {
            best_sources.insert(id.to_string(), source);
        }
    }
}

// NOTE(2026-10-10, #1858): Copilot CLI can re-export a chat span under new trace or span IDs with the original response ID and usage. VS Code reuses response IDs across a turn, so compare usage as well.
fn repeated_chat_spans(candidates: &[CopilotUsageCandidate]) -> HashSet<usize> {
    let mut earliest = HashMap::new();
    let mut repeated = HashSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.source != CopilotUsageSource::ChatSpan {
            continue;
        }
        let Some(response_id) = candidate.response_id.as_deref() else {
            continue;
        };
        let call = (
            response_id,
            candidate.model.as_str(),
            candidate.input_tokens,
            candidate.output_tokens,
            candidate.cache_creation_tokens,
            candidate.cache_read_tokens,
            candidate.reasoning_output_tokens,
            candidate.extra_total_tokens,
        );
        let kept = *earliest.entry(call).or_insert(index);
        if kept == index {
            continue;
        }
        if candidate.timestamp < candidates[kept].timestamp {
            repeated.insert(kept);
            earliest.insert(call, index);
        } else {
            repeated.insert(index);
        }
    }
    repeated
}

fn should_emit_candidate(
    index: usize,
    candidate: &CopilotUsageCandidate,
    sets: &CandidateSets,
) -> bool {
    if candidate.source == CopilotUsageSource::ChatSpan {
        return !sets.repeated_chat_spans.contains(&index);
    }
    let outranked = |best_sources: &HashMap<String, CopilotUsageSource>, id: &Option<String>| {
        id.as_ref()
            .and_then(|id| best_sources.get(id))
            .is_some_and(|best| best.outranks(candidate.source))
    };
    !outranked(&sets.best_source_by_trace, &candidate.trace_id)
        && !outranked(&sets.best_source_by_response, &candidate.response_id)
}

const MODEL_ATTRS: &[&str] = &["gen_ai.response.model", "gen_ai.request.model"];
const SESSION_ATTRS: &[(&str, u8)] = &[
    ("gen_ai.conversation.id", 3),
    ("copilot_chat.session_id", 3),
    ("copilot_chat.chat_session_id", 3),
    ("session.id", 3),
    ("github.copilot.interaction_id", 2),
    ("gen_ai.response.id", 1),
];

fn first_non_empty_model_attr(attributes: &Map<String, Value>) -> Option<String> {
    MODEL_ATTRS
        .iter()
        .filter_map(|key| attributes.get(*key)?.as_str())
        .find(|model| !model.trim().is_empty())
        .map(str::to_string)
}

fn is_span_record(record: &CopilotRecord) -> bool {
    if let Some(record_type) = record.record_type.as_ref().and_then(Value::as_str) {
        return record_type == "span";
    }
    string_value(record.name.as_ref()).is_some()
        && (string_value(record.span_id.as_ref()).is_some()
            || string_value(record.trace_id.as_ref()).is_some()
            || record.start_time.is_some()
            || record.end_time.is_some()
            || record.duration.is_some()
            || record.kind.is_some())
}

fn is_chat_span_record(record: &CopilotRecord, attributes: &Map<String, Value>) -> bool {
    is_span_record(record)
        && (attr_string(attributes, "gen_ai.operation.name").as_deref() == Some("chat")
            || string_value(record.name.as_ref()).is_some_and(|name| name.starts_with("chat ")))
}

fn is_agent_summary_span_record(record: &CopilotRecord, attributes: &Map<String, Value>) -> bool {
    is_span_record(record)
        && (attr_string(attributes, "gen_ai.operation.name").as_deref() == Some("invoke_agent")
            || string_value(record.name.as_ref())
                .is_some_and(|name| name.starts_with("invoke_agent ")))
}

fn is_inference_log_record(record: &CopilotRecord, attributes: &Map<String, Value>) -> bool {
    !is_span_record(record)
        && (attr_string(attributes, "event.name").as_deref()
            == Some("gen_ai.client.inference.operation.details")
            || record_body(record).is_some_and(|body| body.starts_with("GenAI inference:")))
}

fn is_agent_turn_log_record(record: &CopilotRecord, attributes: &Map<String, Value>) -> bool {
    !is_span_record(record)
        && (attr_string(attributes, "event.name").as_deref() == Some("copilot_chat.agent.turn")
            || record_body(record).is_some_and(|body| body.starts_with("copilot_chat.agent.turn")))
}

fn dedup_key_for_record(
    source: CopilotUsageSource,
    record: &CopilotRecord,
    attributes: &Map<String, Value>,
    trace_id: &Option<String>,
    session_id: &str,
    timestamp: TimestampMs,
    index: usize,
) -> (String, Option<CallKey>) {
    let trace = trace_id.clone();
    let call_key = match source {
        CopilotUsageSource::ChatSpan | CopilotUsageSource::AgentSummarySpan => trace
            .zip(span_id_from_record(record))
            .map(|(trace, span)| CallKey::Span { trace, span }),
        CopilotUsageSource::InferenceLog => trace
            .zip(span_id_from_record(record))
            .map(|(trace, span)| CallKey::Log { trace, span }),
        CopilotUsageSource::AgentTurnLog => trace
            .zip(turn_index(attributes))
            .map(|(trace, turn)| CallKey::AgentTurn { trace, turn }),
    };
    if let Some(call_key) = call_key {
        return (call_key.to_string(), Some(call_key));
    }
    let millis = timestamp.as_millis();
    let key = match source {
        CopilotUsageSource::ChatSpan | CopilotUsageSource::AgentSummarySpan => {
            format!("span:{session_id}:{millis}:{index}")
        }
        CopilotUsageSource::InferenceLog => format!("log:{session_id}:{millis}:{index}"),
        CopilotUsageSource::AgentTurnLog => {
            let turn_index = turn_index(attributes)
                .map_or_else(|| format!("idx-{index}"), |turn| turn.to_string());
            trace_id.as_ref().map_or_else(
                || format!("agent-turn:{session_id}:{turn_index}:{index}"),
                |trace_id| format!("agent-turn:{trace_id}:{turn_index}"),
            )
        }
    };
    (key, None)
}

fn turn_index(attributes: &Map<String, Value>) -> Option<u64> {
    number_value(attributes.get("turn.index"))
        .or_else(|| number_value(attributes.get("copilot_chat.turn.index")))
}

fn trace_id_from_record(record: &CopilotRecord) -> Option<String> {
    string_value(record.trace_id.as_ref())
        .map(str::to_string)
        .or_else(|| nested_string(record.span_context.as_ref(), "traceId"))
}

fn span_id_from_record(record: &CopilotRecord) -> Option<String> {
    string_value(record.span_id.as_ref())
        .map(str::to_string)
        .or_else(|| nested_string(record.span_context.as_ref(), "spanId"))
}

fn nested_string(object: Option<&Value>, key: &str) -> Option<String> {
    object
        .and_then(Value::as_object)
        .and_then(|object| string_value(object.get(key)))
        .map(str::to_string)
}

fn record_body(record: &CopilotRecord) -> Option<&str> {
    string_value(record.body.as_ref()).or_else(|| string_value(record.underscore_body.as_ref()))
}

fn string_value(value: Option<&Value>) -> Option<&str> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then_some(value)
}

fn number_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_i64()
                .and_then(|value| (value >= 0).then_some(value as u64))
        }),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn attr_string(attributes: &Map<String, Value>, key: &str) -> Option<String> {
    string_value(attributes.get(key)).map(str::to_string)
}

fn attr_number(attributes: &Map<String, Value>, key: &str) -> u64 {
    number_value(attributes.get(key)).unwrap_or_default()
}

fn attr_number_first(attributes: &Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .map(|key| attr_number(attributes, key))
        .find(|value| *value > 0)
        .unwrap_or_default()
}

fn best_session_attr(attributes: &Map<String, Value>) -> Option<(String, u8)> {
    SESSION_ATTRS
        .iter()
        .filter_map(|(key, priority)| attr_string(attributes, key).map(|value| (value, *priority)))
        .max_by_key(|(_, priority)| *priority)
}

fn timestamp_from_record(record: &CopilotRecord) -> Option<TimestampMs> {
    timestamp_from_parts(record.end_time.as_ref())
        .or_else(|| timestamp_from_parts(record.start_time.as_ref()))
        .or_else(|| timestamp_from_parts(record.hr_time.as_ref()))
        .or_else(|| timestamp_from_parts(record.underscore_hr_time.as_ref()))
        .or_else(|| timestamp_from_parts(record.time.as_ref()))
        .or_else(|| timestamp_from_scalar(record.timestamp.as_ref()))
        .or_else(|| timestamp_from_scalar(record.observed_timestamp.as_ref()))
        .or_else(|| timestamp_from_unix_nanos(record.time_unix_nano.as_ref()))
}

pub(super) fn timestamp_from_parts(value: Option<&Value>) -> Option<TimestampMs> {
    let values = value?.as_array()?;
    let seconds = number_value(values.first())?;
    let nanos = number_value(values.get(1))?;
    let millis = seconds.checked_mul(1_000)?.checked_add(nanos / 1_000_000)?;
    TimestampMs::from_instant_millis(i64::try_from(millis).ok()?)
}

fn timestamp_from_scalar(value: Option<&Value>) -> Option<TimestampMs> {
    let raw = number_value(value)?;
    let millis = if raw >= 100_000_000_000_000_000 {
        raw / 1_000_000
    } else if raw >= 100_000_000_000_000 {
        raw / 1_000
    } else {
        return ccusage_adapter_common::timestamps::epoch_secs_or_millis(i64::try_from(raw).ok()?);
    };
    TimestampMs::from_instant_millis(i64::try_from(millis).ok()?)
}

fn timestamp_from_unix_nanos(value: Option<&Value>) -> Option<TimestampMs> {
    let raw = number_value(value).filter(|raw| *raw > 0)?;
    TimestampMs::from_instant_millis(i64::try_from(raw / 1_000_000).ok()?)
}
