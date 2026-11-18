use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    Result, TimestampMs, TokenUsageRaw, apply_total_token_fallback, fast::LinePrefilter,
    parse_ts_timestamp,
};
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
    pub(super) extra_total_tokens: u64,
    pub(super) request_count: u64,
    /// Credits Copilot recorded for this entry, in nano-AIU: per call for a
    /// usage record, cumulative per model for a shutdown. `None` when the
    /// source has no credits, which leaves the cost to token pricing.
    pub(super) nano_aiu: Option<u64>,
    pub(super) dedup_key: String,
    /// Whether `dedup_key` names the same call in every file that holds it.
    pub(super) file_independent_key: bool,
    pub(super) call_ids: CallIds,
}

/// IDs of a single call that its usage record and its OpenTelemetry rows
/// share. The two kinds are separate namespaces and only match their own kind.
#[derive(Debug, Clone, Default)]
pub(super) struct CallIds {
    /// Record `apiCallId`, OTel `gen_ai.response.id`: the provider completion ID.
    response_id: Option<String>,
    /// Record `serviceRequestId`, OTel `github.copilot.service_request_id`.
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

/// Session-wide credit total from a `session.shutdown` or
/// `session.usage_checkpoint` event, in nano-AIU. One AIU is one GitHub AI
/// credit, billed at $0.01.
#[derive(Debug, Clone)]
pub(super) struct CopilotCreditSnapshot {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: String,
    pub(super) is_shutdown: bool,
    /// `None` for a shutdown whose session total or per-model totals are
    /// missing, which makes its credits impossible to reconcile.
    pub(super) total_nano_aiu: Option<u64>,
    pub(super) dedup_key: String,
}

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageRecordEntry {
    pub(super) entry: CopilotUsageEntry,
    /// `accounting.sourceSessionId`: the session that assigned the call's
    /// accounting sequence. `None` when the record has no usable identity.
    pub(super) source_session_id: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct CopilotSourceUsage {
    pub(super) entries: Vec<CopilotUsageEntry>,
    /// One entry per `session.usage_record` call.
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
    session_id: String,
    timestamp: TimestampMs,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_output_tokens: u64,
    extra_total_tokens: u64,
    dedup_key: String,
    file_independent_key: bool,
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
        .enumerate()
        .filter(|(index, candidate)| should_emit_candidate(*index, candidate, &sets))
        .map(|(_, candidate)| CopilotUsageEntry {
            timestamp: candidate.timestamp,
            timestamp_text: crate::format_rfc3339_millis(candidate.timestamp),
            session_id: candidate.session_id,
            model: candidate.model,
            input_tokens: candidate.input_tokens,
            output_tokens: candidate.output_tokens,
            cache_creation_tokens: candidate.cache_creation_tokens,
            cache_read_tokens: candidate.cache_read_tokens,
            reasoning_output_tokens: candidate.reasoning_output_tokens,
            extra_total_tokens: candidate.extra_total_tokens,
            request_count: 1,
            nano_aiu: None,
            dedup_key: candidate.dedup_key,
            file_independent_key: candidate.file_independent_key,
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
    /// Watermarks name the usage records a snapshot's totals include, per
    /// source session, wherever those records are stored.
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

/// One model call from `session.usage_record`. Its token fields have the same
/// meaning as a `modelMetrics` usage, which sums them.
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

pub(super) fn parse_session_state_file(path: &Path) -> Result<CopilotSourceUsage> {
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

    let content = fs::read(path)?;
    let prefilter = LinePrefilter::any(&[
        SHUTDOWN_EVENT.as_bytes(),
        USAGE_CHECKPOINT_EVENT.as_bytes(),
        USAGE_RECORD_EVENT.as_bytes(),
    ]);
    let mut parsed = CopilotSourceUsage::default();
    let mut records = Vec::new();
    let mut covered_by_records = false;
    for event in jsonl::records::<CopilotSessionStateEvent>(&content, Some(&prefilter)) {
        let is_shutdown = match event.event_type.as_deref() {
            Some(SHUTDOWN_EVENT) => true,
            Some(USAGE_CHECKPOINT_EVENT) => false,
            Some(USAGE_RECORD_EVENT) => {
                let record = usage_record(event);
                covered_by_records |= record.is_some();
                records.extend(record);
                continue;
            }
            _ => continue,
        };
        let data = event.data.unwrap_or_default();
        // Shutdowns and checkpoints written after a usage record, or by a
        // process that reports watermarks, only total calls that usage records
        // report, in this file or in the file of the session they belong to.
        covered_by_records |= data.covers_usage_records();
        if covered_by_records {
            continue;
        }
        let Some(timestamp_text) = event.timestamp.as_deref() else {
            continue;
        };
        let Some(timestamp) = parse_ts_timestamp(timestamp_text) else {
            continue;
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
        if !is_shutdown {
            // A checkpoint without a total carries nothing to reconcile.
            if let Some(total) = session_nano_aiu {
                parsed.credit_snapshots.push(CopilotCreditSnapshot {
                    timestamp,
                    session_id: session_id.clone(),
                    is_shutdown,
                    total_nano_aiu: Some(total),
                    dedup_key: snapshot_key(Some(total)),
                });
            }
            continue;
        }
        let mut models_report_credits = true;
        for (model, metrics) in data.model_metrics.unwrap_or_default() {
            let model = normalize_copilot_model(&model);
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
                timestamp_text,
                session_id: session_id.clone(),
                model,
                input_tokens: uncached_session_input_tokens(&usage),
                output_tokens: usage.output_tokens,
                cache_creation_tokens: usage.cache_write_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                reasoning_output_tokens: usage.reasoning_tokens,
                extra_total_tokens: 0,
                request_count,
                nano_aiu: model_nano_aiu,
                dedup_key,
                file_independent_key: false,
                call_ids: CallIds::default(),
            });
        }
        let total_nano_aiu = session_nano_aiu.filter(|_| models_report_credits);
        parsed.credit_snapshots.push(CopilotCreditSnapshot {
            timestamp,
            session_id: session_id.clone(),
            is_shutdown,
            total_nano_aiu,
            dedup_key: snapshot_key(total_nano_aiu),
        });
    }
    if covered_by_records {
        // Usage records report every call with its own credits or tokens, so
        // the session-wide credit totals have nothing left to explain.
        parsed.credit_snapshots.clear();
    }
    parsed.usage_records = usage_record_entries(records, &session_id);
    Ok(parsed)
}

struct UsageRecordCall {
    timestamp: TimestampMs,
    /// `(sourceSessionId, sequence)`, unique across every session file.
    accounting: Option<(String, u64)>,
    event_id: Option<String>,
    call_ids: CallIds,
    model: String,
    usage: CopilotSessionUsage,
    nano_aiu: Option<u64>,
}

fn usage_record(event: CopilotSessionStateEvent) -> Option<UsageRecordCall> {
    let timestamp = parse_ts_timestamp(event.timestamp.as_deref()?)?;
    let record = event.data?.usage?;
    let model = normalize_copilot_model(record.model.as_deref()?);
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
            // An accounting identity names the call in every file it appears
            // in; an event ID or line position names it only in this file.
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
                    timestamp_text: crate::format_rfc3339_millis(record.timestamp),
                    session_id: session_id.to_string(),
                    model: record.model,
                    input_tokens: uncached_session_input_tokens(&call),
                    output_tokens: call.output_tokens,
                    cache_creation_tokens: call.cache_write_tokens,
                    cache_read_tokens: call.cache_read_tokens,
                    reasoning_output_tokens: call.reasoning_tokens,
                    extra_total_tokens: 0,
                    request_count: 1,
                    nano_aiu: record.nano_aiu,
                    dedup_key,
                    file_independent_key: false,
                    call_ids: record.call_ids,
                },
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
    // Copilot counts cache reads and writes in the input tokens, as it does in
    // session-state `inputTokens`.
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
    let model = first_non_empty_model_attr(attributes)
        .or_else(|| trace_context.and_then(|context| context.model.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let session_id = best_session_attr(attributes)
        .map(|(session_id, _)| session_id)
        .or_else(|| trace_context.and_then(|context| context.session_id.clone()))
        .or_else(|| trace_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let timestamp = timestamp_from_record(record).unwrap_or(fallback_timestamp);
    let (dedup_key, file_independent_key) = dedup_key_for_record(
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
        session_id,
        timestamp,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: reasoning,
        extra_total_tokens,
        dedup_key,
        file_independent_key,
    })
}

struct CandidateSets {
    chat_traces: HashSet<String>,
    inference_traces: HashSet<String>,
    agent_turn_traces: HashSet<String>,
    chat_response_ids: HashSet<String>,
    inference_response_ids: HashSet<String>,
    agent_turn_response_ids: HashSet<String>,
    repeated_chat_spans: HashSet<usize>,
}

impl CandidateSets {
    fn new(candidates: &[CopilotUsageCandidate]) -> Self {
        Self {
            repeated_chat_spans: repeated_chat_spans(candidates),
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

/// The Copilot CLI exports some chat spans a second time, under a new trace or
/// span ID but with the response ID and usage of the original call. VS Code
/// reports one response ID on every call of a turn, so the usage tells calls
/// apart. Returns the chat spans that repeat an earlier one.
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
        CopilotUsageSource::ChatSpan => !sets.repeated_chat_spans.contains(&index),
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

fn normalize_copilot_model(model: &str) -> String {
    let model = model.trim();
    model
        .strip_suffix("-1m-internal")
        .or_else(|| model.strip_suffix("-1m"))
        .unwrap_or(model)
        .to_string()
}

fn first_non_empty_model_attr(attributes: &Map<String, Value>) -> Option<String> {
    first_non_empty_attr(attributes, MODEL_ATTRS).map(|model| normalize_copilot_model(&model))
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

/// Returns the row's key and whether it comes from IDs the record carries.
/// Only such a key names the same call in another file; the fallbacks use the
/// row's line and time, which can come from the file's modification time.
fn dedup_key_for_record(
    source: CopilotUsageSource,
    record: &CopilotRecord,
    attributes: &Map<String, Value>,
    trace_id: &Option<String>,
    session_id: &str,
    timestamp: TimestampMs,
    index: usize,
) -> (String, bool) {
    let span_id = span_id_from_record(record);
    let record_key = match source {
        CopilotUsageSource::ChatSpan | CopilotUsageSource::AgentSummarySpan => trace_id
            .as_ref()
            .zip(span_id.as_ref())
            .map(|(trace_id, span_id)| format!("{trace_id}:{span_id}")),
        CopilotUsageSource::InferenceLog => trace_id
            .as_ref()
            .zip(span_id.as_ref())
            .map(|(trace_id, span_id)| format!("log:{trace_id}:{span_id}")),
        CopilotUsageSource::AgentTurnLog => trace_id
            .as_ref()
            .zip(turn_index(attributes))
            .map(|(trace_id, turn_index)| format!("agent-turn:{trace_id}:{turn_index}")),
    };
    if let Some(key) = record_key {
        return (key, true);
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
    (key, false)
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

#[cfg(test)]
mod session_state_tests {
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use super::*;

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

        let entries = parse_session_state_file(&fixture.path("session-1/events.jsonl"))
            .expect("session-state file should parse")
            .entries;

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp_text, "2026-04-15T09:52:27.352Z");
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

        let entries = parse_session_state_file(&fixture.path("session-1/events.jsonl"))
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

        let entries = parse_session_state_file(&fixture.path("session-1/events.jsonl"))
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

        let parsed = parse_session_state_file(&fixture.path("session-1/events.jsonl")).unwrap();

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

        let parsed = parse_session_state_file(&fixture.path("session-1/events.jsonl")).unwrap();

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
