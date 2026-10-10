use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        CopilotCreditSnapshot, CopilotSourceUsage, CopilotUsageEntry, parse_otel_file,
        parse_session_state_file,
    },
    paths::{CopilotSourceKind, paths},
};
use crate::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage_at, cli::CostMode, date_range_bounds_ms, debug_log, format_date_tz,
    missing_pricing_model_for_usage, parse_tz, read_files_parallel,
};

pub fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("GitHub Copilot CLI"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let sources = paths()?;
    let source_kinds = sources
        .iter()
        .map(|source| (source.path.clone(), source.kind))
        .collect::<HashMap<_, _>>();
    let files = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    // Read source files in parallel; entries keep their original file order before
    // the stable sort, so OTel-only output is identical to the previous read.
    let parsed = read_files_parallel(&files, shared.single_thread, |path| {
        let kind = source_kinds
            .get(path)
            .copied()
            .unwrap_or(CopilotSourceKind::Otel);
        read_source_file(path, kind).unwrap_or_else(|error| {
            let source_name = match kind {
                CopilotSourceKind::Otel => "OTEL",
                CopilotSourceKind::SessionState => "session-state",
            };
            debug_log(
                shared,
                format!(
                    "Failed to read Copilot {source_name} file {}: {error}",
                    path.display(),
                ),
            );
            CopilotSourceUsage::default()
        })
    });
    let mut otel_entries = Vec::new();
    let mut session_state_entries = Vec::new();
    let mut credit_snapshots = Vec::new();
    for (source, file_usage) in sources.iter().map(|source| source.kind).zip(parsed) {
        match source {
            CopilotSourceKind::Otel => otel_entries.extend(file_usage.entries),
            CopilotSourceKind::SessionState => {
                session_state_entries.extend(file_usage.entries);
                credit_snapshots.extend(file_usage.credit_snapshots);
            }
        }
    }
    let (since_millis, until_millis) = date_range_bounds_ms(
        shared.since.as_deref(),
        shared.until.as_deref(),
        tz.as_ref(),
    );
    let session_state = reconcile_session_state_entries(
        session_state_entries,
        &otel_call_timestamps(&otel_entries),
        since_millis,
        until_millis,
    );
    // OpenTelemetry rows record every model call, so sessions that have them
    // are left to that source rather than to the coarser credit totals.
    let otel_sessions = otel_entries
        .iter()
        .map(|entry| entry.session_id.clone())
        .collect::<HashSet<_>>();
    let credit_gaps = credit_gap_entries(
        credit_snapshots,
        &session_state.attributed_nano_aiu,
        &otel_sessions,
        since_millis,
        until_millis,
    );
    let otel_after = session_state
        .otel_after
        .iter()
        .map(|((session_id, model), timestamp)| ((session_id.as_str(), model.as_str()), *timestamp))
        .collect::<HashMap<_, _>>();
    otel_entries.retain(|entry| {
        otel_after
            .get(&(entry.session_id.as_str(), entry.model.as_str()))
            .is_none_or(|after| entry.timestamp > *after)
    });

    let mut entries = session_state
        .entries
        .into_iter()
        .chain(otel_entries)
        .chain(credit_gaps)
        .map(|entry| usage_entry_to_loaded(entry, tz.as_ref(), shared.mode, pricing))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

struct SessionStateReconciliation {
    entries: Vec<CopilotUsageEntry>,
    /// Per `(session, model)`, the time after which OpenTelemetry rows are
    /// reported: the latest shutdown through `--until`, or the shutdown before
    /// the intervals that OTel replaces. Without an entry, every row counts.
    otel_after: HashMap<(String, String), TimestampMs>,
    /// Per-model credits reported at each shutdown, keyed by session and
    /// shutdown timestamp, including shutdowns outside `--since`/`--until`.
    attributed_nano_aiu: HashMap<(String, i64), u64>,
}

/// OpenTelemetry has one row per model call; timestamps per `(session, model)`,
/// sorted.
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

/// A Copilot process restores the breakdown of the session's last shutdown, so
/// the calls of a process that ended without one (killed, crashed, or restarted
/// by the app) are missing from every later shutdown, while OpenTelemetry still
/// has a row for each of them. Counted from the first shutdown interval that has
/// OTel rows, OTel replaces the shutdown intervals when it holds more calls than
/// they report; returns the position of that interval in `sorted`. Shutdowns
/// without request counts cannot be compared and keep their intervals. The
/// comparison runs over every shutdown, so it does not depend on the date range.
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

// Session-state usage is cumulative per `(session, model)`, so resumed sessions
// emit one shutdown per resume. Each snapshot is turned into interval usage:
// the first snapshot is kept as-is and every later snapshot subtracts its
// predecessor, keeping daily attribution while preserving the total.
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
    // Sort keys for deterministic output across HashMap iteration.
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
        if let Some(position) = otel_after_position {
            otel_after.insert(key.clone(), entries[sorted[position]].timestamp);
        }
        let mut previous: Option<&CopilotUsageEntry> = None;
        // Credits are collected past `--until` too, so a breakdown that a later
        // shutdown catches up on is judged the same way in every date range.
        for (position, index) in sorted.iter().enumerate() {
            let current = &entries[*index];
            let reconciled = previous.map_or_else(
                || current.clone(),
                |baseline| subtract_usage(current, baseline),
            );
            previous = Some(current);
            *attributed_nano_aiu
                .entry((current.session_id.clone(), current.timestamp.as_millis()))
                .or_default() += reconciled.nano_aiu;
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
    // Keep chronological order for downstream sorting stability.
    interval_indices.sort_by_key(|entry| (entry.timestamp, entry.dedup_key.clone()));
    SessionStateReconciliation {
        entries: interval_indices,
        otel_after,
        attributed_nano_aiu,
    }
}

/// Model label for credits Copilot billed without saying which model used them.
const UNATTRIBUTED_MODEL: &str = "unknown";

/// Credits in the session-wide `totalNanoAiu` that no `modelMetrics` entry
/// accounts for, as cost-only entries.
///
/// `modelMetrics` only covers the running Copilot process: a resumed session,
/// or a session the client restarted, reports an empty or partial breakdown,
/// while `totalNanoAiu` keeps the whole session. At every shutdown, and at the
/// checkpoints after the last one (sessions still open or never shut down
/// cleanly), the unexplained amount is the session total minus the per-model
/// credits reported so far. A shutdown can carry a breakdown that a later one
/// catches up on, so only what stays unexplained at every later snapshot is
/// reported, dated at the first snapshot from which it stays. Checkpoints
/// before a shutdown are already part of that shutdown's total.
///
/// Only sessions with checkpoints qualify: Copilot versions that predate them
/// restart both totals on every resume, so the cumulative reading used for
/// `modelMetrics` does not hold there and the two cannot be compared.
fn credit_gap_entries(
    snapshots: Vec<CopilotCreditSnapshot>,
    attributed_nano_aiu: &HashMap<(String, i64), u64>,
    skipped_sessions: &HashSet<String>,
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
        // Sessions without checkpoints come from Copilot versions that restart
        // their totals. Without a credit total on every shutdown, the token
        // usage already reported cannot be told apart from what is missing.
        if skipped_sessions.contains(&session_id)
            || snapshots.iter().all(|snapshot| snapshot.is_shutdown)
            || snapshots
                .iter()
                .any(|snapshot| snapshot.is_shutdown && snapshot.total_nano_aiu.is_none())
        {
            continue;
        }
        snapshots.sort_by_key(|snapshot| (snapshot.timestamp, snapshot.is_shutdown));
        let last_shutdown = snapshots.iter().rposition(|snapshot| snapshot.is_shutdown);
        let mut session_total = 0_u64;
        let mut attributed = 0_u64;
        let mut points = Vec::new();
        for (position, snapshot) in snapshots.iter().enumerate() {
            if !snapshot.is_shutdown && last_shutdown.is_some_and(|last| position < last) {
                continue;
            }
            session_total = session_total.max(snapshot.total_nano_aiu.unwrap_or_default());
            if snapshot.is_shutdown {
                attributed += attributed_nano_aiu
                    .get(&(session_id.clone(), snapshot.timestamp.as_millis()))
                    .copied()
                    .unwrap_or_default();
            }
            points.push((snapshot, session_total.saturating_sub(attributed)));
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
        timestamp_text: crate::format_rfc3339_millis(snapshot.timestamp),
        session_id: snapshot.session_id.clone(),
        model: UNATTRIBUTED_MODEL.to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_output_tokens: 0,
        extra_total_tokens: 0,
        request_count: 0,
        nano_aiu,
        // One AIU is one GitHub AI credit, billed at $0.01.
        cost_usd: Some(nano_aiu as f64 / 1e11),
        dedup_key: snapshot.dedup_key.clone(),
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
        timestamp_text: current.timestamp_text.clone(),
        session_id: current.session_id.clone(),
        model: current.model.clone(),
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
        nano_aiu: current.nano_aiu.saturating_sub(baseline.nano_aiu),
        cost_usd: None,
        dedup_key: current.dedup_key.clone(),
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
}

fn read_source_file(path: &Path, kind: CopilotSourceKind) -> Result<CopilotSourceUsage> {
    match kind {
        CopilotSourceKind::Otel => Ok(CopilotSourceUsage {
            entries: parse_otel_file(path)?,
            credit_snapshots: Vec::new(),
        }),
        CopilotSourceKind::SessionState => parse_session_state_file(path),
    }
}

#[cfg(test)]
fn read_otel_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    Ok(read_source_file(path, CopilotSourceKind::Otel)?
        .entries
        .into_iter()
        .map(|entry| usage_entry_to_loaded(entry, tz, mode, pricing))
        .collect())
}

fn usage_entry_to_loaded(
    entry: CopilotUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(entry.extra_total_tokens),
        cache_creation: None,
        ..usage
    };
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(entry.model.clone()),
            id: Some(entry.dedup_key),
        },
        cost_usd: entry.cost_usd,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost = calculate_cost_for_usage_at(
        Some(&entry.model),
        cost_usage,
        entry.cost_usd,
        Some(entry.timestamp),
        mode,
        Some(pricing),
    );
    let missing_pricing_model = missing_pricing_model_for_usage(
        Some(&entry.model),
        cost_usage,
        entry.cost_usd,
        mode,
        Some(pricing),
    );
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: (entry.request_count > 0).then_some(entry.request_count),
        model: Some(entry.model),
        data,
        usage_limit_reset_time: None,
        missing_pricing_model,
    }
}

