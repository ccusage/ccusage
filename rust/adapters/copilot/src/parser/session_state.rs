use std::{collections::BTreeMap, path::Path};

use serde::Deserialize;
use serde_json::Value;

use super::{
    CallIds, CopilotCreditSnapshot, CopilotSourceUsage, CopilotUsageEntry, CopilotUsageRecordEntry,
    copilot_model_names,
};
use crate::{TimestampMs, fast::LinePrefilter, parse_ts_timestamp};
use ccusage_adapter_common::{jsonl, units::UnitError};
use ccusage_core::Diagnostics;

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

pub(super) fn is_session_state_usage_envelope(value: &Value) -> bool {
    CopilotSessionStateEvent::deserialize(value).is_ok_and(|event| event.carries_usage())
}

pub(crate) fn parse_session_state_file(
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
                    fallback_output_tokens: 0,
                    request_count,
                    nano_aiu: model_nano_aiu,
                    dedup_key,
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
                    fallback_output_tokens: 0,
                    request_count: 1,
                    nano_aiu: record.nano_aiu,
                    dedup_key,
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

fn uncached_session_input_tokens(usage: &CopilotSessionUsage) -> u64 {
    usage.input_tokens.saturating_sub(
        usage
            .cache_read_tokens
            .saturating_add(usage.cache_write_tokens),
    )
}
