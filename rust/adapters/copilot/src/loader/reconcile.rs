use std::collections::HashMap;

use crate::TimestampMs;
use crate::parser::{CallIdKind, CallIds, CopilotUsageEntry, CopilotUsageRecordEntry};

// NOTE(2026-11-16, #2108): A Copilot service request ID names one request, while a VS Code response ID is shared across the turn. Match service request IDs first to preserve the unambiguous pairs.
pub(super) fn recorded_otel_calls(
    otel_entries: &[CopilotUsageEntry],
    records: &[CopilotUsageRecordEntry],
) -> HashMap<usize, bool> {
    if records.is_empty() || otel_entries.is_empty() {
        return HashMap::new();
    }
    let mut rows_by_call_id = HashMap::<(CallIdKind, &str), Vec<usize>>::new();
    for (index, entry) in otel_entries.iter().enumerate() {
        for call_id in entry.call_ids.iter() {
            rows_by_call_id.entry(call_id).or_default().push(index);
        }
    }
    let mut records = records
        .iter()
        .map(|record| &record.entry)
        .collect::<Vec<_>>();
    records.sort_by_key(|record| (record.timestamp, record.dedup_key.as_str()));
    let mut matched = vec![false; records.len()];
    let mut replaced = HashMap::new();
    for kind in [CallIdKind::ServiceRequest, CallIdKind::Response] {
        for (position, record) in records.iter().enumerate() {
            if matched[position] {
                continue;
            }
            let nearest = record
                .call_ids
                .iter()
                .filter(|(id_kind, _)| *id_kind == kind)
                .filter_map(|call_id| rows_by_call_id.get(&call_id))
                .flatten()
                .copied()
                .filter(|index| {
                    otel_entries[*index].model == record.model && !replaced.contains_key(index)
                })
                .min_by_key(|index| {
                    let row = otel_entries[*index].timestamp.as_millis();
                    (row.abs_diff(record.timestamp.as_millis()), row, *index)
                });
            if let Some(index) = nearest {
                replaced.insert(index, otel_entries[index].session_id == record.session_id);
                matched[position] = true;
            }
        }
    }
    replaced
}

pub(super) fn deduplicate_usage_records(
    records: Vec<CopilotUsageRecordEntry>,
) -> Vec<CopilotUsageRecordEntry> {
    let rank = |record: &CopilotUsageRecordEntry| {
        (
            record.source_session_id.as_deref() != Some(record.entry.session_id.as_str()),
            record.entry.session_id.clone(),
        )
    };
    let mut positions = HashMap::<String, usize>::new();
    let mut kept = Vec::<CopilotUsageRecordEntry>::new();
    for record in records {
        match positions.get(&record.entry.dedup_key) {
            Some(&position) => {
                if rank(&record) < rank(&kept[position]) {
                    kept[position] = record;
                }
            }
            None => {
                positions.insert(record.entry.dedup_key.clone(), kept.len());
                kept.push(record);
            }
        }
    }
    kept
}

pub(super) struct SessionStateReconciliation {
    pub(super) entries: Vec<CopilotUsageEntry>,
    pub(super) otel_after: HashMap<(String, String), OtelCutoff>,
    pub(super) attributed_nano_aiu: HashMap<(String, i64), u64>,
}

pub(super) struct OtelCutoff {
    pub(super) visible: Option<TimestampMs>,
    pub(super) full: Option<TimestampMs>,
}

pub(super) fn otel_call_timestamps(
    entries: &[CopilotUsageEntry],
) -> HashMap<(String, String), Vec<TimestampMs>> {
    let mut calls = HashMap::<(String, String), Vec<TimestampMs>>::new();
    for entry in entries {
        calls
            .entry((entry.session_id.clone(), entry.model.clone()))
            .or_default()
            .push(entry.timestamp);
    }
    for timestamps in calls.values_mut() {
        timestamps.sort_unstable();
    }
    calls
}

// NOTE(2026-10-10, #1858): Copilot restores the last shutdown's breakdown when resuming a session. In shutdown-only sessions, OpenTelemetry can therefore contain calls missing from every shutdown.
fn otel_replacement_start(
    entries: &[CopilotUsageEntry],
    sorted: &[usize],
    calls: &[TimestampMs],
) -> Option<usize> {
    if sorted
        .iter()
        .any(|index| entries[*index].request_count == 0)
    {
        return None;
    }
    let mut start = None;
    let mut counted_calls = 0;
    let mut otel_calls = 0_u64;
    let mut reported_calls = 0_u64;
    let mut previous_requests = 0;
    for (position, index) in sorted.iter().enumerate() {
        let entry = &entries[*index];
        let calls_through_shutdown = calls.partition_point(|call| *call <= entry.timestamp);
        let interval_calls = calls_through_shutdown - counted_calls;
        counted_calls = calls_through_shutdown;
        if start.is_none() && interval_calls > 0 {
            start = Some(position);
        }
        if start.is_some() {
            otel_calls += interval_calls as u64;
            reported_calls += entry.request_count.saturating_sub(previous_requests);
        }
        previous_requests = entry.request_count;
    }
    start.filter(|_| otel_calls > reported_calls)
}

