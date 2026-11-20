use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        CallIdKind, CallIds, CallKey, CopilotCreditSnapshot, CopilotSourceUsage, CopilotUsageEntry,
        CopilotUsageRecordEntry, parse_otel_file, parse_session_state_file,
    },
    paths::CopilotSourceKind,
};
use ccusage_adapter_common::{
    source::LoadContext,
    units::{UnitError, read_typed_units},
};
use ccusage_core::Diagnostics;

use crate::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, stored_cost_for_mode,
};

pub(crate) fn load_entries_for_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let sources = super::paths::paths(cx.diagnostics)?;
    let parsed = read_typed_units(&sources, shared.single_thread, cx.diagnostics, |source| {
        read_source_file(&source.path, source.kind, cx.diagnostics)
    });
    let mut otel_entries = Vec::new();
    let mut otel_keys = HashSet::<CallKey>::new();
    let mut session_state_entries = Vec::new();
    let mut usage_records = Vec::new();
    let mut credit_snapshots = Vec::new();
    for (source, file_usage) in sources.iter().map(|source| source.kind).zip(parsed) {
        let Some(file_usage) = file_usage else {
            continue;
        };
        match source {
            CopilotSourceKind::Otel => {
                let new_rows = file_usage
                    .entries
                    .into_iter()
                    .filter(|entry| {
                        entry
                            .call_key
                            .as_ref()
                            .is_none_or(|key| !otel_keys.contains(key))
                    })
                    .collect::<Vec<_>>();
                otel_keys.extend(new_rows.iter().filter_map(|entry| entry.call_key.clone()));
                otel_entries.extend(new_rows);
            }
            CopilotSourceKind::SessionState => {
                session_state_entries.extend(file_usage.entries);
                usage_records.extend(file_usage.usage_records);
                credit_snapshots.extend(file_usage.credit_snapshots);
            }
        }
    }
    let (since_millis, until_millis) = cx.window.bounds_ms();
    let session_state = reconcile_session_state_entries(
        session_state_entries,
        &otel_call_timestamps(&otel_entries),
        since_millis,
        until_millis,
    );
    let otel_after = session_state
        .otel_after
        .iter()
        .map(|((session_id, model), cutoff)| ((session_id.as_str(), model.as_str()), cutoff))
        .collect::<HashMap<_, _>>();
    let survives_shutdown = |entry: &CopilotUsageEntry, in_report| {
        otel_after
            .get(&(entry.session_id.as_str(), entry.model.as_str()))
            .and_then(|cutoff| {
                if in_report {
                    cutoff.visible
                } else {
                    cutoff.full
                }
            })
            .is_none_or(|after| entry.timestamp > after)
    };
    let usage_records = deduplicate_usage_records(usage_records);
    let matches = recorded_otel_calls(&otel_entries, &usage_records);
    let credit_gaps = if credit_snapshots
        .iter()
        .any(|snapshot| !snapshot.is_shutdown)
    {
        let uncovered = otel_entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                survives_shutdown(entry, false) && matches.get(index) != Some(&true)
            })
            .fold(
                HashMap::<&str, TimestampMs>::new(),
                |mut earliest, (_, entry)| {
                    earliest
                        .entry(entry.session_id.as_str())
                        .and_modify(|timestamp| *timestamp = (*timestamp).min(entry.timestamp))
                        .or_insert(entry.timestamp);
                    earliest
                },
            );
        let receipts = receipt_ledgers(&credit_snapshots, &usage_records);
        credit_gap_entries(
            credit_snapshots,
            &session_state.attributed_nano_aiu,
            &uncovered,
            &receipts,
            since_millis,
            until_millis,
        )
    } else {
        Vec::new()
    };
    let visible_otel = otel_entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (survives_shutdown(&entry, true) && !matches.contains_key(&index)).then_some(entry)
        });

    let mut entries = session_state
        .entries
        .into_iter()
        .chain(usage_records.into_iter().map(|record| record.entry))
        .chain(visible_otel)
        .chain(credit_gaps)
        .map(|entry| usage_entry_to_loaded(entry, tz, shared.mode, pricing))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

