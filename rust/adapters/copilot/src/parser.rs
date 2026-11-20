use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    TimestampMs, TokenUsageRaw, apply_total_token_fallback, fast::LinePrefilter, parse_ts_timestamp,
};
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

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) logged_model: Option<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) extra_total_tokens: u64,
    pub(super) request_count: u64,
    pub(super) nano_aiu: Option<u64>,
    pub(super) dedup_key: String,
    pub(super) call_key: Option<CallKey>,
    pub(super) call_ids: CallIds,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CallKey {
    Span { trace: String, span: String },
    Log { trace: String, span: String },
    AgentTurn { trace: String, turn: u64 },
}

impl fmt::Display for CallKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Span { trace, span } => write!(formatter, "{trace}:{span}"),
            Self::Log { trace, span } => write!(formatter, "log:{trace}:{span}"),
            Self::AgentTurn { trace, turn } => write!(formatter, "agent-turn:{trace}:{turn}"),
        }
    }
}

// NOTE(2026-11-16, #2108): Copilot `apiCallId` corresponds to OTel `gen_ai.response.id` and `serviceRequestId` corresponds to `github.copilot.service_request_id`. Match each ID in its own namespace.
#[derive(Debug, Clone, Default)]
pub(super) struct CallIds {
    response_id: Option<String>,
    service_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CallIdKind {
    Response,
    ServiceRequest,
}

impl CallIds {
    pub(super) fn iter(&self) -> impl Iterator<Item = (CallIdKind, &str)> {
        [
            (CallIdKind::Response, self.response_id.as_deref()),
            (
                CallIdKind::ServiceRequest,
                self.service_request_id.as_deref(),
            ),
        ]
        .into_iter()
        .filter_map(|(kind, id)| Some((kind, id?)))
    }
}

#[derive(Debug, Clone)]
pub(super) struct CopilotCreditSnapshot {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: String,
    pub(super) is_shutdown: bool,
    pub(super) total_nano_aiu: Option<u64>,
    pub(super) receipt_watermark: Option<u64>,
    pub(super) dedup_key: String,
}

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageRecordEntry {
    pub(super) entry: CopilotUsageEntry,
    pub(super) source_session_id: Option<String>,
    pub(super) sequence: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct CopilotSourceUsage {
    pub(super) entries: Vec<CopilotUsageEntry>,
    pub(super) usage_records: Vec<CopilotUsageRecordEntry>,
    pub(super) credit_snapshots: Vec<CopilotCreditSnapshot>,
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

pub(super) fn parse_otel_file(
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

#[derive(Debug, Default, Deserialize)]
struct CopilotSessionStateEvent {
    #[serde(rename = "type", default, deserialize_with = "jsonl::non_empty_string")]
    event_type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    data: Option<CopilotSessionStateData>,
}

impl CopilotSessionStateEvent {
    fn carries_usage(&self) -> bool {
        let Some(data) = &self.data else {
            return false;
        };
        match self.event_type.as_deref() {
            Some(USAGE_RECORD_EVENT) => data.usage.is_some(),
            Some(SHUTDOWN_EVENT | USAGE_CHECKPOINT_EVENT) => {
                data.total_nano_aiu.is_some()
                    || data.model_metrics.as_ref().is_some_and(|models| {
                        models
                            .values()
                            .any(|metrics| metrics.usage.is_some() || metrics.requests.is_some())
                    })
            }
            _ => false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct CopilotSessionStateData {
    #[serde(
        rename = "modelMetrics",
        default,
        deserialize_with = "jsonl::lenient_object"
    )]
    model_metrics: Option<BTreeMap<String, CopilotSessionModelMetrics>>,
    #[serde(
        rename = "totalNanoAiu",
        default,
        deserialize_with = "jsonl::lenient_f64"
    )]
    total_nano_aiu: Option<f64>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<CopilotUsageRecord>,
    #[serde(
        rename = "usageAccountingWatermarks",
        default,
        deserialize_with = "jsonl::lenient_object"
    )]
    usage_accounting_watermarks: Option<BTreeMap<String, Value>>,
}

impl CopilotSessionStateData {
    fn covers_usage_records(&self) -> bool {
        self.usage_accounting_watermarks
            .as_ref()
            .is_some_and(|watermarks| {
                watermarks
                    .values()
                    .any(|sequence| sequence.as_u64().is_some_and(|sequence| sequence > 0))
            })
    }
}

#[derive(Debug, Default, Deserialize)]
struct CopilotUsageRecord {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    accounting: Option<CopilotUsageAccounting>,
    #[serde(
        rename = "apiCallId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    api_call_id: Option<String>,
    #[serde(
        rename = "serviceRequestId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    service_request_id: Option<String>,
    #[serde(
        rename = "copilotUsage",
        default,
        deserialize_with = "jsonl::lenient_object"
    )]
    copilot_usage: Option<CopilotRecordedCredits>,
    #[serde(flatten)]
    tokens: CopilotSessionUsage,
}

