use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use compact_str::CompactString;
use jiff::tz::TimeZone as JiffTimeZone;
use rustc_hash::FxHasher;

use crate::{
    CodexGroup, CodexServiceTier, CodexTokenUsageEvent, CodexUsageBucket, Result,
    cli::{AgentReportKind, SharedArgs, WeekDay},
    fast::FxHashMap,
    format_date_tz, merge_codex_service_tiers, parse_ts_timestamp, parse_tz, wants_json,
    week_start,
};

use super::{parser, paths, replay::CodexReplayPlan};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CodexEventKey {
    session_hash: u64,
    session_len: usize,
    timestamp: crate::TimestampMs,
    model_hash: u64,
    model_len: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

struct CodexDedupeRecord {
    service_tier: Option<CodexServiceTier>,
    model: CompactString,
    session_id: Option<CompactString>,
}

type CodexDedupeMap = FxHashMap<CodexEventKey, CodexDedupeRecord>;
type CodexDedupeShards = [Mutex<CodexDedupeMap>];

struct CodexAggregation {
    groups: BTreeMap<String, CodexGroup>,
    seen: CodexDedupeMap,
}

/// Read-only inputs every file of one aggregation run shares.
struct CodexAggregateRun<'a> {
    sessions_dir: &'a Path,
    files: &'a [PathBuf],
    shared: &'a SharedArgs,
    kind: AgentReportKind,
    replay_plan: &'a CodexReplayPlan,
}

pub(crate) fn load_groups(
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    let sources = paths::codex_usage_sources()?;
    if sources.len() == 1 && !wants_json(shared) {
        return load_groups_from_directory(&sources[0].dir, shared, kind);
    }
    load_groups_from_sources(&sources, shared, kind)
}

fn load_groups_from_sources(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    let file_groups = paths::collect_deduped_codex_usage_files(sources);
    let replay_plan = CodexReplayPlan::new(
        file_groups
            .iter()
            .map(|group| (group.dir.as_path(), group.files.as_slice())),
        shared.single_thread,
    );
    let mut groups = BTreeMap::new();
    let seen = create_dedupe_shards();
    for group in &file_groups {
        merge_groups(
            &mut groups,
            aggregate_files_with_dedupe(
                &CodexAggregateRun {
                    sessions_dir: &group.dir,
                    files: &group.files,
                    shared,
                    kind,
                    replay_plan: &replay_plan,
                },
                &seen,
            )?,
        );
    }
    apply_recorded_usage_from_shards(&mut groups, &seen, shared, kind);
    Ok(groups)
}

pub(super) fn load_groups_from_directory(
    sessions_dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    let files = paths::collect_codex_usage_files(sessions_dir);
    let replay_plan =
        CodexReplayPlan::new([(sessions_dir, files.as_slice())], shared.single_thread);
    let run = CodexAggregateRun {
        sessions_dir,
        files: &files,
        shared,
        kind,
        replay_plan: &replay_plan,
    };
    if shared.single_thread {
        return aggregate_files_local(&run);
    }
    let seen = create_dedupe_shards();
    let mut groups = aggregate_files_parallel(&run, &seen)?;
    apply_recorded_usage_from_shards(&mut groups, &seen, shared, kind);
    Ok(groups)
}

fn aggregate_files_with_dedupe(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<BTreeMap<String, CodexGroup>> {
    if run.shared.single_thread {
        return aggregate_files(run, seen);
    }
    aggregate_files_parallel(run, seen)
}

fn aggregate_files(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<BTreeMap<String, CodexGroup>> {
    let mut groups = BTreeMap::new();
    let timezone =
        parse_tz(run.shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    for file in run.files {
        aggregate_file(run, file, timezone.as_ref(), seen, &mut groups)?;
    }
    Ok(groups)
}

fn aggregate_files_parallel(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<BTreeMap<String, CodexGroup>> {
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(run.files.len());
    if worker_count <= 1 {
        return aggregate_files(run, seen);
    }

    let chunks = crate::chunk_file_indexes_by_size(run.files, worker_count);
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                let mut groups = BTreeMap::new();
                let timezone = parse_tz(run.shared.timezone.as_deref())
                    .or_else(|| Some(JiffTimeZone::system()));
                for index in chunk {
                    aggregate_file(run, &run.files[index], timezone.as_ref(), seen, &mut groups)?;
                }
                Result::<BTreeMap<String, CodexGroup>>::Ok(groups)
            }));
        }

        let mut groups = BTreeMap::new();
        for handle in handles {
            merge_groups(
                &mut groups,
                handle
                    .join()
                    .map_err(|_| crate::cli_error("codex worker panicked"))??,
            );
        }
        Ok(groups)
    })
}

