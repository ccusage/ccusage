use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use jiff::tz::TimeZone as JiffTimeZone;
use rustc_hash::FxHasher;

use crate::{
    CodexGroup, CodexTokenUsageEvent, Result,
    cli::{AgentReportKind, SharedArgs, WeekDay},
    fast::FxHashSet,
    format_date_tz, parse_ts_timestamp, parse_tz, wants_json, week_start,
};

use super::{parser, paths, replay::CodexReplayPlan};

type CodexEventKey = (
    u64,
    usize,
    crate::TimestampMs,
    u64,
    usize,
    u64,
    u64,
    u64,
    u64,
    u64,
);
type CodexDedupeShards = [Mutex<FxHashSet<CodexEventKey>>];

struct CodexAggregation {
    groups: BTreeMap<String, CodexGroup>,
    seen: FxHashSet<CodexEventKey>,
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
    aggregate_files_parallel(&run, &seen)
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
    Ok(aggregate_files_local_with_seen(run)?.groups)
}

fn aggregate_files_local_with_seen(run: &CodexAggregateRun<'_>) -> Result<CodexAggregation> {
    let mut aggregation = CodexAggregation {
        groups: BTreeMap::new(),
        seen: FxHashSet::default(),
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
    if !aggregation
        .seen
        .insert(codex_event_key(event, timestamp, model.as_ref(), kind))
    {
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
    let date = format_date_tz(timestamp, timezone);
    if shared.since.is_some() || shared.until.is_some() {
        let date_key = date.replace('-', "");
        if shared.since.as_ref().is_some_and(|since| &date_key < since)
            || shared.until.as_ref().is_some_and(|until| &date_key > until)
        {
            return Ok(());
        }
    }
    let period = match kind {
        AgentReportKind::Daily => date,
        AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
        AgentReportKind::Monthly => date[..7].to_string(),
        AgentReportKind::Session => event.session_id.clone(),
    };
    let group = groups.entry(period).or_default();
    accumulate_codex_event_into_group(group, event, model);
    Ok(())
}

fn accumulate_codex_event_into_group(
    group: &mut CodexGroup,
    event: &CodexTokenUsageEvent,
    model: &str,
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
    if event.input_tokens > crate::pricing::long_context_split_threshold(model) {
        model_usage.long_context_input_tokens += event.input_tokens;
        model_usage.long_context_cached_input_tokens += event.cached_input_tokens;
        model_usage.long_context_output_tokens += event.output_tokens;
    }
    model_usage.is_fallback |= event.is_fallback_model;
}

fn create_dedupe_shards() -> Vec<Mutex<FxHashSet<CodexEventKey>>> {
    let shard_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    (0..shard_count.max(1))
        .map(|_| Mutex::new(FxHashSet::default()))
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
    seen[shard_index].lock().unwrap().insert(key)
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
    (
        session_hash,
        session_len,
        timestamp,
        hash_text(model),
        model.len(),
        event.input_tokens,
        event.cached_input_tokens,
        event.output_tokens,
        event.reasoning_output_tokens,
        event.total_tokens,
    )
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
        accumulate_codex_event_into_group(group, event, model.as_ref());
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
        adapter::codex::paths::CodexUsageSource, model_aliases::set_model_aliases_for_tests,
    };

    #[test]
    fn skips_forked_parent_prefix_rewritten_across_seconds() {
        fn token_count(timestamp: &str, input: u64, total_input: u64) -> String {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.2",
                        "last_token_usage": {
                            "input_tokens": input,
                            "output_tokens": 1,
                            "total_tokens": input + 1,
                        },
                        "total_token_usage": {
                            "input_tokens": total_input,
                            "output_tokens": 1,
                            "total_tokens": total_input + 1,
                        },
                    },
                },
            })
            .to_string()
        }

        let fixture = fs_fixture!({
            "sessions/parent.jsonl": [
                json!({
                    "type": "session_meta",
                    "payload": {"id": "parent"},
                })
                .to_string(),
                token_count("2026-07-10T08:01:00.000Z", 100, 100),
                token_count("2026-07-10T08:02:00.000Z", 200, 300),
            ]
            .join("\n"),
            "sessions/child.jsonl": [
                json!({
                    "type": "session_meta",
                    "payload": {"id": "child", "forked_from_id": "parent"},
                })
                .to_string(),
                token_count("2026-07-10T09:00:00.100Z", 100, 100),
                token_count("2026-07-10T09:00:01.100Z", 200, 300),
                token_count("2026-07-10T09:00:02.100Z", 50, 350),
            ]
            .join("\n"),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        let group = groups.get("2026-07-10").unwrap();
        assert_eq!(group.input_tokens, 350);
        assert_eq!(group.total_tokens, 353);
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
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &format!("{long_line}\n{short_line}"),
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