#[derive(Debug, Default, Deserialize)]
struct CopilotRecordedCredits {
    #[serde(
        rename = "totalNanoAiu",
        default,
        deserialize_with = "jsonl::lenient_f64"
    )]
    total_nano_aiu: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopilotUsageAccounting {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    source_session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    sequence: u64,
}

#[derive(Debug, Default, Deserialize)]
struct CopilotSessionModelMetrics {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<CopilotSessionUsage>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    requests: Option<CopilotSessionRequests>,
    #[serde(
        rename = "totalNanoAiu",
        default,
        deserialize_with = "jsonl::lenient_f64"
    )]
    total_nano_aiu: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct CopilotSessionRequests {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    count: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopilotSessionUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning_tokens: u64,
}

impl CopilotSessionUsage {
    fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.reasoning_tokens == 0
    }
}

const SHUTDOWN_EVENT: &str = "session.shutdown";
const USAGE_CHECKPOINT_EVENT: &str = "session.usage_checkpoint";
const USAGE_RECORD_EVENT: &str = "session.usage_record";

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

fn is_otel_usage_envelope(value: &Value) -> bool {
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

fn is_session_state_usage_envelope(value: &Value) -> bool {
    CopilotSessionStateEvent::deserialize(value).is_ok_and(|event| event.carries_usage())
}

pub(super) fn parse_session_state_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> Result<CopilotSourceUsage, UnitError> {
    let session_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let Some(session_id) = session_id else {
        return Ok(CopilotSourceUsage::default());
    };

    let prefilter = LinePrefilter::any(&[
        SHUTDOWN_EVENT.as_bytes(),
        USAGE_CHECKPOINT_EVENT.as_bytes(),
        USAGE_RECORD_EVENT.as_bytes(),
    ]);
    let mut parsed = CopilotSourceUsage::default();
    let mut records = Vec::new();
    let mut covered_by_records = false;
    let mut unproven_receipt_coverage = false;
    jsonl::read_records::<CopilotSessionStateEvent>(
        path,
        Some(&prefilter),
        diagnostics,
        is_session_state_usage_envelope,
        |number, event, diagnostics| {
            let timestamp = event.timestamp.as_deref().and_then(parse_ts_timestamp);
            let skip_undated = |carries_usage: bool| {
                if carries_usage {
                    diagnostics.record(ccusage_core::Skip::record(
                        path,
                        number,
                        ccusage_core::SkipKind::InvalidTimestamp,
                        "Copilot usage has no valid timestamp",
                    ));
                }
            };
            let is_shutdown = match event.event_type.as_deref() {
                Some(SHUTDOWN_EVENT) => true,
                Some(USAGE_CHECKPOINT_EVENT) => false,
                Some(USAGE_RECORD_EVENT) => {
                    let Some(timestamp) = timestamp else {
                        skip_undated(event.carries_usage());
                        return;
                    };
                    let record = usage_record(event, timestamp);
                    if let Some(record) = &record {
                        covered_by_records = true;
                        unproven_receipt_coverage |= record
                            .accounting
                            .as_ref()
                            .is_none_or(|(source, _)| source != &session_id);
                    }
                    records.extend(record);
                    return;
                }
                _ => return,
            };
            let carries_usage = event.carries_usage();
            let data = event.data.unwrap_or_default();
            // NOTE(2026-11-16, #2108): Copilot shutdowns and checkpoints can total calls already represented by usage records. Watermarks can refer to receipts in a different session file, so those snapshots also overlap.
            let receipt_watermark =
                data.usage_accounting_watermarks
                    .as_ref()
                    .and_then(|watermarks| {
                        let local = watermarks.get(&session_id)?.as_u64()?;
                        (local > 0
                            && watermarks.iter().all(|(source, sequence)| {
                                source == &session_id || sequence.as_u64() == Some(0)
                            }))
                        .then_some(local)
                    });
            covered_by_records |= data.covers_usage_records();
            if covered_by_records && (receipt_watermark.is_none() || timestamp.is_none()) {
                unproven_receipt_coverage = true;
                return;
            }
            let Some(timestamp) = timestamp else {
                skip_undated(carries_usage);
                return;
            };
            let session_nano_aiu = nano_aiu(data.total_nano_aiu);
            let snapshot_key = |total: Option<u64>| {
                event.id.as_deref().map_or_else(
                    || {
                        format!(
                            "credits:{session_id}:{is_shutdown}:{}:{total:?}",
                            timestamp.as_millis()
                        )
                    },
                    |event_id| format!("credits:{session_id}:{event_id}"),
                )
            };
            if !is_shutdown || receipt_watermark.is_some() {
                if is_shutdown || session_nano_aiu.is_some() {
                    parsed.credit_snapshots.push(CopilotCreditSnapshot {
                        timestamp,
                        session_id: session_id.clone(),
                        is_shutdown,
                        total_nano_aiu: session_nano_aiu,
                        receipt_watermark,
                        dedup_key: snapshot_key(session_nano_aiu),
                    });
                }
                return;
            }
            let mut models_report_credits = true;
            for (logged_model, metrics) in data.model_metrics.unwrap_or_default() {
                let (model, logged_model) = copilot_model_names(logged_model);
                let request_count = metrics
                    .requests
                    .as_ref()
                    .map_or(0, |requests| requests.count);
                let Some(usage) = metrics.usage else {
                    continue;
                };
                if model.is_empty() || usage.is_empty() && request_count == 0 {
                    continue;
                }
                let model_nano_aiu = nano_aiu(metrics.total_nano_aiu);
                models_report_credits &= model_nano_aiu.is_some();
                let timestamp_text = crate::format_rfc3339_millis(timestamp);
                let dedup_key = session_state_dedup_key(
                    &session_id,
                    event.id.as_deref(),
                    &timestamp_text,
                    &model,
                    &usage,
                    request_count,
                );
                parsed.entries.push(CopilotUsageEntry {
                    timestamp,

                    session_id: session_id.clone(),
                    model,
                    logged_model,
                    input_tokens: uncached_session_input_tokens(&usage),
                    output_tokens: usage.output_tokens,
                    cache_creation_tokens: usage.cache_write_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    reasoning_output_tokens: usage.reasoning_tokens,
                    extra_total_tokens: 0,
                    request_count,
                    nano_aiu: model_nano_aiu,
                    dedup_key,
                    call_key: None,
                    call_ids: CallIds::default(),
                });
            }
            let total_nano_aiu = session_nano_aiu.filter(|_| models_report_credits);
            parsed.credit_snapshots.push(CopilotCreditSnapshot {
                timestamp,
                session_id: session_id.clone(),
                is_shutdown,
                total_nano_aiu,
                receipt_watermark: None,
                dedup_key: snapshot_key(total_nano_aiu),
            });
        },
    )?;
    if covered_by_records
        && (unproven_receipt_coverage
            || !parsed
                .credit_snapshots
                .iter()
                .any(|snapshot| snapshot.receipt_watermark.is_some()))
    {
        parsed.credit_snapshots.clear();
    }
    parsed.usage_records = usage_record_entries(records, &session_id);
    Ok(parsed)
}

struct UsageRecordCall {
    timestamp: TimestampMs,
    // NOTE(2026-11-16, #2108): Copilot assigns `(sourceSessionId, sequence)` to a call across session files. Event IDs and line positions only identify a copy within its file.
    accounting: Option<(String, u64)>,
    event_id: Option<String>,
    call_ids: CallIds,
    model: String,
    logged_model: Option<String>,
    usage: CopilotSessionUsage,
    nano_aiu: Option<u64>,
}

fn usage_record(
    event: CopilotSessionStateEvent,
    timestamp: TimestampMs,
) -> Option<UsageRecordCall> {
    let record = event.data?.usage?;
    let (model, logged_model) = copilot_model_names(record.model?);
    let usage = record.tokens;
    let recorded_nano_aiu = nano_aiu(
        record
            .copilot_usage
            .and_then(|credits| credits.total_nano_aiu),
    );
    if model.is_empty() {
        return None;
    }
    let accounting = record.accounting.and_then(|accounting| {
        let source = accounting.source_session_id?;
        (accounting.sequence > 0).then_some((source, accounting.sequence))
    });
    let call_ids = CallIds {
        response_id: record.api_call_id,
        service_request_id: record.service_request_id,
    };
    Some(UsageRecordCall {
        timestamp,
        accounting,
        event_id: event.id,
        call_ids,
        model,
        logged_model,
        usage,
        nano_aiu: recorded_nano_aiu,
    })
}

fn usage_record_entries(
    records: Vec<UsageRecordCall>,
    session_id: &str,
) -> Vec<CopilotUsageRecordEntry> {
    records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let call = record.usage;
            let dedup_key = match &record.accounting {
                Some((source, sequence)) => format!("usage-record:{source}:{sequence}"),
                None => record.event_id.map_or_else(
                    || format!("usage-record:{session_id}:line:{index}"),
                    |event_id| format!("usage-record:{session_id}:event:{event_id}"),
                ),
            };
            CopilotUsageRecordEntry {
                entry: CopilotUsageEntry {
                    timestamp: record.timestamp,
                    session_id: session_id.to_string(),
                    model: record.model,
                    logged_model: record.logged_model,
                    input_tokens: uncached_session_input_tokens(&call),
                    output_tokens: call.output_tokens,
                    cache_creation_tokens: call.cache_write_tokens,
                    cache_read_tokens: call.cache_read_tokens,
                    reasoning_output_tokens: call.reasoning_tokens,
                    extra_total_tokens: 0,
                    request_count: 1,
                    nano_aiu: record.nano_aiu,
                    dedup_key,
                    call_key: None,
                    call_ids: record.call_ids,
                },
                sequence: record.accounting.as_ref().map(|(_, sequence)| *sequence),
                source_session_id: record.accounting.map(|(source, _)| source),
            }
        })
        .collect()
}