fn aggregate_file(
    run: &CodexAggregateRun<'_>,
    file: &Path,
    timezone: Option<&JiffTimeZone>,
    seen: &CodexDedupeShards,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    parser::visit_codex_session_file(
        run.sessions_dir,
        file,
        run.replay_plan.replay_prefix(file),
        |event| add_event_to_groups(&event, run.kind, timezone, run.shared, seen, groups),
    )
}

fn aggregate_files_local(run: &CodexAggregateRun<'_>) -> Result<BTreeMap<String, CodexGroup>> {
    let CodexAggregation { mut groups, seen } = aggregate_files_local_with_seen(run)?;
    apply_recorded_usage_entries(&mut groups, seen.iter(), run.shared, run.kind);
    Ok(groups)
}

fn aggregate_files_local_with_seen(run: &CodexAggregateRun<'_>) -> Result<CodexAggregation> {
    let mut aggregation = CodexAggregation {
        groups: BTreeMap::new(),
        seen: FxHashMap::default(),
    };
    let timezone =
        parse_tz(run.shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    for file in run.files {
        aggregate_file_local(run, file, timezone.as_ref(), &mut aggregation)?;
    }
    Ok(aggregation)
}

fn aggregate_file_local(
    run: &CodexAggregateRun<'_>,
    file: &Path,
    timezone: Option<&JiffTimeZone>,
    aggregation: &mut CodexAggregation,
) -> Result<()> {
    parser::visit_codex_session_file(
        run.sessions_dir,
        file,
        run.replay_plan.replay_prefix(file),
        |event| add_event_to_groups_local(&event, run.kind, timezone, run.shared, aggregation),
    )
}

fn add_event_to_groups(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    seen: &CodexDedupeShards,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    if !insert_event_key(event, timestamp, model.as_ref(), kind, seen) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        groups,
    )
}

fn add_event_to_groups_local(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    aggregation: &mut CodexAggregation,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    let key = codex_event_key(event, timestamp, model.as_ref(), kind);
    if !insert_dedupe_record(&mut aggregation.seen, key, event, model.as_ref(), kind) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        &mut aggregation.groups,
    )
}

fn add_deduped_event_to_groups(
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let Some(period) = codex_period_for(
        timestamp,
        Some(event.session_id.as_str()),
        kind,
        timezone,
        shared,
    ) else {
        return Ok(());
    };
    let group = groups.entry(period).or_default();
    accumulate_codex_event_into_group(group, event, model, false);
    Ok(())
}

fn codex_period_for(
    timestamp: crate::TimestampMs,
    session_id: Option<&str>,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
) -> Option<String> {
    let date = format_date_tz(timestamp, timezone);
    if shared.since.is_some() || shared.until.is_some() {
        let date_key = date.replace('-', "");
        if shared.since.as_ref().is_some_and(|since| &date_key < since)
            || shared.until.as_ref().is_some_and(|until| &date_key > until)
        {
            return None;
        }
    }
    Some(match kind {
        AgentReportKind::Daily => date,
        AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
        AgentReportKind::Monthly => date[..7].to_string(),
        AgentReportKind::Session => session_id?.to_string(),
    })
}