#[cfg(test)]
use super::report::{report_from_rows, summarize_entries};

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
    use serde_json::json;

    use super::super::parser::parse_otel_file;
    use super::*;
    use crate::cli::AgentReportKind;

    #[test]
    fn parses_copilot_chat_spans() {
        let fixture = fs_fixture!({
            "copilot.jsonl": [
                json!({ "type": "metric", "name": "gen_ai.client.token.usage" }).to_string(),
                json!({
                    "type": "span",
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "chat claude-sonnet-4",
                    "endTime": [1_775_934_264_u64, 967_317_833_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.request.model": "claude-sonnet-4",
                        "gen_ai.response.model": "claude-sonnet-4",
                        "gen_ai.conversation.id": "conv-1",
                        "gen_ai.usage.input_tokens": 19_452,
                        "gen_ai.usage.output_tokens": 281,
                        "gen_ai.usage.cache_read.input_tokens": 123,
                        "gen_ai.usage.cache_creation.input_tokens": 25,
                        "gen_ai.usage.reasoning.output_tokens": 128,
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });
        let file = fixture.path("copilot.jsonl");

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp_text, "2026-04-11T19:04:24.967Z");
        assert_eq!(entries[0].session_id, "conv-1");
        assert_eq!(entries[0].model, "claude-sonnet-4");
        assert_eq!(entries[0].input_tokens, 19_304);
        assert_eq!(entries[0].output_tokens, 281);
        assert_eq!(entries[0].cache_creation_tokens, 25);
        assert_eq!(entries[0].cache_read_tokens, 123);
        assert_eq!(entries[0].reasoning_output_tokens, 128);
        assert_eq!(entries[0].dedup_key, "trace-1:span-1");
    }

    #[test]
    fn suppresses_lower_priority_records_for_same_response() {
        let fixture = fs_fixture!({
            "copilot.jsonl": [
                json!({
                    "type": "span",
                    "traceId": "trace-dupe",
                    "spanId": "agent-1",
                    "name": "invoke_agent GitHub Copilot Chat",
                    "attributes": {
                        "gen_ai.operation.name": "invoke_agent",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.conversation.id": "conv-dupe",
                        "gen_ai.response.id": "resp-dupe",
                        "gen_ai.usage.input_tokens": 100,
                        "gen_ai.usage.output_tokens": 30,
                    },
                })
                .to_string(),
                json!({
                    "hrTime": [1_775_934_263_u64, 0_u64],
                    "attributes": {
                        "event.name": "gen_ai.client.inference.operation.details",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.response.id": "resp-dupe",
                        "gen_ai.usage.input_tokens": 80,
                        "gen_ai.usage.output_tokens": 20,
                    },
                    "_body": "GenAI inference: gpt-5.4-mini",
                })
                .to_string(),
                json!({
                    "type": "span",
                    "traceId": "trace-dupe",
                    "spanId": "chat-1",
                    "name": "chat gpt-5.4-mini",
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.conversation.id": "conv-dupe",
                        "gen_ai.response.id": "resp-dupe",
                        "gen_ai.usage.input_tokens": 60,
                        "gen_ai.usage.output_tokens": 10,
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });
        let file = fixture.path("copilot.jsonl");

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].dedup_key, "trace-dupe:chat-1");
        assert_eq!(entries[0].input_tokens, 60);
        assert_eq!(entries[0].output_tokens, 10);
    }

    fn chat_span(
        (trace_id, span_id): (&str, &str),
        end_seconds: u64,
        response_id: &str,
        input_tokens: u64,
    ) -> String {
        json!({
            "type": "span",
            "traceId": trace_id,
            "spanId": span_id,
            "name": "chat test-model",
            "endTime": [end_seconds, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "conv-1",
                "gen_ai.response.id": response_id,
                "gen_ai.usage.input_tokens": input_tokens,
                "gen_ai.usage.output_tokens": 10,
            },
        })
        .to_string()
    }

    fn dedup_keys(entries: &[CopilotUsageEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.dedup_key.as_str())
            .collect()
    }

    #[test]
    fn counts_a_re_exported_chat_span_once() {
        let fixture = fs_fixture!({
            "copilot.jsonl": [
                chat_span(("trace-1", "span-1"), 1_775_934_264, "resp-1", 100),
                chat_span(("trace-1", "span-2"), 1_775_934_300, "resp-2", 200),
                // The CLI exports some calls a second time, with the response
                // ID and usage of the call but a new trace or span ID.
                chat_span(("trace-2", "span-3"), 1_775_934_368, "resp-1", 100),
                chat_span(("trace-1", "span-4"), 1_775_934_370, "resp-2", 200),
                chat_span(("trace-3", "span-5"), 1_775_934_500, "resp-3", 300),
                chat_span(("trace-1", "span-6"), 1_775_934_400, "resp-3", 300),
            ]
            .join("\n"),
        });

        let entries = parse_otel_file(&fixture.path("copilot.jsonl")).unwrap();

        assert_eq!(
            dedup_keys(&entries),
            ["trace-1:span-1", "trace-1:span-2", "trace-1:span-6"]
        );
    }

    #[test]
    fn keeps_calls_of_one_turn_that_share_a_response_id() {
        // VS Code reports the response ID of the turn on every call it makes.
        let fixture = fs_fixture!({
            "copilot.jsonl": [
                chat_span(("trace-1", "span-1"), 1_775_934_264, "turn-1", 1_000),
                chat_span(("trace-1", "span-2"), 1_775_934_270, "turn-1", 1_200),
                chat_span(("trace-1", "span-3"), 1_775_934_280, "turn-1", 1_400),
            ]
            .join("\n"),
        });

        let entries = parse_otel_file(&fixture.path("copilot.jsonl")).unwrap();

        assert_eq!(
            dedup_keys(&entries),
            ["trace-1:span-1", "trace-1:span-2", "trace-1:span-3"]
        );
    }

    #[test]
    fn does_not_double_count_reasoning_tokens() {
        let fixture = fs_fixture!({
            "copilot.jsonl":
            format!(
                "{}\n",
                json!({
                    "type": "span",
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "chat test-model",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "conv-1",
                        "gen_ai.usage.input_tokens": 100,
                        "gen_ai.usage.output_tokens": 50,
                        "gen_ai.usage.cache_read.input_tokens": 10,
                        "gen_ai.usage.cache_creation.input_tokens": 20,
                        "gen_ai.usage.reasoning.output_tokens": 5,
                    },
                })
            ),
        });
        let file = fixture.path("copilot.jsonl");
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
        );

        let loaded = read_otel_file(&file, None, CostMode::Auto, &pricing).unwrap();
        let rows = summarize_entries(&loaded, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 70);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 150);
        assert_eq!(report["daily"][0]["totalCost"], 270.0);
        assert_eq!(
            report["daily"][0]["modelBreakdowns"],
            json!([{
                "modelName": "test-model",
                "inputTokens": 70,
                "outputTokens": 50,
                "cacheCreationTokens": 20,
                "cacheReadTokens": 10,
                "cost": 270.0
            }])
        );
    }

    #[test]
    fn includes_separate_otel_reasoning_tokens_in_total_and_cost() {
        let fixture = fs_fixture!({
            "copilot.jsonl": format!(
                "{}\n",
                json!({
                    "type": "span",
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "chat test-model",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "conv-1",
                        "gen_ai.usage.input_tokens": 100,
                        "gen_ai.usage.output_tokens": 50,
                        "gen_ai.usage.cache_read.input_tokens": 10,
                        "gen_ai.usage.cache_creation.input_tokens": 20,
                        "gen_ai.usage.reasoning.output_tokens": 5,
                        "gen_ai.usage.total_tokens": 155,
                    },
                })
            ),
        });
        let file = fixture.path("copilot.jsonl");
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
        );

        let loaded = read_otel_file(&file, None, CostMode::Auto, &pricing).unwrap();
        let rows = summarize_entries(&loaded, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(loaded[0].extra_total_tokens, 5);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 155);
        assert_eq!(report["daily"][0]["totalCost"], 280.0);
        assert_eq!(report["daily"][0]["modelBreakdowns"][0]["cost"], 280.0);
    }

    #[test]
    fn splits_otel_input_tokens_like_session_state() {
        // Copilot counts cache reads and writes in the input tokens of both
        // sources; one call, as each source records it.
        let fixture = fs_fixture!({
            "copilot.jsonl": json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat claude-haiku-4.5",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "claude-haiku-4.5",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 20_000,
                    "gen_ai.usage.output_tokens": 300,
                    "gen_ai.usage.cache_read.input_tokens": 12_000,
                    "gen_ai.usage.cache_write.input_tokens": 7_990,
                },
            })
            .to_string(),
            "session-1/events.jsonl": json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-11T19:04:24.000Z",
                "data": {"modelMetrics": {"claude-haiku-4.5": {
                    "usage": {
                        "inputTokens": 20_000,
                        "outputTokens": 300,
                        "cacheReadTokens": 12_000,
                        "cacheWriteTokens": 7_990
                    },
                    "requests": {"count": 1}
                }}}
            })
            .to_string(),
        });
        let split = |entry: &CopilotUsageEntry| {
            (
                entry.input_tokens,
                entry.cache_read_tokens,
                entry.cache_creation_tokens,
                entry.output_tokens,
            )
        };

        let otel = parse_otel_file(&fixture.path("copilot.jsonl")).unwrap();
        let session_state =
            super::super::parser::parse_session_state_file(&fixture.path("session-1/events.jsonl"))
                .unwrap()
                .entries;

        assert_eq!(split(&otel[0]), (10, 12_000, 7_990, 300));
        assert_eq!(split(&otel[0]), split(&session_state[0]));
    }

    #[test]
    fn falls_back_to_total_tokens_when_copilot_parts_are_missing() {
        let fixture = fs_fixture!({
            "copilot.jsonl":
            format!(
                "{}\n",
                json!({
                    "type": "span",
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "chat test-model",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "conv-1",
                        "gen_ai.usage.total_tokens": 567,
                        "gen_ai.usage.reasoning_tokens": 5,
                    },
                })
            ),
        });
        let file = fixture.path("copilot.jsonl");

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].output_tokens, 567);
        assert_eq!(entries[0].reasoning_output_tokens, 5);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn loads_session_state_tokens_and_calculates_token_cost() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": format!(
                "{}\n",
                json!({
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
                                },
                                "requests": {"count": 3, "cost": 999}
                            }
                        }
                    }
                })
            ),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
        );
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Auto,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &pricing).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[0].message_count, Some(3));
        assert_eq!(entries[0].cost, 270.0);
    }

    #[test]
    fn uses_shutdown_snapshot_as_of_until_for_otel_reconciliation() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-old",
                    "timestamp": "2026-01-02T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 100,
                        "outputTokens": 50,
                        "cacheReadTokens": 10,
                        "cacheWriteTokens": 20
                    }}}}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-latest",
                    "timestamp": "2026-01-03T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 200,
                        "outputTokens": 80,
                        "cacheReadTokens": 20,
                        "cacheWriteTokens": 30
                    }}}}
                })
                .to_string(),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": json!({
                "type": "span",
                "traceId": "trace-between-shutdowns",
                "spanId": "span-between-shutdowns",
                "name": "chat test-model",
                "endTime": [1_767_320_400_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 11,
                    "gen_ai.usage.output_tokens": 12
                }
            })
            .to_string(),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some("UTC".to_string()),
            until: Some("20260102".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[1].data.message.usage.input_tokens, 11);
        assert_eq!(entries[1].data.message.usage.output_tokens, 12);
    }

    #[test]
    fn subtracts_the_pre_since_shutdown_before_retaining_resumed_otel_rows() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-old",
                    "timestamp": "2026-01-02T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {
                        "usage": {
                            "inputTokens": 100,
                            "outputTokens": 50,
                            "cacheReadTokens": 10,
                            "cacheWriteTokens": 20
                        },
                        "requests": {"count": 1}
                    }}}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-latest",
                    "timestamp": "2026-01-03T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {
                        "usage": {
                            "inputTokens": 200,
                            "outputTokens": 80,
                            "cacheReadTokens": 20,
                            "cacheWriteTokens": 30
                        },
                        "requests": {"count": 3}
                    }}}
                })
                .to_string(),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": json!({
                "type": "span",
                "traceId": "trace-resumed",
                "spanId": "span-resumed",
                "name": "chat test-model",
                "endTime": [1_767_406_800_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 13,
                    "gen_ai.usage.output_tokens": 14
                }
            })
            .to_string(),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            since: Some("20260103".to_string()),
            timezone: Some("UTC".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 80);
        assert_eq!(entries[0].data.message.usage.output_tokens, 30);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            10
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].message_count, Some(2));
        assert_eq!(entries[1].data.message.usage.input_tokens, 13);
        assert_eq!(entries[1].data.message.usage.output_tokens, 14);
        assert_eq!(entries[1].message_count, Some(1));
    }

    #[test]
    fn normalizes_copilot_model_suffixes_for_pricing_and_otel_dedupe() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-08-30T12:00:00Z",
                    "data": {
                        "modelMetrics": {
                            "claude-opus-4.6-1m": {
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
            ),
            "home/.copilot/otel/session.jsonl": format!(
                "{}\n",
                json!({
                    "type": "span",
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "chat claude-opus-4.6-1m-internal",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "claude-opus-4.6-1m-internal",
                        "gen_ai.conversation.id": "session-1",
                        "gen_ai.usage.input_tokens": 999,
                        "gen_ai.usage.output_tokens": 999
                    }
                })
            )
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Auto,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-opus-4.6"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert!((entries[0].cost - 0.00173).abs() < 1e-12);
    }

    #[test]
    fn splits_cumulative_shutdowns_into_intervals_and_keeps_unmatched_otel_rows() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 10,
                        "outputTokens": 20
                    }}}}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 10,
                        "outputTokens": 20
                    }}}}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-2",
                    "timestamp": "2026-04-15T09:53:27.352Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 30,
                        "outputTokens": 40
                    }}}}
                })
                .to_string(),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": [
                json!({
                    "type": "span",
                    "traceId": "trace-duplicate",
                    "spanId": "span-duplicate",
                    "name": "chat test-model",
                    "endTime": [1_776_246_780_u64, 352_000_000_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "session-1",
                        "gen_ai.usage.input_tokens": 100,
                        "gen_ai.usage.output_tokens": 200
                    }
                })
                .to_string(),
                json!({
                    "type": "span",
                    "traceId": "trace-post-shutdown",
                    "spanId": "span-post-shutdown",
                    "name": "chat test-model",
                    "endTime": [1_776_246_840_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "session-1",
                        "gen_ai.usage.input_tokens": 7,
                        "gen_ai.usage.output_tokens": 8
                    }
                })
                .to_string(),
                json!({
                    "type": "span",
                    "traceId": "trace-other-model",
                    "spanId": "span-other-model",
                    "name": "chat other-model",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "other-model",
                        "gen_ai.conversation.id": "session-1",
                        "gen_ai.usage.input_tokens": 3,
                        "gen_ai.usage.output_tokens": 4
                    }
                })
                .to_string(),
                json!({
                    "type": "span",
                    "traceId": "trace-other-session",
                    "spanId": "span-other-session",
                    "name": "chat test-model",
                    "endTime": [1_775_934_264_u64, 0_u64],
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "test-model",
                        "gen_ai.conversation.id": "session-2",
                        "gen_ai.usage.input_tokens": 5,
                        "gen_ai.usage.output_tokens": 6
                    }
                })
                .to_string(),
            ]
            .join("\n"),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.session_id.to_string(),
                    entry.model.clone().unwrap_or_default(),
                    entry.data.message.usage.input_tokens,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("session-1".to_string(), "other-model".to_string(), 3),
                ("session-2".to_string(), "test-model".to_string(), 5),
                ("session-1".to_string(), "test-model".to_string(), 10),
                ("session-1".to_string(), "test-model".to_string(), 20),
                ("session-1".to_string(), "test-model".to_string(), 7),
            ]
        );
    }

    #[test]
    fn splits_resumed_shutdowns_across_dates_without_double_counting() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-old",
                    "timestamp": "2026-01-02T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 100,
                        "outputTokens": 50,
                        "cacheReadTokens": 10,
                        "cacheWriteTokens": 20
                    },
                    "requests": {"count": 1}}}}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-latest",
                    "timestamp": "2026-01-03T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 200,
                        "outputTokens": 80,
                        "cacheReadTokens": 20,
                        "cacheWriteTokens": 30
                    },
                    "requests": {"count": 3}}}}
                })
                .to_string(),
            ]
            .join("\n"),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].message_count, Some(1));
        assert_eq!(entries[1].date, "2026-01-03");
        assert_eq!(entries[1].data.message.usage.input_tokens, 80);
        assert_eq!(entries[1].data.message.usage.output_tokens, 30);
        assert_eq!(entries[1].message_count, Some(2));
        let total_input: u64 = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(total_input, 150);
    }

    #[test]
    fn keeps_one_entry_for_a_single_shutdown_snapshot() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-01-02T01:20:00.000Z",
                    "data": {"modelMetrics": {"test-model": {"usage": {
                        "inputTokens": 100,
                        "outputTokens": 50
                    },
                    "requests": {"count": 1}}}}
                })
            ),
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].message_count, Some(1));
    }

    const NANO_AIU_PER_CREDIT: u64 = 1_000_000_000;

    fn checkpoint(id: &str, timestamp: &str, credits: u64) -> String {
        json!({
            "type": "session.usage_checkpoint",
            "id": id,
            "timestamp": timestamp,
            "data": {"totalNanoAiu": credits * NANO_AIU_PER_CREDIT}
        })
        .to_string()
    }

    fn shutdown(id: &str, timestamp: &str, credits: u64, model_credits: Option<u64>) -> String {
        let mut model = json!({"usage": {"inputTokens": 100, "outputTokens": 50}});
        if let Some(model_credits) = model_credits {
            model["totalNanoAiu"] = json!(model_credits * NANO_AIU_PER_CREDIT);
        }
        json!({
            "type": "session.shutdown",
            "id": id,
            "timestamp": timestamp,
            "data": {
                "totalNanoAiu": credits * NANO_AIU_PER_CREDIT,
                "modelMetrics": {"test-model": model}
            }
        })
        .to_string()
    }

    fn load_copilot_home(
        fixture: &ccusage_test_support::Fixture,
        shared: &crate::cli::SharedArgs,
    ) -> Vec<LoadedEntry> {
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let mut entries = load_entries_inner(shared, &crate::PricingMap::default()).unwrap();
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, shared);
        entries
    }

    fn utc_args() -> crate::cli::SharedArgs {
        crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..crate::cli::SharedArgs::default()
        }
    }

    /// `(date, model, total tokens, cost in whole credits)` per entry.
    fn credit_rows(entries: &[LoadedEntry]) -> Vec<(String, String, u64, u64)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.date.to_string(),
                    entry.model.clone().unwrap_or_default(),
                    crate::total_usage_tokens(entry.data.message.usage),
                    (entry.cost * 100.0).round() as u64,
                )
            })
            .collect()
    }

    #[test]
    fn reports_session_credits_missing_from_model_metrics() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 51),
                checkpoint("checkpoint-2", "2026-01-02T10:10:00.000Z", 73),
                shutdown("shutdown-1", "2026-01-02T10:20:00.000Z", 93, Some(42)),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [
                ("2026-01-02".to_string(), "test-model".to_string(), 150, 0),
                ("2026-01-02".to_string(), "unknown".to_string(), 0, 51),
            ]
        );
        assert_eq!(entries[1].message_count, None);
        assert_eq!(entries[1].missing_pricing_model, None);
    }

    #[test]
    fn reports_checkpoint_credits_of_sessions_without_a_shutdown() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 10, Some(10)),
                checkpoint("checkpoint-1", "2026-01-02T11:00:00.000Z", 25),
                checkpoint("checkpoint-2", "2026-01-03T11:00:00.000Z", 40),
            ]
            .join("\n"),
            "home/.copilot/session-state/session-2/events.jsonl":
                checkpoint("checkpoint-3", "2026-01-03T12:00:00.000Z", 7),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [
                ("2026-01-02".to_string(), "test-model".to_string(), 150, 0),
                ("2026-01-02".to_string(), "unknown".to_string(), 0, 15),
                ("2026-01-03".to_string(), "unknown".to_string(), 0, 15),
                ("2026-01-03".to_string(), "unknown".to_string(), 0, 7),
            ]
        );
    }

    #[test]
    fn reports_resumed_session_credits_when_the_last_shutdown_has_no_model_metrics() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 1_072),
                json!({
                    "type": "session.resume",
                    "timestamp": "2026-01-03T09:00:00.000Z",
                    "data": {}
                })
                .to_string(),
                json!({
                    "type": "session.shutdown",
                    "id": "shutdown-1",
                    "timestamp": "2026-01-03T10:00:00.000Z",
                    "data": {"totalNanoAiu": 1_072 * NANO_AIU_PER_CREDIT, "modelMetrics": {}}
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-03".to_string(), "unknown".to_string(), 0, 1_072)]
        );
    }

    #[test]
    fn subtracts_credit_snapshots_before_since() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(10)),
                checkpoint("checkpoint-1", "2026-01-03T10:00:00.000Z", 50),
            ]
            .join("\n"),
        });
        let shared = crate::cli::SharedArgs {
            since: Some("20260103".to_string()),
            ..utc_args()
        };

        let entries = load_copilot_home(&fixture, &shared);

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-03".to_string(), "unknown".to_string(), 0, 20)]
        );
    }

    #[test]
    fn does_not_count_credits_that_a_later_shutdown_attributes() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 30),
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(30)),
                checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 50),
                shutdown("shutdown-2", "2026-01-02T12:00:00.000Z", 50, Some(35)),
                shutdown("shutdown-3", "2026-01-02T13:00:00.000Z", 50, Some(50)),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 150, 0)]
        );
    }

    #[test]
    fn credit_gaps_do_not_depend_on_until() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 30),
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(30)),
                checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 50),
                shutdown("shutdown-2", "2026-01-02T12:00:00.000Z", 50, Some(35)),
                shutdown("shutdown-3", "2026-01-03T10:00:00.000Z", 50, Some(50)),
            ]
            .join("\n"),
        });
        let shared = crate::cli::SharedArgs {
            until: Some("20260102".to_string()),
            ..utc_args()
        };

        let entries = load_copilot_home(&fixture, &shared);

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 150, 0)]
        );
    }

    #[test]
    fn leaves_sessions_without_checkpoints_unchanged() {
        // Copilot versions without checkpoints restart both totals on resume.
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 60, Some(40)),
                shutdown("shutdown-2", "2026-01-03T10:00:00.000Z", 25, Some(25)),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 150, 0)]
        );
    }

    #[test]
    fn leaves_sessions_with_incomplete_credit_totals_unchanged() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 20),
                shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 93, None),
                checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 120),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 150, 0)]
        );
    }

    #[test]
    fn leaves_credit_gaps_to_otel_when_a_session_has_otel_rows() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl":
                checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 20),
            "home/.copilot/otel/otel.jsonl": json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat test-model",
                "endTime": [1_767_348_000_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 11,
                    "gen_ai.usage.output_tokens": 12
                }
            })
            .to_string(),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 23, 0)]
        );
    }

    #[test]
    fn calculate_mode_does_not_price_credit_only_entries() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl":
                checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 20),
        });
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Calculate,
            ..utc_args()
        };

        let entries = load_copilot_home(&fixture, &shared);

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "unknown".to_string(), 0, 0)]
        );
    }

    /// A shutdown whose `test-model` breakdown reports `requests` requests of
    /// 100 input tokens each.
    fn shutdown_with_requests(id: &str, timestamp: &str, requests: u64) -> String {
        json!({
            "type": "session.shutdown",
            "id": id,
            "timestamp": timestamp,
            "data": {"modelMetrics": {"test-model": {
                "usage": {"inputTokens": requests * 100, "outputTokens": requests * 10},
                "requests": {"count": requests}
            }}}
        })
        .to_string()
    }

    /// One `test-model` call of `session-1` with a single input token.
    fn otel_call(span_id: &str, timestamp: &str) -> String {
        let seconds = crate::parse_ts_timestamp(timestamp).unwrap().as_millis() / 1_000;
        json!({
            "type": "span",
            "traceId": "trace-1",
            "spanId": span_id,
            "name": "chat test-model",
            "endTime": [seconds, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 1,
                "gen_ai.usage.output_tokens": 1
            }
        })
        .to_string()
    }

    /// `(timestamp, input tokens)` per entry: OTel calls have one input token,
    /// shutdown intervals 100 per request.
    fn usage_rows(entries: &[LoadedEntry]) -> Vec<(String, u64)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.data.timestamp.clone(),
                    entry.data.message.usage.input_tokens,
                )
            })
            .collect()
    }

    fn rows_on(rows: &[(String, u64)], date: &str) -> Vec<(String, u64)> {
        rows.iter()
            .filter(|(timestamp, _)| timestamp.starts_with(date))
            .cloned()
            .collect()
    }

    /// The first process shuts down cleanly after two calls, the second makes
    /// three calls and dies without a shutdown, and the third restores the
    /// breakdown of the first shutdown, makes one call and shuts down with three
    /// requests: the three calls of the second process are in no shutdown.
    fn crashed_process_fixture() -> ccusage_test_support::Fixture {
        fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
                shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": [
                otel_call("first-1", "2026-01-02T09:00:00.000Z"),
                otel_call("first-2", "2026-01-02T09:30:00.000Z"),
                otel_call("crashed-1", "2026-01-02T20:00:00.000Z"),
                otel_call("crashed-2", "2026-01-02T21:00:00.000Z"),
                otel_call("crashed-3", "2026-01-03T08:00:00.000Z"),
                otel_call("restored-1", "2026-01-03T09:00:00.000Z"),
                otel_call("after-shutdown", "2026-01-03T11:00:00.000Z"),
            ]
            .join("\n"),
        })
    }

    #[test]
    fn reports_otel_calls_of_a_process_that_ended_without_a_shutdown() {
        let fixture = crashed_process_fixture();

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [
                ("2026-01-02T09:00:00.000Z".to_string(), 1),
                ("2026-01-02T09:30:00.000Z".to_string(), 1),
                ("2026-01-02T20:00:00.000Z".to_string(), 1),
                ("2026-01-02T21:00:00.000Z".to_string(), 1),
                ("2026-01-03T08:00:00.000Z".to_string(), 1),
                ("2026-01-03T09:00:00.000Z".to_string(), 1),
                ("2026-01-03T11:00:00.000Z".to_string(), 1),
            ]
        );
    }

    #[test]
    fn otel_reconciliation_does_not_depend_on_the_date_range() {
        let fixture = crashed_process_fixture();
        let full = usage_rows(&load_copilot_home(&fixture, &utc_args()));
        let until = crate::cli::SharedArgs {
            until: Some("20260102".to_string()),
            ..utc_args()
        };
        let since = crate::cli::SharedArgs {
            since: Some("20260103".to_string()),
            ..utc_args()
        };

        assert_eq!(
            usage_rows(&load_copilot_home(&fixture, &until)),
            rows_on(&full, "2026-01-02")
        );
        assert_eq!(
            usage_rows(&load_copilot_home(&fixture, &since)),
            rows_on(&full, "2026-01-03")
        );
    }

    #[test]
    fn keeps_shutdown_usage_from_before_the_otel_export_started() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
                shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": [
                otel_call("crashed-1", "2026-01-02T20:00:00.000Z"),
                otel_call("crashed-2", "2026-01-02T21:00:00.000Z"),
                otel_call("restored-1", "2026-01-03T09:00:00.000Z"),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [
                ("2026-01-02T10:00:00.000Z".to_string(), 200),
                ("2026-01-02T20:00:00.000Z".to_string(), 1),
                ("2026-01-02T21:00:00.000Z".to_string(), 1),
                ("2026-01-03T09:00:00.000Z".to_string(), 1),
            ]
        );
    }

    #[test]
    fn keeps_shutdown_intervals_when_otel_has_no_calls_they_miss() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
                shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": [
                otel_call("first-1", "2026-01-02T09:00:00.000Z"),
                otel_call("first-2", "2026-01-02T09:30:00.000Z"),
                otel_call("second-1", "2026-01-03T09:00:00.000Z"),
                otel_call("after-shutdown", "2026-01-03T11:00:00.000Z"),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [
                ("2026-01-02T10:00:00.000Z".to_string(), 200),
                ("2026-01-03T10:00:00.000Z".to_string(), 100),
                ("2026-01-03T11:00:00.000Z".to_string(), 1),
            ]
        );
    }

    #[test]
    fn does_not_double_count_calls_a_concurrent_process_shuts_down_with() {
        // Process A resumes after shutdown-1 and makes two calls. Process B
        // resumes the same session, makes none and shuts down first, with the
        // breakdown of shutdown-1; A's later shutdown reports A's two calls.
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 1),
                shutdown_with_requests("shutdown-b", "2026-01-02T10:55:00.000Z", 1),
                shutdown_with_requests("shutdown-a", "2026-01-02T11:00:00.000Z", 3),
            ]
            .join("\n"),
            "home/.copilot/otel/otel.jsonl": [
                otel_call("first-1", "2026-01-02T09:00:00.000Z"),
                otel_call("a-1", "2026-01-02T10:40:00.000Z"),
                otel_call("a-2", "2026-01-02T10:50:00.000Z"),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [
                ("2026-01-02T10:00:00.000Z".to_string(), 100),
                ("2026-01-02T11:00:00.000Z".to_string(), 200),
            ]
        );
    }

    #[test]
    fn leaves_shutdowns_without_request_counts_to_the_shutdown_breakdown() {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-01-02T10:00:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {"inputTokens": 100}}}}
            })
            .to_string(),
            "home/.copilot/otel/otel.jsonl": [
                otel_call("first-1", "2026-01-02T09:00:00.000Z"),
                otel_call("first-2", "2026-01-02T09:30:00.000Z"),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [("2026-01-02T10:00:00.000Z".to_string(), 100)]
        );
    }
}
