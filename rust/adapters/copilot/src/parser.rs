use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{Result, TimestampMs, TokenUsageRaw, apply_total_token_fallback, fast::LinePrefilter};
use ccusage_adapter_common::jsonl;

/// A single parsed Copilot OpenTelemetry record. Only the fields ccusage
/// consumes are declared; serde skips everything else. The `attributes` block
/// is kept as a dynamic map because Copilot addresses it by arbitrary dotted
/// keys (for example `gen_ai.usage.input_tokens`), and the timestamp-bearing
/// fields stay as raw values because they appear as both numeric scalars and
/// `[seconds, nanos]` arrays depending on the exporter.
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

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) dedup_key: String,
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
    model: String,
    session_id: String,
    timestamp: TimestampMs,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_output_tokens: u64,
    dedup_key: String,
}

pub(super) fn parse_otel_file(path: &Path) -> Result<Vec<CopilotUsageEntry>> {
    let content = fs::read(path)?;
    // Every usable Copilot OTel record carries the `attributes` object, so
    // lines without it are skipped before JSON parsing.
    let prefilter = LinePrefilter::all(&[br#""attributes""#]);
    let records = jsonl::records::<CopilotRecord>(&content, Some(&prefilter)).collect::<Vec<_>>();
    let trace_contexts = collect_trace_contexts(&records);
    let fallback_timestamp = file_modified_timestamp(path);
    let candidates = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            to_candidate(record, index, fallback_timestamp, &trace_contexts)
        })
        .collect::<Vec<_>>();
    let sets = CandidateSets::new(&candidates);
    Ok(candidates
        .into_iter()
        .filter(|candidate| should_emit_candidate(candidate, &sets))
        .map(|candidate| CopilotUsageEntry {
            timestamp: candidate.timestamp,
            timestamp_text: crate::format_rfc3339_millis(candidate.timestamp),
            session_id: candidate.session_id,
            model: candidate.model,
            input_tokens: candidate.input_tokens,
            output_tokens: candidate.output_tokens,
            cache_creation_tokens: candidate.cache_creation_tokens,
            cache_read_tokens: candidate.cache_read_tokens,
            reasoning_output_tokens: candidate.reasoning_output_tokens,
            dedup_key: candidate.dedup_key,
        })
        .collect())
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
            context.model = first_non_empty_attr(attributes, MODEL_ATTRS);
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
    fallback_timestamp: TimestampMs,
    trace_contexts: &HashMap<String, TraceContext>,
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
    let input = attr_number(attributes, "gen_ai.usage.input_tokens");
    let output = attr_number(attributes, "gen_ai.usage.output_tokens");
    let cache_read = attr_number(attributes, "gen_ai.usage.cache_read.input_tokens");
    let cache_creation = attr_number_first(
        attributes,
        &[
            "gen_ai.usage.cache_write.input_tokens",
            "gen_ai.usage.cache_creation.input_tokens",
        ],
    );
    let reasoning = attr_number_first(
        attributes,
        &[
            "gen_ai.usage.reasoning.output_tokens",
            "gen_ai.usage.reasoning_tokens",
        ],
    );
    let total = attr_number_first(
        attributes,
        &[
            "gen_ai.usage.total_tokens",
            "gen_ai.usage.total.token_count",
        ],
    );
    let usage = TokenUsageRaw {
        input_tokens: input.saturating_sub(input.min(cache_read)),
        output_tokens: output,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    };
    let (usage, reasoning) = apply_total_token_fallback(usage, reasoning, total);
    if crate::total_usage_tokens(usage) + reasoning == 0 {
        return None;
    }
    let trace_id = trace_id_from_record(record);
    let trace_context = trace_id.as_ref().and_then(|id| trace_contexts.get(id));
    let response_id = attr_string(attributes, "gen_ai.response.id");
    let model = first_non_empty_attr(attributes, MODEL_ATTRS)
        .or_else(|| trace_context.and_then(|context| context.model.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let session_id = best_session_attr(attributes)
        .map(|(session_id, _)| session_id)
        .or_else(|| trace_context.and_then(|context| context.session_id.clone()))
        .or_else(|| trace_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let timestamp = timestamp_from_record(record).unwrap_or(fallback_timestamp);
    let dedup_key = dedup_key_for_record(
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
        model,
        session_id,
        timestamp,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: reasoning,
        dedup_key,
    })
}

struct CandidateSets {
    chat_traces: HashSet<String>,
    inference_traces: HashSet<String>,
    agent_turn_traces: HashSet<String>,
    chat_response_ids: HashSet<String>,
    inference_response_ids: HashSet<String>,
    agent_turn_response_ids: HashSet<String>,
}

impl CandidateSets {
    fn new(candidates: &[CopilotUsageCandidate]) -> Self {
        Self {
            chat_traces: source_trace_ids(candidates, CopilotUsageSource::ChatSpan),
            inference_traces: source_trace_ids(candidates, CopilotUsageSource::InferenceLog),
            agent_turn_traces: source_trace_ids(candidates, CopilotUsageSource::AgentTurnLog),
            chat_response_ids: source_response_ids(candidates, CopilotUsageSource::ChatSpan),
            inference_response_ids: source_response_ids(
                candidates,
                CopilotUsageSource::InferenceLog,
            ),
            agent_turn_response_ids: source_response_ids(
                candidates,
                CopilotUsageSource::AgentTurnLog,
            ),
        }
    }
}

fn source_trace_ids(
    candidates: &[CopilotUsageCandidate],
    source: CopilotUsageSource,
) -> HashSet<String> {
    candidates
        .iter()
        .filter(|candidate| candidate.source == source)
        .filter_map(|candidate| candidate.trace_id.clone())
        .collect()
}

fn source_response_ids(
    candidates: &[CopilotUsageCandidate],
    source: CopilotUsageSource,
) -> HashSet<String> {
    candidates
        .iter()
        .filter(|candidate| candidate.source == source)
        .filter_map(|candidate| candidate.response_id.clone())
        .collect()
}

fn should_emit_candidate(candidate: &CopilotUsageCandidate, sets: &CandidateSets) -> bool {
    let trace_match = |values: &HashSet<String>| {
        candidate
            .trace_id
            .as_ref()
            .is_some_and(|trace_id| values.contains(trace_id))
    };
    let response_match = |values: &HashSet<String>| {
        candidate
            .response_id
            .as_ref()
            .is_some_and(|response_id| values.contains(response_id))
    };
    match candidate.source {
        CopilotUsageSource::ChatSpan => true,
        CopilotUsageSource::InferenceLog => {
            !trace_match(&sets.chat_traces) && !response_match(&sets.chat_response_ids)
        }
        CopilotUsageSource::AgentTurnLog => {
            !trace_match(&sets.chat_traces)
                && !trace_match(&sets.inference_traces)
                && !response_match(&sets.chat_response_ids)
                && !response_match(&sets.inference_response_ids)
        }
        CopilotUsageSource::AgentSummarySpan => {
            !trace_match(&sets.chat_traces)
                && !trace_match(&sets.inference_traces)
                && !trace_match(&sets.agent_turn_traces)
                && !response_match(&sets.chat_response_ids)
                && !response_match(&sets.inference_response_ids)
                && !response_match(&sets.agent_turn_response_ids)
        }
    }
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
) -> String {
    let span_id = span_id_from_record(record);
    match source {
        CopilotUsageSource::ChatSpan | CopilotUsageSource::AgentSummarySpan => {
            if let (Some(trace_id), Some(span_id)) = (trace_id, span_id) {
                return format!("{trace_id}:{span_id}");
            }
            format!("span:{session_id}:{}:{index}", timestamp.as_millis())
        }
        CopilotUsageSource::InferenceLog => {
            if let (Some(trace_id), Some(span_id)) = (trace_id, span_id) {
                return format!("log:{trace_id}:{span_id}");
            }
            format!("log:{session_id}:{}:{index}", timestamp.as_millis())
        }
        CopilotUsageSource::AgentTurnLog => {
            let turn_index = number_value(attributes.get("turn.index"))
                .or_else(|| number_value(attributes.get("copilot_chat.turn.index")))
                .map_or_else(|| format!("idx-{index}"), |value| value.to_string());
            trace_id.as_ref().map_or_else(
                || format!("agent-turn:{session_id}:{turn_index}:{index}"),
                |trace_id| format!("agent-turn:{trace_id}:{turn_index}"),
            )
        }
    }
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
        _ => None,
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

fn first_non_empty_attr(attributes: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| attr_string(attributes, key))
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

fn timestamp_from_parts(value: Option<&Value>) -> Option<TimestampMs> {
    let values = value?.as_array()?;
    let seconds = number_value(values.first())?;
    let nanos = number_value(values.get(1))?;
    let millis = seconds.checked_mul(1_000)?.checked_add(nanos / 1_000_000)?;
    Some(TimestampMs::from_millis(millis.min(i64::MAX as u64) as i64))
}

fn timestamp_from_scalar(value: Option<&Value>) -> Option<TimestampMs> {
    let raw = number_value(value)?;
    let millis = if raw >= 100_000_000_000_000_000 {
        raw / 1_000_000
    } else if raw >= 100_000_000_000_000 {
        raw / 1_000
    } else if raw >= 100_000_000_000 {
        raw
    } else {
        raw * 1_000
    };
    Some(TimestampMs::from_millis(millis.min(i64::MAX as u64) as i64))
}

fn timestamp_from_unix_nanos(value: Option<&Value>) -> Option<TimestampMs> {
    let raw = number_value(value)?;
    (raw > 0).then(|| TimestampMs::from_millis((raw / 1_000_000).min(i64::MAX as u64) as i64))
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| TimestampMs::from_millis(duration.as_millis().min(i64::MAX as u128) as i64))
        .unwrap_or_else(crate::utc_now)
}