fn accumulate_codex_event_into_group(
    group: &mut CodexGroup,
    event: &CodexTokenUsageEvent,
    model: &str,
    record_service_tier: bool,
) {
    group.input_tokens += event.input_tokens;
    group.cached_input_tokens += event.cached_input_tokens;
    group.output_tokens += event.output_tokens;
    group.reasoning_output_tokens += event.reasoning_output_tokens;
    group.total_tokens += event.total_tokens;
    if group
        .last_activity
        .as_deref()
        .is_none_or(|current| event.timestamp.as_str() > current)
    {
        group.last_activity = Some(event.timestamp.clone());
    }

    let model_usage = group.models.entry(model.to_string()).or_default();
    model_usage.input_tokens += event.input_tokens;
    model_usage.cached_input_tokens += event.cached_input_tokens;
    model_usage.output_tokens += event.output_tokens;
    model_usage.reasoning_output_tokens += event.reasoning_output_tokens;
    model_usage.total_tokens += event.total_tokens;
    // Each event is one request, so its input size decides the pricing tier
    // here; the summed totals cannot recover per-request context sizes. The
    // boundary is per model (OpenAI's 272K models and any future tier with a
    // different threshold) rather than a single global constant, matching the
    // threshold used to price the long-context buckets.
    let is_long_context = event.input_tokens > crate::pricing::long_context_split_threshold(model);
    if is_long_context {
        model_usage.long_context_input_tokens += event.input_tokens;
        model_usage.long_context_cached_input_tokens += event.cached_input_tokens;
        model_usage.long_context_output_tokens += event.output_tokens;
    }
    if record_service_tier {
        let recorded_usage = match event.service_tier {
            Some(CodexServiceTier::Standard) => Some(&mut model_usage.recorded_standard_usage),
            Some(CodexServiceTier::Fast) => Some(&mut model_usage.recorded_fast_usage),
            None => None,
        };
        if let Some(recorded_usage) = recorded_usage {
            accumulate_codex_event_into_usage_bucket(recorded_usage, event, is_long_context);
        }
    }
    model_usage.is_fallback |= event.is_fallback_model;
}

fn accumulate_codex_event_into_usage_bucket(
    usage: &mut CodexUsageBucket,
    event: &CodexTokenUsageEvent,
    is_long_context: bool,
) {
    usage.input_tokens += event.input_tokens;
    usage.cached_input_tokens += event.cached_input_tokens;
    usage.output_tokens += event.output_tokens;
    if is_long_context {
        usage.long_context_input_tokens += event.input_tokens;
        usage.long_context_cached_input_tokens += event.cached_input_tokens;
        usage.long_context_output_tokens += event.output_tokens;
    }
}

fn merge_codex_usage_bucket(target: &mut CodexUsageBucket, source: CodexUsageBucket) {
    target.input_tokens += source.input_tokens;
    target.cached_input_tokens += source.cached_input_tokens;
    target.output_tokens += source.output_tokens;
    target.long_context_input_tokens += source.long_context_input_tokens;
    target.long_context_cached_input_tokens += source.long_context_cached_input_tokens;
    target.long_context_output_tokens += source.long_context_output_tokens;
}

fn apply_recorded_usage_from_shards(
    groups: &mut BTreeMap<String, CodexGroup>,
    seen: &CodexDedupeShards,
    shared: &SharedArgs,
    kind: AgentReportKind,
) {
    for shard in seen {
        let records = shard.lock().unwrap();
        apply_recorded_usage_entries(groups, records.iter(), shared, kind);
    }
}

fn apply_recorded_usage_entries<'a>(
    groups: &mut BTreeMap<String, CodexGroup>,
    records: impl IntoIterator<Item = (&'a CodexEventKey, &'a CodexDedupeRecord)>,
    shared: &SharedArgs,
    kind: AgentReportKind,
) {
    let timezone = parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    for (key, record) in records {
        let Some(service_tier) = record.service_tier else {
            continue;
        };
        let Some(period) = codex_period_for(
            key.timestamp,
            record.session_id.as_deref(),
            kind,
            timezone.as_ref(),
            shared,
        ) else {
            continue;
        };
        let Some(model_usage) = groups
            .get_mut(&period)
            .and_then(|group| group.models.get_mut(record.model.as_str()))
        else {
            continue;
        };
        let is_long_context =
            key.input_tokens > crate::pricing::long_context_split_threshold(record.model.as_str());
        let usage = CodexUsageBucket {
            input_tokens: key.input_tokens,
            cached_input_tokens: key.cached_input_tokens,
            output_tokens: key.output_tokens,
            long_context_input_tokens: if is_long_context { key.input_tokens } else { 0 },
            long_context_cached_input_tokens: if is_long_context {
                key.cached_input_tokens
            } else {
                0
            },
            long_context_output_tokens: if is_long_context {
                key.output_tokens
            } else {
                0
            },
        };
        let recorded_usage = match service_tier {
            CodexServiceTier::Standard => &mut model_usage.recorded_standard_usage,
            CodexServiceTier::Fast => &mut model_usage.recorded_fast_usage,
        };
        merge_codex_usage_bucket(recorded_usage, usage);
    }
}