fn nano_aiu(value: Option<f64>) -> Option<u64> {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.round() as u64)
}

fn session_state_dedup_key(
    session_id: &str,
    event_id: Option<&str>,
    timestamp_text: &str,
    model: &str,
    usage: &CopilotSessionUsage,
    request_count: u64,
) -> String {
    event_id.map_or_else(
        || {
            format!(
                "shutdown:{session_id}:{timestamp_text}:{model}:{}:{}:{}:{}:{}:{}",
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
                usage.reasoning_tokens,
                request_count,
            )
        },
        |event_id| format!("shutdown:{session_id}:{event_id}:{model}"),
    )
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

fn normalize_copilot_model(model: &str) -> String {
    let model = model.trim();
    model
        .strip_suffix("-1m-internal")
        .or_else(|| model.strip_suffix("-1m"))
        .unwrap_or(model)
        .to_string()
}

fn copilot_model_names(logged_model: String) -> (String, Option<String>) {
    let model = normalize_copilot_model(&logged_model);
    let logged_model = (logged_model != model).then_some(logged_model);
    (model, logged_model)
}

fn first_non_empty_model_attr(attributes: &Map<String, Value>) -> Option<String> {
    MODEL_ATTRS
        .iter()
        .filter_map(|key| attributes.get(*key)?.as_str())
        .find(|model| !model.trim().is_empty())
        .map(str::to_string)
}

fn uncached_session_input_tokens(usage: &CopilotSessionUsage) -> u64 {
    usage.input_tokens.saturating_sub(
        usage
            .cache_read_tokens
            .saturating_add(usage.cache_write_tokens),
    )
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

fn timestamp_from_parts(value: Option<&Value>) -> Option<TimestampMs> {
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

#[cfg(test)]
mod session_state_tests {
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use super::*;

    #[test]
    fn scalar_timestamps_follow_the_shared_epoch_unit_boundary() {
        let fixture = ccusage_test_support::Fixture::new();
        for (raw, expected) in [
            (1_767_312_000_u64, 1_767_312_000_000_i64),
            (10_000_000_000, 10_000_000_000),
            (1_767_312_000_000, 1_767_312_000_000),
            (1_767_312_000_000_000, 1_767_312_000_000),
            (1_767_312_000_000_000_000, 1_767_312_000_000),
        ] {
            let path = fixture.write_file("otel.jsonl", json!({
                "body": "GenAI inference:",
                "timestamp": raw,
                "attributes": {"gen_ai.usage.input_tokens": 10, "gen_ai.usage.output_tokens": 2},
            }).to_string());
            let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());
            let entries = parse_otel_file(&path, &diagnostics).unwrap();
            assert_eq!(entries.len(), 1, "{raw}");
            assert_eq!(entries[0].timestamp.as_millis(), expected, "{raw}");
        }
    }

    #[test]
    fn fixed_unit_timestamps_reject_unsupported_instants() {
        assert_eq!(
            timestamp_from_parts(Some(&json!([10_000_000_000_000_u64, 0]))),
            None
        );
    }

    #[test]
    fn diagnoses_otel_usage_without_a_timestamp() {
        let fixture = ccusage_test_support::fs_fixture!({
            "otel.jsonl": r#"{"body":"GenAI inference:","attributes":{"gen_ai.usage.input_tokens":10,"gen_ai.usage.output_tokens":2,"gen_ai.request.model":"gpt-5"}}"#,
        });
        let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());
        let events = parse_otel_file(&fixture.path("otel.jsonl"), &diagnostics).unwrap();
        assert!(events.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            1
        );
    }

    #[test]
    fn diagnoses_session_state_usage_without_a_timestamp() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session/events.jsonl": r#"{"type":"session.shutdown","data":{"modelMetrics":{"gpt-5":{"usage":{"inputTokens":10,"outputTokens":2}}}}}"#,
        });
        let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());
        let events =
            parse_session_state_file(&fixture.path("session/events.jsonl"), &diagnostics).unwrap();
        assert!(events.entries.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            1
        );
    }

    #[test]
    fn undated_watermark_checkpoint_still_marks_later_shutdowns_as_covered() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session/events.jsonl": concat!(
                r#"{"type":"session.usage_checkpoint","data":{"totalNanoAiu":5,"usageAccountingWatermarks":{"session":1}}}"#,
                "\n",
                r#"{"type":"session.shutdown","timestamp":"2026-02-05T09:00:00.000Z","data":{"modelMetrics":{"gpt-5":{"usage":{"inputTokens":10,"outputTokens":2}}}}}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());

        let parsed =
            parse_session_state_file(&fixture.path("session/events.jsonl"), &diagnostics).unwrap();

        assert!(parsed.entries.is_empty());
        assert!(parsed.credit_snapshots.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            0
        );
    }

    #[test]
    fn confirms_usage_diagnostics_from_token_or_credit_fields() {
        assert!(is_otel_usage_envelope(&json!({
            "attributes": {"gen_ai.usage.input_tokens": 0}
        })));
        assert!(!is_otel_usage_envelope(&json!({
            "attributes": {"gen_ai.request.model": "gpt-5"}
        })));
        assert!(!is_otel_usage_envelope(
            &json!({"attributes": "SECRET_CANARY"})
        ));

        assert!(is_session_state_usage_envelope(&json!({
            "type": "session.shutdown",
            "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 0}}}}
        })));
        assert!(is_session_state_usage_envelope(&json!({
            "type": "session.usage_checkpoint",
            "data": {"totalNanoAiu": 0}
        })));
        assert!(!is_session_state_usage_envelope(&json!({
            "type": "tool",
            "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 1}}}}
        })));
        assert!(!is_session_state_usage_envelope(&json!({
            "type": "session.shutdown",
            "data": {"message": "SECRET_CANARY"}
        })));
        assert!(is_session_state_usage_envelope(&json!({
            "type": "session.usage_record",
            "data": {"usage": {"model": "gpt-5", "inputTokens": 0}}
        })));
        assert!(is_session_state_usage_envelope(&json!({
            "type": "session.usage_record",
            "data": {"usage": {"model": "gpt-5"}}
        })));
        assert!(!is_session_state_usage_envelope(&json!({
            "type": "session.usage_record",
            "data": {"usage": "SECRET_CANARY"}
        })));
        assert!(!is_session_state_usage_envelope(&json!({
            "type": "session.usage_record",
            "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 1}}}}
        })));
    }

    #[test]
    fn reports_a_usage_record_that_does_not_decode_and_keeps_the_next_one() {
        let record = |id: &str| {
            json!({
                "type": "session.usage_record",
                "id": id,
                "timestamp": "2026-02-05T09:00:00.000Z",
                "data": {"usage": {
                    "model": "gpt-5",
                    "inputTokens": 100,
                    "outputTokens": 10,
                    "accounting": {"sourceSessionId": "session-1", "sequence": 1},
                }},
            })
            .to_string()
        };
        let repeated = record("record-1").replacen(
            r#""id":"record-1""#,
            r#""id":"record-1","id":"record-1""#,
            1,
        );
        let fixture = fs_fixture!({
            "session-1/events.jsonl": format!("{repeated}
{}
", record("record-2")),
        });
        let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());

        let parsed =
            parse_session_state_file(&fixture.path("session-1/events.jsonl"), &diagnostics)
                .unwrap();

        assert_eq!(parsed.usage_records.len(), 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::UndecodedUsage),
            0
        );
    }

    #[test]
    fn reports_a_tokenless_usage_record_that_does_not_decode() {
        let record = |id: &str| {
            json!({
                "type": "session.usage_record",
                "id": id,
                "timestamp": "2026-02-05T09:00:00.000Z",
                "data": {"usage": {"model": "gpt-5"}},
            })
            .to_string()
        };
        let repeated = record("record-1").replacen(
            r#""id":"record-1""#,
            r#""id":"record-1","id":"record-1""#,
            1,
        );
        let fixture = fs_fixture!({
            "session-1/events.jsonl": format!("{repeated}
{}
", record("record-2")),
        });
        let diagnostics = Diagnostics::at("copilot", ccusage_core::utc_now());

        let parsed =
            parse_session_state_file(&fixture.path("session-1/events.jsonl"), &diagnostics)
                .unwrap();

        assert_eq!(parsed.usage_records.len(), 1);
        assert_eq!(parsed.usage_records[0].entry.request_count, 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::UndecodedUsage),
            0
        );
    }

    #[test]
    fn parses_session_shutdown_model_metrics() {
        let fixture = fs_fixture!({
            "session-1/events.jsonl": json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "modelMetrics": {
                        "test-model": {
                            "usage": {
                                "inputTokens": 100,
                                "outputTokens": 50,
                                "cacheReadTokens": 10,
                                "cacheWriteTokens": 20,
                                "reasoningTokens": 5
                            }
                        }
                    }
                }
            })
            .to_string(),
        });

        let entries = parse_session_state_file(
            &fixture.path("session-1/events.jsonl"),
            &Diagnostics::at("copilot", ccusage_core::utc_now()),
        )
        .expect("session-state file should parse")
        .entries;

        assert_eq!(entries.len(), 1);
        assert_eq!(
            crate::format_rfc3339_millis(entries[0].timestamp),
            "2026-04-15T09:52:27.352Z"
        );
        assert_eq!(entries[0].session_id, "session-1");
        assert_eq!(entries[0].model, "test-model");
        assert_eq!(entries[0].input_tokens, 70);
        assert_eq!(entries[0].output_tokens, 50);
        assert_eq!(entries[0].cache_creation_tokens, 20);
        assert_eq!(entries[0].cache_read_tokens, 10);
        assert_eq!(entries[0].reasoning_output_tokens, 5);
        assert_eq!(
            entries[0].dedup_key,
            "shutdown:session-1:shutdown-1:test-model"
        );
    }

    #[test]
    fn normalizes_copilot_internal_model_ids() {
        assert_eq!(
            normalize_copilot_model(" claude-opus-4.7-1m-internal "),
            "claude-opus-4.7"
        );
        assert_eq!(
            normalize_copilot_model("claude-opus-4.6-1m"),
            "claude-opus-4.6"
        );
        assert_eq!(normalize_copilot_model("gpt-5.4"), "gpt-5.4");
    }

    #[test]
    fn skips_malformed_lines_and_non_shutdown_events() {
        let fixture = fs_fixture!({
            "session-1/events.jsonl": format!(
                "not json\n{}\n{}",
                json!({
                    "type": "tool",
                    "data": {"message": "session.shutdown"}
                }),
                json!({
                    "type": "session.shutdown",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {
                        "modelMetrics": {
                            "test-model": {
                                "usage": {
                                    "inputTokens": 1,
                                    "outputTokens": 2
                                }
                            }
                        }
                    }
                })
            ),
        });

        let entries = parse_session_state_file(
            &fixture.path("session-1/events.jsonl"),
            &Diagnostics::at("copilot", ccusage_core::utc_now()),
        )
        .unwrap()
        .entries;

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].input_tokens, 1);
        assert_eq!(entries[0].output_tokens, 2);
        assert!(
            entries[0]
                .dedup_key
                .starts_with("shutdown:session-1:2026-04-15T09:52:27.352Z:test-model:")
        );
    }

    #[test]
    fn parses_each_model_and_ignores_request_cost_and_empty_usage() {
        let fixture = fs_fixture!({
            "session-1/events.jsonl": json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "modelMetrics": {
                        "first-model": {
                            "usage": {
                                "inputTokens": 10,
                                "outputTokens": 20
                            },
                            "requests": {"count": 1, "cost": 999}
                        },
                        "empty-model": {
                            "usage": {
                                "inputTokens": 0,
                                "outputTokens": 0
                            }
                        },
                        "second-model": {
                            "usage": {
                                "cacheReadTokens": 3,
                                "cacheWriteTokens": 4
                            }
                        }
                    }
                }
            })
            .to_string(),
        });

        let entries = parse_session_state_file(
            &fixture.path("session-1/events.jsonl"),
            &Diagnostics::at("copilot", ccusage_core::utc_now()),
        )
        .unwrap()
        .entries;

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.model.as_str())
                .collect::<Vec<_>>(),
            ["first-model", "second-model"]
        );
        assert_eq!(entries[1].cache_creation_tokens, 4);
        assert_eq!(entries[1].cache_read_tokens, 3);
        assert_eq!(entries[0].request_count, 1);
    }

    #[test]
    fn parses_credit_totals_from_checkpoints_and_shutdowns() {
        let fixture = fs_fixture!({
            "session-1/events.jsonl": [
                json!({
                    "type": "session.usage_checkpoint",
                    "id": "checkpoint-1",
                    "timestamp": "2026-04-15T09:50:00.000Z",
                    "data": {"totalNanoAiu": 51_322_830_000_u64, "promptCacheBreakState": []}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {
                        "totalNanoAiu": 93_524_970_000.0,
                        "modelMetrics": {
                            "test-model": {
                                "usage": {"inputTokens": 100, "outputTokens": 50},
                                "totalNanoAiu": 42_202_140_000.0
                            }
                        }
                    }
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-2",
                    "timestamp": "2026-04-16T09:00:00.000Z",
                    "data": {"totalNanoAiu": 93_524_970_000_u64, "modelMetrics": {}}
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let parsed = parse_session_state_file(
            &fixture.path("session-1/events.jsonl"),
            &Diagnostics::at("copilot", ccusage_core::utc_now()),
        )
        .unwrap();

        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].nano_aiu, Some(42_202_140_000));
        assert_eq!(
            parsed
                .credit_snapshots
                .iter()
                .map(|snapshot| (
                    snapshot.dedup_key.as_str(),
                    snapshot.is_shutdown,
                    snapshot.total_nano_aiu
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "credits:session-1:checkpoint-1",
                    false,
                    Some(51_322_830_000)
                ),
                ("credits:session-1:shutdown-1", true, Some(93_524_970_000)),
                ("credits:session-1:shutdown-2", true, Some(93_524_970_000)),
            ]
        );
    }

    #[test]
    fn marks_shutdown_credits_unknown_when_a_total_is_missing() {
        let fixture = fs_fixture!({
            "session-1/events.jsonl": [
                json!({
                    "type": "session.shutdown",
                    "id": "without-model-total",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {
                        "totalNanoAiu": 93_524_970_000_u64,
                        "modelMetrics": {
                            "test-model": {"usage": {"inputTokens": 100, "outputTokens": 50}}
                        }
                    }
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "without-session-total",
                    "timestamp": "2026-04-16T09:52:27.352Z",
                    "data": {"modelMetrics": {}}
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let parsed = parse_session_state_file(
            &fixture.path("session-1/events.jsonl"),
            &Diagnostics::at("copilot", ccusage_core::utc_now()),
        )
        .unwrap();

        assert_eq!(
            parsed
                .credit_snapshots
                .iter()
                .map(|snapshot| snapshot.total_nano_aiu)
                .collect::<Vec<_>>(),
            [None, None]
        );
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let model = normalize_copilot_model(" claude-opus-4.7-1m-internal ");
        assert_eq!(model, "claude-opus-4.7");
        assert_eq!(normalize_copilot_model("gpt-5.2"), "gpt-5.2");
    }
}
