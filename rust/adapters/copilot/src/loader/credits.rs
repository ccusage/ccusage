use std::collections::{BTreeMap, HashMap, HashSet};

use crate::TimestampMs;
use crate::parser::{CallIds, CopilotCreditSnapshot, CopilotUsageEntry, CopilotUsageRecordEntry};

pub(super) struct ReceiptCredit {
    timestamp: TimestampMs,
    sequence: u64,
    nano_aiu: Option<u64>,
}

pub(super) fn receipt_ledgers<'a>(
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
pub(super) fn credit_gap_entries(
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
        call_ids: CallIds::default(),
    }
}