fn create_dedupe_shards() -> Vec<Mutex<CodexDedupeMap>> {
    let shard_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    (0..shard_count.max(1))
        .map(|_| Mutex::new(FxHashMap::default()))
        .collect()
}

fn insert_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
    seen: &CodexDedupeShards,
) -> bool {
    let key = codex_event_key(event, timestamp, model, kind);
    let mut hasher = FxHasher::default();
    key.hash(&mut hasher);
    let shard_index = hasher.finish() as usize % seen.len();
    insert_dedupe_record(
        &mut seen[shard_index].lock().unwrap(),
        key,
        event,
        model,
        kind,
    )
}

fn insert_dedupe_record(
    seen: &mut CodexDedupeMap,
    key: CodexEventKey,
    event: &CodexTokenUsageEvent,
    model: &str,
    kind: AgentReportKind,
) -> bool {
    if let Some(record) = seen.get_mut(&key) {
        record.service_tier = merge_codex_service_tiers(record.service_tier, event.service_tier);
        return false;
    }
    seen.insert(
        key,
        CodexDedupeRecord {
            service_tier: event.service_tier,
            model: CompactString::new(model),
            session_id: (kind == AgentReportKind::Session)
                .then(|| CompactString::new(&event.session_id)),
        },
    );
    true
}

fn codex_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
) -> CodexEventKey {
    let (session_hash, session_len) = if kind == AgentReportKind::Session {
        (hash_text(&event.session_id), event.session_id.len())
    } else {
        (0, 0)
    };
    CodexEventKey {
        session_hash,
        session_len,
        timestamp,
        model_hash: hash_text(model),
        model_len: model.len(),
        input_tokens: event.input_tokens,
        cached_input_tokens: event.cached_input_tokens,
        output_tokens: event.output_tokens,
        reasoning_output_tokens: event.reasoning_output_tokens,
        total_tokens: event.total_tokens,
    }
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

fn merge_groups(target: &mut BTreeMap<String, CodexGroup>, source: BTreeMap<String, CodexGroup>) {
    for (period, group) in source {
        let target_group = target.entry(period).or_default();
        target_group.input_tokens += group.input_tokens;
        target_group.cached_input_tokens += group.cached_input_tokens;
        target_group.output_tokens += group.output_tokens;
        target_group.reasoning_output_tokens += group.reasoning_output_tokens;
        target_group.total_tokens += group.total_tokens;
        if target_group.last_activity.as_deref().is_none_or(|current| {
            group
                .last_activity
                .as_deref()
                .is_some_and(|next| next > current)
        }) {
            target_group.last_activity = group.last_activity;
        }
        for (model, usage) in group.models {
            let target_usage = target_group.models.entry(model).or_default();
            target_usage.input_tokens += usage.input_tokens;
            target_usage.cached_input_tokens += usage.cached_input_tokens;
            target_usage.output_tokens += usage.output_tokens;
            target_usage.reasoning_output_tokens += usage.reasoning_output_tokens;
            target_usage.total_tokens += usage.total_tokens;
            target_usage.long_context_input_tokens += usage.long_context_input_tokens;
            target_usage.long_context_cached_input_tokens += usage.long_context_cached_input_tokens;
            target_usage.long_context_output_tokens += usage.long_context_output_tokens;
            merge_codex_usage_bucket(
                &mut target_usage.recorded_standard_usage,
                usage.recorded_standard_usage,
            );
            merge_codex_usage_bucket(
                &mut target_usage.recorded_fast_usage,
                usage.recorded_fast_usage,
            );
            target_usage.is_fallback |= usage.is_fallback;
        }
    }
}

pub(crate) fn aggregate_events(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
) -> Result<BTreeMap<String, CodexGroup>> {
    let mut groups = BTreeMap::new();
    let timezone = parse_tz(timezone).or_else(|| Some(JiffTimeZone::system()));
    for event in events {
        let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
            continue;
        };
        let timestamp = parse_ts_timestamp(&event.timestamp).ok_or_else(|| {
            crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp))
        })?;
        let date = format_date_tz(timestamp, timezone.as_ref());
        let period = match kind {
            AgentReportKind::Daily => date,
            AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
            AgentReportKind::Monthly => date[..7].to_string(),
            AgentReportKind::Session => event.session_id.clone(),
        };
        let group = groups.entry(period).or_insert_with(CodexGroup::default);
        let model = crate::model_aliases::resolve_model_name(model);
        accumulate_codex_event_into_group(group, event, model.as_ref(), true);
    }
    Ok(groups)
}