// NOTE(2026-11-16, #2108): A Copilot service request ID names one request, while a VS Code response ID is shared across the turn. Match service request IDs first to preserve the unambiguous pairs.
fn recorded_otel_calls(
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

fn deduplicate_usage_records(
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

struct SessionStateReconciliation {
    entries: Vec<CopilotUsageEntry>,
    otel_after: HashMap<(String, String), OtelCutoff>,
    attributed_nano_aiu: HashMap<(String, i64), u64>,
}

struct OtelCutoff {
    visible: Option<TimestampMs>,
    full: Option<TimestampMs>,
}

fn otel_call_timestamps(
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

fn reconcile_session_state_entries(
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
    interval_indices.sort_by_key(|entry| (entry.timestamp, entry.dedup_key.clone()));
    SessionStateReconciliation {
        entries: interval_indices,
        otel_after,
        attributed_nano_aiu,
    }
}

struct ReceiptCredit {
    timestamp: TimestampMs,
    sequence: u64,
    nano_aiu: Option<u64>,
}

fn receipt_ledgers<'a>(
    snapshots: &[CopilotCreditSnapshot],
    records: &'a [CopilotUsageRecordEntry],
) -> HashMap<&'a str, Vec<ReceiptCredit>> {
    let sessions = snapshots
        .iter()
        .filter(|snapshot| snapshot.receipt_watermark.is_some())
        .map(|snapshot| snapshot.session_id.as_str())
        .collect::<HashSet<_>>();
    let mut ledgers = HashMap::<_, Vec<ReceiptCredit>>::new();
    for record in records {
        let session = record.entry.session_id.as_str();
        if !sessions.contains(session) || record.source_session_id.as_deref() != Some(session) {
            continue;
        }
        if let Some(sequence) = record.sequence {
            ledgers.entry(session).or_default().push(ReceiptCredit {
                timestamp: record.entry.timestamp,
                sequence,
                nano_aiu: record.entry.nano_aiu,
            });
        }
    }
    for calls in ledgers.values_mut() {
        calls.sort_unstable_by_key(|call| call.sequence);
        let mut total = Some(0_u64);
        for (index, call) in calls.iter_mut().enumerate() {
            total = total
                .zip(call.nano_aiu)
                .filter(|_| call.sequence == index as u64 + 1)
                .and_then(|(previous, amount)| previous.checked_add(amount));
            call.nano_aiu = total;
        }
    }
    ledgers
}

fn covered_receipt_credits(
    snapshots: &[CopilotCreditSnapshot],
    calls: &[ReceiptCredit],
) -> Option<Vec<u64>> {
    if calls.is_empty() || !calls.is_sorted_by_key(|call| call.timestamp) {
        return None;
    }
    let mut previous_watermark = 0;
    let mut previous_total = 0;
    snapshots
        .iter()
        .map(|snapshot| {
            let Some(watermark) = snapshot.receipt_watermark else {
                return (previous_watermark == 0 && snapshot.timestamp < calls[0].timestamp)
                    .then_some(0);
            };
            let total = snapshot.total_nano_aiu?;
            let index = usize::try_from(watermark).ok()?.checked_sub(1)?;
            let call = calls.get(index)?;
            if watermark < previous_watermark
                || total < previous_total
                || call.timestamp > snapshot.timestamp
                || calls
                    .get(index + 1)
                    .is_some_and(|next| next.timestamp <= snapshot.timestamp)
            {
                return None;
            }
            previous_watermark = watermark;
            previous_total = total;
            call.nano_aiu.filter(|amount| *amount <= total)
        })
        .collect()
}

const UNATTRIBUTED_MODEL: &str = "unknown";

// NOTE(2026-10-10, #1824): Copilot session `totalNanoAiu` covers the whole session while `modelMetrics` can omit the earlier process. Older versions without checkpoints restart both totals on resume, so the difference is only useful for sessions with checkpoints.
fn credit_gap_entries(
    snapshots: Vec<CopilotCreditSnapshot>,
    attributed_nano_aiu: &HashMap<(String, i64), u64>,
    uncovered_otel: &HashMap<&str, TimestampMs>,
    receipt_ledgers: &HashMap<&str, Vec<ReceiptCredit>>,
    since_millis: Option<i64>,
    until_millis: Option<i64>,
) -> Vec<CopilotUsageEntry> {
    let mut seen = HashSet::new();
    let mut sessions = BTreeMap::<String, Vec<CopilotCreditSnapshot>>::new();
    for snapshot in snapshots {
        if seen.insert(snapshot.dedup_key.clone()) {
            sessions
                .entry(snapshot.session_id.clone())
                .or_default()
                .push(snapshot);
        }
    }
    let mut gaps = Vec::new();
    for (session_id, mut snapshots) in sessions {
        if snapshots.iter().all(|snapshot| snapshot.is_shutdown)
            || snapshots
                .iter()
                .any(|snapshot| snapshot.is_shutdown && snapshot.total_nano_aiu.is_none())
        {
            continue;
        }
        snapshots.sort_by_key(|snapshot| (snapshot.timestamp, snapshot.is_shutdown));
        let receipt_credits = if snapshots
            .iter()
            .any(|snapshot| snapshot.receipt_watermark.is_some())
        {
            let Some(amounts) = receipt_ledgers
                .get(session_id.as_str())
                .and_then(|calls| covered_receipt_credits(&snapshots, calls))
            else {
                continue;
            };
            amounts
        } else {
            Vec::new()
        };
        let last_shutdown = snapshots.iter().rposition(|snapshot| snapshot.is_shutdown);
        let first_uncovered = uncovered_otel.get(session_id.as_str());
        let mut session_total = 0_u64;
        let mut attributed = 0_u64;
        let mut points = Vec::new();
        for (position, snapshot) in snapshots.iter().enumerate() {
            if !snapshot.is_shutdown && last_shutdown.is_some_and(|last| position < last) {
                continue;
            }
            session_total = session_total.max(snapshot.total_nano_aiu.unwrap_or_default());
            if snapshot.is_shutdown && snapshot.receipt_watermark.is_none() {
                attributed += attributed_nano_aiu
                    .get(&(session_id.clone(), snapshot.timestamp.as_millis()))
                    .copied()
                    .unwrap_or_default();
            }
            let receipted = receipt_credits.get(position).copied().unwrap_or_default();
            points.push((
                snapshot,
                session_total
                    .saturating_sub(attributed)
                    .saturating_sub(receipted),
            ));
        }
        let mut lasting = u64::MAX;
        for (_, unexplained) in points.iter_mut().rev() {
            lasting = lasting.min(*unexplained);
            *unexplained = lasting;
        }
        let mut reported = 0_u64;
        for (snapshot, lasting) in points {
            let gap = lasting.saturating_sub(reported);
            reported = reported.max(lasting);
            let millis = snapshot.timestamp.as_millis();
            if gap == 0
                || first_uncovered.is_some_and(|timestamp| *timestamp <= snapshot.timestamp)
                || since_millis.is_some_and(|start| millis < start)
                || until_millis.is_some_and(|end| millis >= end)
            {
                continue;
            }
            gaps.push(credit_gap_entry(snapshot, gap));
        }
    }
    gaps
}

fn credit_gap_entry(snapshot: &CopilotCreditSnapshot, nano_aiu: u64) -> CopilotUsageEntry {
    CopilotUsageEntry {
        timestamp: snapshot.timestamp,
        session_id: snapshot.session_id.clone(),
        model: UNATTRIBUTED_MODEL.to_string(),
        logged_model: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_output_tokens: 0,
        extra_total_tokens: 0,
        request_count: 0,
        nano_aiu: Some(nano_aiu),
        dedup_key: snapshot.dedup_key.clone(),
        call_key: None,
        call_ids: CallIds::default(),
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
        call_key: None,
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

fn read_source_file(
    path: &Path,
    kind: CopilotSourceKind,
    diagnostics: &Diagnostics,
) -> std::result::Result<CopilotSourceUsage, UnitError> {
    match kind {
        CopilotSourceKind::Otel => Ok(CopilotSourceUsage {
            entries: parse_otel_file(path, diagnostics)?,
            ..CopilotSourceUsage::default()
        }),
        CopilotSourceKind::SessionState => parse_session_state_file(path, diagnostics),
    }
}

// NOTE(2026-10-10, #1824): One AIU is one GitHub AI credit worth USD 0.01. A nano-AIU is a billionth of that credit.
fn nano_aiu_to_usd(nano_aiu: u64) -> f64 {
    nano_aiu as f64 / 1e11
}

fn usage_entry_to_loaded(
    entry: CopilotUsageEntry,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usd = entry.nano_aiu.map(nano_aiu_to_usd);
    let logged = entry.logged_model.as_deref().unwrap_or(&entry.model);
    let model = (entry.model.as_str(), MatchKind::Fuzzy);
    let with_logged = [(logged, MatchKind::Exact), model];
    let candidates: &[(_, _)] = if entry.logged_model.is_some() {
        &with_logged
    } else {
        std::slice::from_ref(&model)
    };
    let priced = price_entry(
        EntryPricing {
            usage,
            reasoning_tokens: 0,
            residual_tokens: entry.extra_total_tokens,
            reported_model: Some(&entry.model),
            candidates,
            stored_cost: cost_usd,
            timestamp: Some(entry.timestamp),
        },
        pricing,
    );
    let cost = priced.cost;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| crate::model_aliases::resolve_model_name(&entry.model).into_owned());
    let stored_cost = stored_cost_for_mode(cost_usd, mode);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: (entry.request_count > 0).then_some(entry.request_count),
        model: Some(entry.model),
        usage,
        message_id: Some(entry.dedup_key),
        version: None,
        recorded_cost: cost_usd,
        usage_limit_reset_time: None,
        missing_pricing_model,
        stored_cost,
    }
}

#[cfg(test)]
mod tests;