pub(super) fn reconcile_session_state_entries(
    entries: Vec<CopilotUsageEntry>,
    otel_calls: &HashMap<(String, String), Vec<TimestampMs>>,
    since_millis: Option<i64>,
    until_millis: Option<i64>,
) -> SessionStateReconciliation {
    let entries = deduplicate_session_entries(entries);
    let mut grouped = HashMap::<(String, String), Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        grouped
            .entry((entry.session_id.clone(), entry.model.clone()))
            .or_default()
            .push(index);
    }
    let mut interval_indices = Vec::new();
    let mut otel_after = HashMap::new();
    let mut attributed_nano_aiu = HashMap::<(String, i64), u64>::new();
    let mut keys = grouped.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        let mut sorted = grouped.remove(&key).unwrap_or_default();
        sorted.sort_by_key(|index| (entries[*index].timestamp, *index));
        let latest_visible = sorted.iter().rposition(|index| {
            until_millis.is_none_or(|end| entries[*index].timestamp.as_millis() < end)
        });
        let otel_start = otel_calls
            .get(&key)
            .and_then(|calls| otel_replacement_start(&entries, &sorted, calls));
        let otel_after_position = match otel_start {
            Some(start) => start.checked_sub(1),
            None => latest_visible,
        };
        let full = match otel_start {
            Some(start) => start.checked_sub(1),
            None => sorted.len().checked_sub(1),
        };
        otel_after.insert(
            key,
            OtelCutoff {
                visible: otel_after_position.map(|position| entries[sorted[position]].timestamp),
                full: full.map(|position| entries[sorted[position]].timestamp),
            },
        );
        let mut previous: Option<&CopilotUsageEntry> = None;
        for (position, index) in sorted.iter().enumerate() {
            let current = &entries[*index];
            let reconciled = previous.map_or_else(
                || current.clone(),
                |baseline| subtract_usage(current, baseline),
            );
            previous = Some(current);
            *attributed_nano_aiu
                .entry((current.session_id.clone(), current.timestamp.as_millis()))
                .or_default() += reconciled.nano_aiu.unwrap_or_default();
            if latest_visible.is_none_or(|latest_pos| position > latest_pos)
                || otel_start.is_some_and(|start| position >= start)
                || since_millis.is_some_and(|start| current.timestamp.as_millis() < start)
            {
                continue;
            }
            if has_usage(&reconciled) {
                interval_indices.push(reconciled);
            }
        }
    }
    interval_indices.sort_by(|left, right| {
        (left.timestamp, left.dedup_key.as_str()).cmp(&(right.timestamp, right.dedup_key.as_str()))
    });
    SessionStateReconciliation {
        entries: interval_indices,
        otel_after,
        attributed_nano_aiu,
    }
}

fn deduplicate_session_entries(entries: Vec<CopilotUsageEntry>) -> Vec<CopilotUsageEntry> {
    let mut indexes = HashMap::<String, usize>::new();
    for (index, entry) in entries.iter().enumerate() {
        if indexes
            .get(&entry.dedup_key)
            .is_none_or(|previous| entries[*previous].timestamp <= entry.timestamp)
        {
            indexes.insert(entry.dedup_key.clone(), index);
        }
    }
    let mut indexes = indexes.into_values().collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes
        .into_iter()
        .map(|index| entries[index].clone())
        .collect()
}

fn subtract_usage(current: &CopilotUsageEntry, baseline: &CopilotUsageEntry) -> CopilotUsageEntry {
    CopilotUsageEntry {
        timestamp: current.timestamp,
        session_id: current.session_id.clone(),
        model: current.model.clone(),
        logged_model: current.logged_model.clone(),
        input_tokens: current.input_tokens.saturating_sub(baseline.input_tokens),
        output_tokens: current.output_tokens.saturating_sub(baseline.output_tokens),
        cache_creation_tokens: current
            .cache_creation_tokens
            .saturating_sub(baseline.cache_creation_tokens),
        cache_read_tokens: current
            .cache_read_tokens
            .saturating_sub(baseline.cache_read_tokens),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(baseline.reasoning_output_tokens),
        extra_total_tokens: current
            .extra_total_tokens
            .saturating_sub(baseline.extra_total_tokens),
        request_count: current.request_count.saturating_sub(baseline.request_count),
        nano_aiu: current
            .nano_aiu
            .zip(baseline.nano_aiu)
            .map(|(current, baseline)| current.saturating_sub(baseline)),
        dedup_key: current.dedup_key.clone(),
        call_ids: CallIds::default(),
    }
}

fn has_usage(entry: &CopilotUsageEntry) -> bool {
    entry.input_tokens > 0
        || entry.output_tokens > 0
        || entry.cache_creation_tokens > 0
        || entry.cache_read_tokens > 0
        || entry.reasoning_output_tokens > 0
        || entry.extra_total_tokens > 0
        || entry.request_count > 0
        || entry.nano_aiu.is_some_and(|nano_aiu| nano_aiu > 0)
}