pub(crate) fn filter_events_by_date(
    events: &mut Vec<CodexTokenUsageEvent>,
    shared: &SharedArgs,
) -> Result<()> {
    if shared.since.is_none() && shared.until.is_none() {
        return Ok(());
    }
    let timezone = parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    let mut kept = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let timestamp = parse_ts_timestamp(&event.timestamp).ok_or_else(|| {
            crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp))
        })?;
        let date = format_date_tz(timestamp, timezone.as_ref()).replace('-', "");
        if shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
        {
            kept.push(event);
        }
    }
    *events = kept;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use crate::{
        PricingMap, adapter::codex::paths::CodexUsageSource, cli::CodexSpeed,
        model_aliases::set_model_aliases_for_tests,
    };

    #[test]
    fn selects_codex_period_for_each_report_kind() {
        let timestamp = parse_ts_timestamp("2026-05-29T08:01:00.000Z").unwrap();
        let timezone = parse_tz(Some("UTC")).unwrap();
        let shared = SharedArgs::default();

        for (kind, expected) in [
            (AgentReportKind::Daily, "2026-05-29"),
            (AgentReportKind::Weekly, "2026-05-25"),
            (AgentReportKind::Monthly, "2026-05"),
            (AgentReportKind::Session, "sessions/child.jsonl"),
        ] {
            assert_eq!(
                codex_period_for(
                    timestamp,
                    Some("sessions/child.jsonl"),
                    kind,
                    Some(&timezone),
                    &shared,
                )
                .as_deref(),
                Some(expected),
            );
        }
    }

    #[test]
    fn omits_codex_period_outside_date_bounds() {
        let timezone = parse_tz(Some("UTC")).unwrap();
        let shared = SharedArgs {
            since: Some("20260528".to_string()),
            until: Some("20260530".to_string()),
            ..SharedArgs::default()
        };

        for timestamp in ["2026-05-27T23:59:59.000Z", "2026-05-31T00:00:00.000Z"] {
            assert_eq!(
                codex_period_for(
                    parse_ts_timestamp(timestamp).unwrap(),
                    Some("sessions/child.jsonl"),
                    AgentReportKind::Daily,
                    Some(&timezone),
                    &shared,
                ),
                None,
            );
        }
    }

    #[test]
    fn omits_session_period_without_session_id() {
        let timestamp = parse_ts_timestamp("2026-05-29T08:01:00.000Z").unwrap();
        let timezone = parse_tz(Some("UTC")).unwrap();

        assert_eq!(
            codex_period_for(
                timestamp,
                None,
                AgentReportKind::Session,
                Some(&timezone),
                &SharedArgs::default(),
            ),
            None,
        );
    }

    #[test]
    fn dedupes_copied_token_usage_across_session_files() {
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let service_tier_line = json!({
            "timestamp": "2026-05-29T08:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "priority" },
            },
        })
        .to_string();
        let rollout = format!("{service_tier_line}\n{usage_line}");
        let unclassified_first = fs_fixture!({
            "sessions/a-unclassified.jsonl": &usage_line,
            "sessions/z-recorded.jsonl": &rollout,
        });
        let recorded_first = fs_fixture!({
            "sessions/a-recorded.jsonl": &rollout,
            "sessions/z-unclassified.jsonl": &usage_line,
        });
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.2": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "cache_read_input_token_cost": 0.0000005,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
        );
        let mut costs = Vec::new();
        for fixture in [&unclassified_first, &recorded_first] {
            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };

                let groups = load_groups_from_directory(
                    &fixture.path("sessions"),
                    &shared,
                    AgentReportKind::Daily,
                )
                .unwrap();

                assert_eq!(groups.len(), 1);
                let group = groups.get("2026-05-29").unwrap();
                assert_eq!(group.input_tokens, 1_000);
                assert_eq!(group.cached_input_tokens, 100);
                assert_eq!(group.output_tokens, 200);
                assert_eq!(group.reasoning_output_tokens, 20);
                assert_eq!(group.total_tokens, 1_200);
                assert_eq!(
                    group.models["gpt-5.2"].recorded_fast_usage.input_tokens,
                    1_000
                );
                costs.push(crate::adapter::codex::calculate_group_cost(
                    group,
                    &pricing,
                    CodexSpeed::Auto,
                ));
            }
        }
        assert!(costs.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn resolves_conflicting_duplicate_tiers_as_standard() {
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let rollout = |service_tier: &str| {
            format!(
                "{}\n{usage_line}",
                json!({
                    "timestamp": "2026-05-29T08:00:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": service_tier },
                    },
                })
            )
        };
        let standard = rollout("default");
        let fast = rollout("priority");
        let standard_first = fs_fixture!({
            "sessions/a-standard.jsonl": &standard,
            "sessions/z-fast.jsonl": &fast,
        });
        let fast_first = fs_fixture!({
            "sessions/a-fast.jsonl": &fast,
            "sessions/z-standard.jsonl": &standard,
        });

        for fixture in [&standard_first, &fast_first] {
            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };
                let groups = load_groups_from_directory(
                    &fixture.path("sessions"),
                    &shared,
                    AgentReportKind::Daily,
                )
                .unwrap();
                let usage = &groups["2026-05-29"].models["gpt-5.2"];

                assert_eq!(usage.input_tokens, 1_000);
                assert_eq!(usage.recorded_standard_usage.input_tokens, 1_000);
                assert_eq!(usage.recorded_fast_usage.input_tokens, 0);
            }
        }
    }

    #[test]
    fn tracks_long_context_token_split_per_request() {
        let usage_line = |input: u64, cached: u64, output: u64| {
            json!({
                "timestamp": "2026-07-09T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": cached,
                            "output_tokens": output,
                            "reasoning_output_tokens": 0,
                            "total_tokens": input + output,
                        },
                    },
                },
            })
            .to_string()
        };
        // One request above the 272K input threshold and one below it.
        let long_line = usage_line(280_000, 20_000, 500);
        let short_line = usage_line(100_000, 50_000, 300);
        let fast_marker = json!({
            "timestamp": "2026-07-09T08:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "priority" },
            },
        })
        .to_string();
        let standard_marker = json!({
            "timestamp": "2026-07-09T08:02:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "default" },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &format!(
                "{fast_marker}\n{long_line}\n{standard_marker}\n{short_line}"
            ),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        let group = groups.get("2026-07-09").unwrap();
        let usage = group.models.get("gpt-5.6-sol").unwrap();
        assert_eq!(usage.input_tokens, 380_000);
        assert_eq!(usage.cached_input_tokens, 70_000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.long_context_input_tokens, 280_000);
        assert_eq!(usage.long_context_cached_input_tokens, 20_000);
        assert_eq!(usage.long_context_output_tokens, 500);
        assert_eq!(usage.recorded_fast_usage.input_tokens, 280_000);
        assert_eq!(usage.recorded_fast_usage.cached_input_tokens, 20_000);
        assert_eq!(usage.recorded_fast_usage.output_tokens, 500);
        assert_eq!(usage.recorded_fast_usage.long_context_input_tokens, 280_000);
        assert_eq!(
            usage.recorded_fast_usage.long_context_cached_input_tokens,
            20_000
        );
        assert_eq!(usage.recorded_fast_usage.long_context_output_tokens, 500);
        assert_eq!(usage.recorded_standard_usage.input_tokens, 100_000);
        assert_eq!(usage.recorded_standard_usage.cached_input_tokens, 50_000);
        assert_eq!(usage.recorded_standard_usage.output_tokens, 300);
    }

    #[test]
    fn parallel_merge_preserves_speed_and_long_context_buckets() {
        let rollout = |marker_timestamp: &str,
                       usage_timestamp: &str,
                       service_tier: &str,
                       input_tokens: u64,
                       cached_input_tokens: u64,
                       output_tokens: u64| {
            [
                json!({
                    "timestamp": marker_timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": service_tier },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": usage_timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-sol",
                            "last_token_usage": {
                                "input_tokens": input_tokens,
                                "cached_input_tokens": cached_input_tokens,
                                "output_tokens": output_tokens,
                                "total_tokens": input_tokens + output_tokens,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n")
        };
        let fast_long = rollout(
            "2026-07-09T08:00:00.000Z",
            "2026-07-09T08:01:00.000Z",
            "priority",
            280_000,
            20_000,
            500,
        );
        let standard_short = rollout(
            "2026-07-09T09:00:00.000Z",
            "2026-07-09T09:01:00.000Z",
            "default",
            100_000,
            50_000,
            300,
        );
        let fixture = fs_fixture!({
            "sessions/fast.jsonl": &fast_long,
            "sessions/standard.jsonl": &standard_short,
        });
        let mut observed = Vec::new();

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };
            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();
            let usage = &groups["2026-07-09"].models["gpt-5.6-sol"];

            assert_eq!(usage.input_tokens, 380_000);
            assert_eq!(usage.long_context_input_tokens, 280_000);
            assert_eq!(usage.recorded_fast_usage.input_tokens, 280_000);
            assert_eq!(usage.recorded_fast_usage.long_context_input_tokens, 280_000);
            assert_eq!(usage.recorded_standard_usage.input_tokens, 100_000);
            assert_eq!(usage.recorded_standard_usage.long_context_input_tokens, 0);
            observed.push((usage.recorded_fast_usage, usage.recorded_standard_usage));
        }

        assert_eq!(observed[0], observed[1]);
    }

    #[test]
    fn dedupes_copied_token_usage_after_model_alias_resolution() {
        let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.2")]);
        let private_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "private-alpha",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let canonical_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &private_usage_line,
            "sessions/goal.jsonl": &canonical_usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();

            let group = groups.get("2026-05-29").unwrap();
            assert_eq!(group.input_tokens, 1_000);
            assert_eq!(group.models.len(), 1);
            assert_eq!(group.models["gpt-5.2"].input_tokens, 1_000);
        }
    }

    #[test]
    fn keeps_matching_token_usage_in_distinct_session_groups() {
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &usage_line,
            "sessions/goal.jsonl": &usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Session,
            )
            .unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["root"].input_tokens, 1_000);
            assert_eq!(groups["goal"].input_tokens, 1_000);
        }
    }

    #[test]
    fn aggregates_active_copy_when_archived_file_has_same_relative_path() {
        let active_usage = [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 111,
                            "cached_input_tokens": 10,
                            "output_tokens": 20,
                            "reasoning_output_tokens": 1,
                            "total_tokens": 131,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let archived_usage = [
            json!({
                "timestamp": "2026-05-12T09:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T09:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 999,
                            "cached_input_tokens": 90,
                            "output_tokens": 80,
                            "reasoning_output_tokens": 7,
                            "total_tokens": 1_079,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "codex/sessions/duplicate.jsonl": active_usage,
            "codex/archived_sessions/duplicate.jsonl": archived_usage,
            "codex/archived_sessions/archived-only.jsonl": [
                json!({
                    "timestamp": "2026-05-13T08:00:00.000Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "gpt-5.2",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-13T08:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 222,
                                "cached_input_tokens": 20,
                                "output_tokens": 30,
                                "reasoning_output_tokens": 2,
                                "total_tokens": 252,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                ..SharedArgs::default()
            };
            let sources = vec![
                CodexUsageSource::new_for_test(
                    fixture.path("codex/sessions"),
                    fixture.path("codex"),
                ),
                CodexUsageSource::new_for_test(
                    fixture.path("codex/archived_sessions"),
                    fixture.path("codex"),
                ),
            ];
            let groups =
                load_groups_from_sources(&sources, &shared, AgentReportKind::Daily).unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["2026-05-12"].input_tokens, 111);
            assert_eq!(groups["2026-05-13"].input_tokens, 222);
        }
    }
}
