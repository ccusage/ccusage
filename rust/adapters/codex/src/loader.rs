use std::{
    path::{Path, PathBuf},
    thread,
};

use compact_str::CompactString;

use crate::{
    CodexTokenUsageEvent, Result, chunk_file_indexes_by_size, cli::SharedArgs, fast::FxHashMap,
    merge_codex_service_tiers, progress,
};

use super::{
    parser::visit_codex_session_file,
    paths,
    paths::{
        CodexUsageSource, codex_usage_sources, collect_codex_usage_files,
        collect_deduped_codex_usage_files,
    },
    replay::CodexReplayPlan,
    source::normalize_codex_originator,
};

pub fn load_codex_events_from_directory(
    sessions_dir: &Path,
    single_thread: bool,
) -> Result<Vec<CodexTokenUsageEvent>> {
    let files = collect_codex_usage_files(sessions_dir);
    let replay_plan = CodexReplayPlan::new([(sessions_dir, files.as_slice())], single_thread);
    let mut events =
        read_codex_events_from_files(sessions_dir, &files, single_thread, &replay_plan);
    dedupe_codex_events(&mut events);
    Ok(events)
}

/// Loads Codex usage events and reports whether any source file existed before
/// date filtering narrowed the files to parse.
pub fn load_codex_events_with_detection(
    shared: &SharedArgs,
) -> Result<(Vec<CodexTokenUsageEvent>, bool)> {
    progress::track_usage_load(progress::UsageLoadAgent("Codex"), shared.json, || {
        load_codex_events_from_sources_with_shared(&codex_usage_sources()?, shared)
    })
}

#[cfg(test)]
fn load_codex_events_from_sources(
    sources: &[CodexUsageSource],
    single_thread: bool,
) -> Result<Vec<CodexTokenUsageEvent>> {
    load_codex_events_from_sources_with_files(sources, single_thread, None)
        .map(|(events, _)| events)
}

fn load_codex_events_from_sources_with_shared(
    sources: &[CodexUsageSource],
    shared: &SharedArgs,
) -> Result<(Vec<CodexTokenUsageEvent>, bool)> {
    load_codex_events_from_sources_with_files(sources, shared.single_thread, Some(shared))
}

fn load_codex_events_from_sources_with_files(
    sources: &[CodexUsageSource],
    single_thread: bool,
    shared: Option<&SharedArgs>,
) -> Result<(Vec<CodexTokenUsageEvent>, bool)> {
    if let [source] = sources {
        if let Some(shared) = shared {
            return load_codex_events_from_directory_with_shared(&source.dir, shared);
        }
        let files = collect_codex_usage_files(&source.dir);
        let replay_plan =
            CodexReplayPlan::new([(source.dir.as_path(), files.as_slice())], single_thread);
        let mut events =
            read_codex_events_from_files(&source.dir, &files, single_thread, &replay_plan);
        dedupe_codex_events(&mut events);
        return Ok((events, false));
    }

    let groups = collect_deduped_codex_usage_files(sources);
    let detected_before_filter = groups.iter().any(|group| !group.files.is_empty());
    let files_by_group = groups
        .iter()
        .map(|group| {
            shared.map_or_else(
                || group.files.clone(),
                |shared| paths::filter_codex_usage_files(&group.dir, &group.files, shared),
            )
        })
        .collect::<Vec<_>>();
    let replay_plan = if shared.is_some_and(has_date_bounds) {
        CodexReplayPlan::for_bounded_files(
            groups
                .iter()
                .zip(&files_by_group)
                .map(|(group, files)| (group.dir.as_path(), files.as_slice())),
            groups
                .iter()
                .map(|group| (group.dir.as_path(), group.files.as_slice())),
            single_thread,
        )
    } else {
        CodexReplayPlan::new(
            groups
                .iter()
                .map(|group| (group.dir.as_path(), group.files.as_slice())),
            single_thread,
        )
    };
    let mut events = Vec::new();
    for (group, files) in groups.into_iter().zip(files_by_group) {
        let mut source_events =
            read_codex_events_from_files(&group.dir, &files, single_thread, &replay_plan);
        events.append(&mut source_events);
    }
    dedupe_codex_events(&mut events);
    let detected = if shared.is_some_and(has_date_bounds) {
        detected_before_filter
    } else {
        !events.is_empty()
    };
    Ok((events, detected))
}

fn load_codex_events_from_directory_with_shared(
    sessions_dir: &Path,
    shared: &SharedArgs,
) -> Result<(Vec<CodexTokenUsageEvent>, bool)> {
    let all_files = collect_codex_usage_files(sessions_dir);
    let files = paths::filter_codex_usage_files(sessions_dir, &all_files, shared);
    let replay_plan = if has_date_bounds(shared) {
        CodexReplayPlan::for_bounded_files(
            [(sessions_dir, files.as_slice())],
            [(sessions_dir, all_files.as_slice())],
            shared.single_thread,
        )
    } else {
        CodexReplayPlan::new([(sessions_dir, all_files.as_slice())], shared.single_thread)
    };
    let mut events =
        read_codex_events_from_files(sessions_dir, &files, shared.single_thread, &replay_plan);
    dedupe_codex_events(&mut events);
    let detected = if has_date_bounds(shared) {
        !all_files.is_empty()
    } else {
        !events.is_empty()
    };
    Ok((events, detected))
}

fn read_codex_events_from_files(
    sessions_dir: &Path,
    files: &[PathBuf],
    single_thread: bool,
    replay_plan: &CodexReplayPlan,
) -> Vec<CodexTokenUsageEvent> {
    if single_thread {
        files
            .iter()
            .flat_map(|file| read_codex_session_file(sessions_dir, file, replay_plan))
            .collect()
    } else {
        read_codex_session_files_parallel(sessions_dir, files, replay_plan)
    }
}

fn has_date_bounds(shared: &SharedArgs) -> bool {
    shared.since.is_some() || shared.until.is_some()
}

fn read_codex_session_files_parallel(
    sessions_dir: &Path,
    files: &[PathBuf],
    replay_plan: &CodexReplayPlan,
) -> Vec<CodexTokenUsageEvent> {
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(files.len());
    if worker_count <= 1 {
        return files
            .iter()
            .flat_map(|file| read_codex_session_file(sessions_dir, file, replay_plan))
            .collect();
    }

    let chunks = chunk_file_indexes_by_size(files, worker_count);
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|index| {
                        (
                            index,
                            read_codex_session_file(sessions_dir, &files[index], replay_plan),
                        )
                    })
                    .collect::<Vec<_>>()
            }));
        }

        let mut loaded_files = Vec::with_capacity(files.len());
        loaded_files.resize_with(files.len(), || None);
        for (index, events) in handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("codex worker panicked"))
        {
            loaded_files[index] = Some(events);
        }
        loaded_files
            .into_iter()
            .flatten()
            .flatten()
            .collect::<Vec<_>>()
    })
}

fn read_codex_session_file(
    sessions_dir: &Path,
    path: &Path,
    replay_plan: &CodexReplayPlan,
) -> Vec<CodexTokenUsageEvent> {
    let mut events = Vec::new();
    let _ = visit_codex_session_file(
        sessions_dir,
        path,
        replay_plan.replay_prefix(path),
        |event| {
            events.push(event);
            Ok(())
        },
    );
    events
}

fn dedupe_codex_events(events: &mut Vec<CodexTokenUsageEvent>) {
    let mut indexes = FxHashMap::<_, usize>::default();
    let mut deduped = Vec::<CodexTokenUsageEvent>::with_capacity(events.len());
    for event in events.drain(..) {
        let key = (
            CompactString::new(&event.timestamp),
            event.model.as_deref().map(CompactString::new),
            event.input_tokens,
            event.cached_input_tokens,
            event.cache_creation_tokens,
            event.output_tokens,
            event.reasoning_output_tokens,
            event.total_tokens,
            CompactString::new(normalize_codex_originator(event.source.as_deref())),
        );
        if let Some(index) = indexes.get(&key).copied() {
            let retained = &mut deduped[index];
            retained.service_tier =
                merge_codex_service_tiers(retained.service_tier, event.service_tier);
        } else {
            indexes.insert(key, deduped.len());
            deduped.push(event);
        }
    }
    *events = deduped;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use crate::paths::CodexUsageSource;

    fn codex_event(session_id: &str) -> CodexTokenUsageEvent {
        CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            timestamp: "2026-01-02T00:00:00.000Z".to_string(),
            model: Some("gpt-5".to_string()),
            input_tokens: 100,
            cached_input_tokens: 10,
            cache_creation_tokens: 0,
            output_tokens: 50,
            reasoning_output_tokens: 0,
            total_tokens: 150,
            is_fallback_model: false,
            service_tier: None,
            source: None,
        }
    }

    #[test]
    fn reads_codex_originator_from_top_level_session_metadata() {
        let token_count = |timestamp: &str| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5",
                        "last_token_usage": {
                            "input_tokens": 10,
                            "output_tokens": 1,
                            "total_tokens": 11,
                        },
                    },
                },
            })
            .to_string()
        };
        let session_meta = |originator: Option<&str>| {
            let mut payload = json!({ "id": "session" });
            if let Some(originator) = originator {
                payload["originator"] = json!(originator);
            }
            json!({
                "timestamp": "2026-07-22T00:00:00.000Z",
                "type": "session_meta",
                "payload": payload,
            })
            .to_string()
        };
        let fixture = fs_fixture!({
            "cli.jsonl": [
                session_meta(Some("codex-tui")),
                token_count("2026-07-22T00:00:01.000Z"),
            ].join("\n"),
            "exec.jsonl": [
                session_meta(Some("codex_exec")),
                token_count("2026-07-22T00:00:02.000Z"),
            ].join("\n"),
            "desktop.jsonl": [
                session_meta(Some("Codex Desktop")),
                token_count("2026-07-22T00:00:03.000Z"),
            ].join("\n"),
            "empty.jsonl": [
                session_meta(Some("")),
                token_count("2026-07-22T00:00:04.000Z"),
            ].join("\n"),
            "unknown.jsonl": [
                session_meta(Some("future-client")),
                token_count("2026-07-22T00:00:05.000Z"),
            ].join("\n"),
            "missing.jsonl": token_count("2026-07-22T00:00:06.000Z"),
            "nested.jsonl": json!({
                "timestamp": "2026-07-22T00:00:07.000Z",
                "type": "event_msg",
                "payload": {
                    "originator": "nested-content-is-not-metadata",
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5",
                        "last_token_usage": {
                            "input_tokens": 10,
                            "output_tokens": 1,
                            "total_tokens": 11,
                        },
                    },
                },
            }).to_string(),
            "whitespace.jsonl": [
                r#"{ "timestamp": "2026-07-22T00:00:08.000Z", "type": "session_meta", "payload": { "originator": "codex_exec" } }"#.to_string(),
                token_count("2026-07-22T00:00:09.000Z"),
            ].join("\n"),
        });

        let sources = load_codex_events_from_directory(fixture.root(), true)
            .unwrap()
            .into_iter()
            .map(|event| (event.session_id, event.source))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(sources["cli"].as_deref(), Some("CLI"));
        assert_eq!(sources["exec"].as_deref(), Some("Exec"));
        assert_eq!(sources["desktop"].as_deref(), Some("Desktop App"));
        assert_eq!(sources["empty"].as_deref(), Some("Uncategorized"));
        assert_eq!(sources["unknown"].as_deref(), Some("future-client"));
        assert_eq!(sources["missing"], None);
        assert_eq!(sources["nested"], None);
        assert_eq!(sources["whitespace"].as_deref(), Some("Exec"));
    }

    #[test]
    fn records_service_tier_transitions_for_following_usage() {
        let service_tier = |timestamp: &str, value: &str| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": {
                        "service_tier": value,
                    },
                },
            })
            .to_string()
        };
        let token_count = |timestamp: &str, input_tokens: u64| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "cached_input_tokens": 0,
                            "output_tokens": 1,
                            "total_tokens": input_tokens + 1,
                        },
                    },
                },
            })
            .to_string()
        };
        let fixture = fs_fixture!({
            "session.jsonl": [
                service_tier("2026-07-22T00:00:00.000Z", "default"),
                token_count("2026-07-22T00:00:01.000Z", 10),
                service_tier("2026-07-22T00:00:02.000Z", "priority"),
                token_count("2026-07-22T00:00:03.000Z", 20),
                service_tier("2026-07-22T00:00:04.000Z", "default"),
                token_count("2026-07-22T00:00:05.000Z", 30),
                service_tier("2026-07-22T00:00:06.000Z", "fast"),
                token_count("2026-07-22T00:00:07.000Z", 40),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 4);
        assert_eq!(
            events[0].service_tier,
            Some(crate::CodexServiceTier::Standard)
        );
        assert_eq!(events[1].service_tier, Some(crate::CodexServiceTier::Fast));
        assert_eq!(
            events[2].service_tier,
            Some(crate::CodexServiceTier::Standard)
        );
        assert_eq!(events[3].service_tier, Some(crate::CodexServiceTier::Fast));
    }

    /// Codex emits `thread_settings_applied` without a `service_tier` key for
    /// auto-review threads. Such an event says nothing about the tier, so usage
    /// after it keeps the tier the rollout already recorded. A tier that is
    /// present but unrecognized is the opposite case and clears it.
    #[test]
    fn keeps_recorded_service_tier_when_a_later_settings_event_omits_it() {
        let settings = |timestamp: &str, thread_settings: serde_json::Value| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": thread_settings,
                },
            })
            .to_string()
        };
        let token_count = |timestamp: &str, input_tokens: u64| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "cached_input_tokens": 0,
                            "output_tokens": 1,
                            "total_tokens": input_tokens + 1,
                        },
                    },
                },
            })
            .to_string()
        };
        let fixture = fs_fixture!({
            "session.jsonl": [
                settings("2026-07-22T00:00:00.000Z", json!({"service_tier": "priority"})),
                token_count("2026-07-22T00:00:01.000Z", 10),
                // Auto-review thread settings: no service_tier key at all.
                settings("2026-07-22T00:00:02.000Z", json!({"model": "codex-auto-review"})),
                token_count("2026-07-22T00:00:03.000Z", 20),
                // Recognized tier again, then an unrecognized one that clears it.
                settings("2026-07-22T00:00:04.000Z", json!({"service_tier": "standard"})),
                token_count("2026-07-22T00:00:05.000Z", 30),
                settings("2026-07-22T00:00:06.000Z", json!({"service_tier": "flex"})),
                token_count("2026-07-22T00:00:07.000Z", 40),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].service_tier, Some(crate::CodexServiceTier::Fast));
        assert_eq!(
            events[1].service_tier,
            Some(crate::CodexServiceTier::Fast),
            "an omitted service_tier must not clear the recorded tier"
        );
        assert_eq!(
            events[2].service_tier,
            Some(crate::CodexServiceTier::Standard)
        );
        assert_eq!(
            events[3].service_tier, None,
            "an unrecognized service_tier must clear the recorded tier"
        );
    }

    #[test]
    fn leaves_unmarked_and_unsupported_service_tiers_unclassified() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2026-07-22T00:00:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-sol",
                            "last_token_usage": {
                                "input_tokens": 10,
                                "output_tokens": 1,
                                "total_tokens": 11,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-07-22T00:00:01.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": "priority" },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-07-22T00:00:02.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": "flex" },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-07-22T00:00:03.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-sol",
                            "last_token_usage": {
                                "input_tokens": 20,
                                "output_tokens": 1,
                                "total_tokens": 21,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].service_tier, None);
        assert_eq!(events[1].service_tier, None);
    }

    #[test]
    fn dedupes_matching_codex_usage_events_from_distinct_sessions() {
        let mut events = vec![codex_event("session-a"), codex_event("session-b")];

        dedupe_codex_events(&mut events);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "session-a");
    }

    #[test]
    fn preserves_recorded_tier_when_deduping_matching_events() {
        let unclassified = codex_event("session-a");
        let mut fast = codex_event("session-b");
        fast.service_tier = Some(crate::CodexServiceTier::Fast);

        for mut events in [
            vec![unclassified.clone(), fast.clone()],
            vec![fast.clone(), unclassified.clone()],
        ] {
            dedupe_codex_events(&mut events);

            assert_eq!(events.len(), 1);
            assert_eq!(events[0].service_tier, Some(crate::CodexServiceTier::Fast));
        }
    }

    #[test]
    fn resolves_conflicting_recorded_tiers_deterministically() {
        let mut standard = codex_event("session-a");
        standard.service_tier = Some(crate::CodexServiceTier::Standard);
        let mut fast = codex_event("session-b");
        fast.service_tier = Some(crate::CodexServiceTier::Fast);

        for mut events in [
            vec![standard.clone(), fast.clone()],
            vec![fast.clone(), standard.clone()],
        ] {
            dedupe_codex_events(&mut events);

            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].service_tier,
                Some(crate::CodexServiceTier::Standard)
            );
        }
    }

    #[test]
    fn keeps_cross_originator_duplicates_distinct_across_loading_paths() {
        let usage = |originator: &str| {
            [
                json!({
                    "timestamp": "2026-01-02T00:00:00.000Z",
                    "type": "session_meta",
                    "payload": { "originator": originator },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-01-02T00:00:01.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5",
                            "last_token_usage": {
                                "input_tokens": 100,
                                "output_tokens": 50,
                                "total_tokens": 150,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n")
        };
        let fixture = fs_fixture!({
            "sessions/2026/01/02/cli.jsonl": &usage("codex-tui"),
            "sessions/2026/01/02/exec.jsonl": &usage("codex_exec"),
        });
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let bounded = SharedArgs {
            since: Some("20260102".to_string()),
            until: Some("20260102".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let mut loaded = Vec::new();
        for single_thread in [true, false] {
            loaded.push(
                load_codex_events_from_directory(&fixture.path("sessions"), single_thread).unwrap(),
            );
            loaded.push(
                load_codex_events_from_sources_with_shared(
                    &sources,
                    &SharedArgs {
                        single_thread,
                        ..bounded.clone()
                    },
                )
                .unwrap()
                .0,
            );
        }

        for events in &loaded {
            assert_eq!(events.len(), 2);
            assert_eq!(
                events
                    .iter()
                    .map(|event| event.source.as_deref())
                    .collect::<Vec<_>>(),
                vec![Some("CLI"), Some("Exec")]
            );
            assert_eq!(
                events.iter().map(|event| event.input_tokens).sum::<u64>(),
                200
            );
            assert_eq!(
                events.iter().map(|event| event.total_tokens).sum::<u64>(),
                300
            );
        }
        for pair in loaded.windows(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn dedupes_copied_branch_history_across_session_files() {
        let parent_history = [
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
                            "input_tokens": 1_000,
                            "cached_input_tokens": 100,
                            "output_tokens": 200,
                            "reasoning_output_tokens": 20,
                            "total_tokens": 1_200,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let branch_history = [
            parent_history.as_str(),
            &json!({
                "timestamp": "2026-05-12T08:02:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 1_600,
                            "cached_input_tokens": 300,
                            "output_tokens": 450,
                            "reasoning_output_tokens": 40,
                            "total_tokens": 2_050,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "2026-05-12T08-00-00-parent.jsonl": &parent_history,
            "2026-05-12T08-02-00-branch.jsonl": branch_history,
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();

            assert_eq!(events.len(), 2);
            assert_eq!(events[0].session_id, "2026-05-12T08-00-00-parent");
            assert_eq!(events[0].input_tokens, 1_000);
            assert_eq!(events[0].cached_input_tokens, 100);
            assert_eq!(events[0].output_tokens, 200);
            assert_eq!(events[0].reasoning_output_tokens, 20);
            assert_eq!(events[0].total_tokens, 1_200);
            assert_eq!(events[1].session_id, "2026-05-12T08-02-00-branch");
            assert_eq!(events[1].input_tokens, 600);
            assert_eq!(events[1].cached_input_tokens, 200);
            assert_eq!(events[1].output_tokens, 250);
            assert_eq!(events[1].reasoning_output_tokens, 20);
            assert_eq!(events[1].total_tokens, 850);
        }
    }

    #[test]
    fn loads_saved_codex_exec_json_usage() {
        let fixture = fs_fixture!({
            "run.jsonl": [
                json!({
                    "type": "turn.completed",
                    "timestamp": "2026-01-02T03:04:05.000Z",
                    "model": "gpt-5.2-codex",
                    "usage": {
                        "input_tokens": 120,
                        "cached_input_tokens": 20,
                        "output_tokens": 30,
                        "total_tokens": 150,
                    },
                })
                .to_string(),
                json!({
                    "type": "result",
                    "data": {
                        "timestamp": "2026-01-02T03:05:05.000Z",
                        "model_name": "gpt-5.2-codex",
                        "usage": {
                            "prompt_tokens": 50,
                            "cached_tokens": 5,
                            "completion_tokens": 12,
                        },
                    },
                })
                .to_string(),
                json!({
                    "type": "turn.completed",
                    "timestamp": "2026-01-02T03:06:05.000Z",
                    "model": "gpt-5.2-codex",
                    "usage": {
                        "input_tokens": 9,
                        "output_tokens": 4,
                        "reasoning_output_tokens": 1,
                        "total_tokens": 0,
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].session_id, "run");
        assert_eq!(events[0].timestamp, "2026-01-02T03:04:05.000Z");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[0].input_tokens, 120);
        assert_eq!(events[0].cached_input_tokens, 20);
        assert_eq!(events[0].output_tokens, 30);
        assert_eq!(events[0].total_tokens, 150);
        assert_eq!(events[0].source.as_deref(), Some("Exec"));
        assert_eq!(events[1].timestamp, "2026-01-02T03:05:05.000Z");
        assert_eq!(events[1].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[1].input_tokens, 50);
        assert_eq!(events[1].cached_input_tokens, 5);
        assert_eq!(events[1].output_tokens, 12);
        assert_eq!(events[1].total_tokens, 62);
        assert_eq!(events[1].source.as_deref(), Some("Exec"));
        assert_eq!(events[2].timestamp, "2026-01-02T03:06:05.000Z");
        assert_eq!(events[2].input_tokens, 9);
        assert_eq!(events[2].output_tokens, 4);
        assert_eq!(events[2].reasoning_output_tokens, 1);
        assert_eq!(events[2].total_tokens, 13);
        assert_eq!(events[2].source.as_deref(), Some("Exec"));
    }

    #[test]
    fn loads_active_copy_when_archived_file_has_same_relative_path() {
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
            let events = load_codex_events_from_sources(&sources, single_thread).unwrap();

            assert_eq!(events.len(), 2);
            assert_eq!(events[0].session_id, "duplicate");
            assert_eq!(events[0].input_tokens, 111);
            assert_eq!(events[1].session_id, "archived-only");
            assert_eq!(events[1].input_tokens, 222);
        }
    }

    #[test]
    fn loads_session_usage_with_numeric_timestamp() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2026-01-02T00:00:00.000Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "gpt-5",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": 1767312001000_u64,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 10,
                                "output_tokens": 50,
                                "reasoning_output_tokens": 0,
                                "total_tokens": 150,
                            },
                            "model": "gpt-5",
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "session");
        assert_eq!(events[0].timestamp, "2026-01-02T00:00:01.000Z");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].total_tokens, 150);
    }

    #[test]
    fn loads_session_usage_with_spaced_type_fields() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{ "timestamp": "2026-01-02T00:00:00.000Z", "type" : "turn_context", "payload": { "model": "gpt-5" } }"#,
                r#"{ "timestamp": "2026-01-02T00:00:01.000Z", "type" : "event_msg", "payload": { "type" : "token_count", "info": { "total_token_usage": { "input_tokens": 100, "cached_input_tokens": 10, "output_tokens": 50, "total_tokens": 150 }, "model": "gpt-5" } } }"#,
                r#"{ "timestamp": "2026-01-02T00:00:02.000Z", "type" : "event_msg", "payload": { "type":"token_count", "info": { "total_token_usage": { "input_tokens": 200, "cached_input_tokens": 20, "output_tokens": 75, "total_tokens": 275 }, "model": "gpt-5" } } }"#,
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].timestamp, "2026-01-02T00:00:01.000Z");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].total_tokens, 150);
        assert_eq!(events[1].timestamp, "2026-01-02T00:00:02.000Z");
        assert_eq!(events[1].input_tokens, 100);
        assert_eq!(events[1].cached_input_tokens, 10);
        assert_eq!(events[1].output_tokens, 25);
        assert_eq!(events[1].total_tokens, 125);
    }

    #[test]
    fn subtracts_cache_write_usage_from_cumulative_token_totals() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2026-01-02T00:00:01.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-terra",
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 60,
                                "cache_write_input_tokens": 20,
                                "output_tokens": 10,
                                "total_tokens": 110,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-01-02T00:00:02.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-terra",
                            "total_token_usage": {
                                "input_tokens": 200,
                                "cached_input_tokens": 100,
                                "cache_write_input_tokens": 50,
                                "output_tokens": 25,
                                "total_tokens": 225,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].cache_creation_tokens, 20);
        assert_eq!(events[1].input_tokens, 100);
        assert_eq!(events[1].cached_input_tokens, 40);
        assert_eq!(events[1].cache_creation_tokens, 30);
        assert_eq!(events[1].output_tokens, 15);
        assert_eq!(events[1].total_tokens, 115);
    }

    #[test]
    fn loads_headless_usage_with_unexpected_noncritical_field_types() {
        let fixture = fs_fixture!({
            "run.jsonl":
            json!({
                "type": "turn.completed",
                "timestamp": false,
                "model": {
                    "name": "unexpected"
                },
                "usage": {
                    "input_tokens": 120,
                    "cached_input_tokens": 20,
                    "output_tokens": 30,
                    "total_tokens": 150,
                },
            })
            .to_string(),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "run");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5"));
        assert!(events[0].is_fallback_model);
        assert_eq!(events[0].input_tokens, 120);
        assert_eq!(events[0].cached_input_tokens, 20);
        assert_eq!(events[0].output_tokens, 30);
        assert_eq!(events[0].total_tokens, 150);
        assert_eq!(events[0].service_tier, None);
    }

    #[test]
    fn resolves_codex_auto_review_to_latest_model_for_event_date() {
        let fixture = fs_fixture!({
            "run.jsonl": [
                json!({
                    "type": "turn.completed",
                    "timestamp": "2026-02-05T00:00:00.000Z",
                    "model": "codex-auto-review",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "total_tokens": 15,
                    },
                })
                .to_string(),
                json!({
                    "type": "turn.completed",
                    "timestamp": "2026-03-05T00:00:00.000Z",
                    "model": "codex-auto-review",
                    "usage": {
                        "input_tokens": 20,
                        "output_tokens": 10,
                        "total_tokens": 30,
                    },
                })
                .to_string(),
                json!({
                    "type": "turn.completed",
                    "timestamp": "2026-04-23T00:00:00.000Z",
                    "model": "codex-auto-review",
                    "usage": {
                        "input_tokens": 30,
                        "output_tokens": 15,
                        "total_tokens": 45,
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(events[1].model.as_deref(), Some("gpt-5.4"));
        assert_eq!(events[2].model.as_deref(), Some("gpt-5.5"));
        assert!(events.iter().all(|event| event.is_fallback_model));
    }

    #[test]
    fn resolves_codex_auto_review_with_invalid_event_date_falls_back_to_file_mtime() {
        // When the log's own timestamp fields are unparsable AND no alternate
        // timestamp fields are present, the parser falls back to the file's
        // modified time so the date-based fallback table still resolves to a
        // real model rather than locking in a misleading malformed string.
        let fixture = fs_fixture!({
            "invalid-month.jsonl": json!({
                "type": "turn.completed",
                "timestamp": "2026-99-99T00:00:00.000Z",
                "model": "codex-auto-review",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15,
                },
            })
            .to_string(),
            "invalid-leap-day.jsonl": json!({
                "type": "turn.completed",
                "timestamp": "2026-02-29T00:00:00.000Z",
                "model": "codex-auto-review",
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 10,
                    "total_tokens": 30,
                },
            })
            .to_string(),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 2);
        // File mtime is "now" at fixture creation, which is on or after every
        // entry currently in the fallback table, so resolution lands on the
        // newest known model. The exact value updates when the table grows.
        assert!(events.iter().all(|event| {
            event
                .model
                .as_deref()
                .is_some_and(|model| model.starts_with("gpt-5"))
        }));
        assert!(events.iter().all(|event| event.is_fallback_model));
    }

    #[test]
    fn resolves_codex_auto_review_uses_created_at_when_top_level_timestamp_is_malformed() {
        // Regression test for the model-date resolution chain short-circuiting
        // on a malformed top-level `timestamp` and ignoring the `created_at`
        // fallback field that holds a parseable date.
        let fixture = fs_fixture!({
            "run.jsonl": json!({
                "type": "turn.completed",
                "timestamp": "not-a-date",
                "created_at": "2025-12-11T00:00:00.000Z",
                "model": "codex-auto-review",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15,
                },
            })
            .to_string(),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.2-codex"));
        assert!(events[0].is_fallback_model);
    }

    #[test]
    fn resolves_codex_auto_review_turn_context_for_each_event_date() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2025-12-11T00:00:00.000Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "codex-auto-review",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2025-12-11T00:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 10,
                                "output_tokens": 5,
                                "total_tokens": 15,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-04-23T00:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 20,
                                "output_tokens": 10,
                                "total_tokens": 30,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[1].model.as_deref(), Some("gpt-5.5"));
        assert!(events.iter().all(|event| event.is_fallback_model));
    }

    #[test]
    fn loads_headless_usage_with_token_count_text_content() {
        let fixture = fs_fixture!({
            "run.jsonl":
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T03:04:05.000Z",
                "model": "gpt-5.2-codex",
                "content": "debug token_count payload text",
                "usage": {
                    "input_tokens": 120,
                    "cached_input_tokens": 20,
                    "output_tokens": 30,
                    "total_tokens": 150,
                },
            })
            .to_string(),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "run");
        assert_eq!(events[0].timestamp, "2026-01-02T03:04:05.000Z");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[0].input_tokens, 120);
        assert_eq!(events[0].cached_input_tokens, 20);
        assert_eq!(events[0].output_tokens, 30);
        assert_eq!(events[0].total_tokens, 150);
    }

    #[test]
    fn uses_nested_model_name_for_standalone_exec_usage() {
        let fixture = fs_fixture!({
            "solo.jsonl":
            json!({
                "data": {
                    "timestamp": "2026-03-01T00:00:00.000Z",
                    "model_name": "gpt-5.2-codex",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "total_tokens": 15,
                    },
                },
            })
            .to_string(),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "solo");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[0].input_tokens, 10);
        assert_eq!(events[0].output_tokens, 5);
        assert_eq!(events[0].total_tokens, 15);
    }

    #[test]
    fn skips_replayed_parent_token_history_in_thread_spawn_subagent_files() {
        let fixture = fs_fixture!({
            "2026-05-12T08-00-00-parent.jsonl": [
                json!({
                    "timestamp": "2026-05-12T08:00:00.000Z",
                    "type": "turn_context",
                    "payload": {"model": "gpt-5.2", "model_name": null, "metadata": null},
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:02:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 500,
                                "cached_input_tokens": 50,
                                "output_tokens": 100,
                                "total_tokens": 600,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_500,
                                "cached_input_tokens": 150,
                                "output_tokens": 300,
                                "total_tokens": 1_800,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
            "2026-05-12T08-03-00-subagent.jsonl": [
                // session_meta: subagent with thread_spawn
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "subagent-abc",
                        "source": {
                            "subagent": {
                                "thread_spawn": {
                                    "parent_thread_id": "parent-xyz"
                                }
                            }
                        }
                    },
                })
                .to_string(),
                // session_meta: parent
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {"id": "parent-xyz"},
                })
                .to_string(),
                // replayed parent history — timestamps all at subagent creation time
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 500,
                                "cached_input_tokens": 50,
                                "output_tokens": 100,
                                "total_tokens": 600,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_500,
                                "cached_input_tokens": 150,
                                "output_tokens": 300,
                                "total_tokens": 1_800,
                            },
                        },
                    },
                })
                .to_string(),
                // subagent's own entries — different timestamps
                json!({
                    "timestamp": "2026-05-12T08:04:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 10,
                                "output_tokens": 20,
                                "total_tokens": 120,
                            },
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 10,
                                "output_tokens": 20,
                                "total_tokens": 120,
                            },
                            "model": "gpt-5.2",
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:05:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 50,
                                "cached_input_tokens": 5,
                                "output_tokens": 10,
                                "total_tokens": 60,
                            },
                            "total_token_usage": {
                                "input_tokens": 150,
                                "cached_input_tokens": 15,
                                "output_tokens": 30,
                                "total_tokens": 180,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();

            assert_eq!(
                events.len(),
                4,
                "expected 4 events (2 parent + 2 subagent real), got {} with single_thread={}",
                events.len(),
                single_thread
            );

            let parent_events: Vec<_> = events
                .iter()
                .filter(|e| e.session_id.contains("parent"))
                .collect();
            assert_eq!(parent_events.len(), 2);
            assert_eq!(parent_events[0].input_tokens, 1_000);
            assert_eq!(parent_events[1].input_tokens, 500);

            let subagent_events: Vec<_> = events
                .iter()
                .filter(|e| e.session_id.contains("subagent"))
                .collect();
            assert_eq!(subagent_events.len(), 2);
            assert_eq!(subagent_events[0].input_tokens, 100);
            assert_eq!(subagent_events[0].output_tokens, 20);
            assert_eq!(subagent_events[1].input_tokens, 50);
            assert_eq!(subagent_events[1].output_tokens, 10);
        }
    }

    #[test]
    fn skips_replayed_parent_token_history_in_forked_session_files() {
        let fixture = fs_fixture!({
            "2026-05-12T08-00-00-parent.jsonl": [
                json!({
                    "timestamp": "2026-05-12T08:00:00.000Z",
                    "type": "turn_context",
                    "payload": {"model": "gpt-5.2"},
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:02:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 500,
                                "cached_input_tokens": 50,
                                "output_tokens": 100,
                                "total_tokens": 600,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_500,
                                "cached_input_tokens": 150,
                                "output_tokens": 300,
                                "total_tokens": 1_800,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
            "2026-05-12T08-03-00-fork.jsonl": [
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "fork-abc",
                        "forked_from_id": "parent-xyz",
                        "originator": "codex_exec",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {"id": "parent-xyz", "originator": "codex-tui"},
                })
                .to_string(),
                // replayed parent history with timestamps rewritten to fork creation time
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 500,
                                "cached_input_tokens": 50,
                                "output_tokens": 100,
                                "total_tokens": 600,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_500,
                                "cached_input_tokens": 150,
                                "output_tokens": 300,
                                "total_tokens": 1_800,
                            },
                        },
                    },
                })
                .to_string(),
                // fork's own entry
                json!({
                    "timestamp": "2026-05-12T08:04:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 10,
                                "output_tokens": 20,
                                "total_tokens": 120,
                            },
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 10,
                                "output_tokens": 20,
                                "total_tokens": 120,
                            },
                            "model": "gpt-5.2",
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();

            assert_eq!(
                events.len(),
                3,
                "expected 3 events (2 parent + 1 fork real), got {} with single_thread={}",
                events.len(),
                single_thread
            );

            let fork_events: Vec<_> = events
                .iter()
                .filter(|event| event.session_id.contains("fork"))
                .collect();
            assert_eq!(fork_events.len(), 1);
            assert_eq!(fork_events[0].input_tokens, 100);
            assert_eq!(fork_events[0].cached_input_tokens, 10);
            assert_eq!(fork_events[0].output_tokens, 20);
            assert_eq!(fork_events[0].total_tokens, 120);
            assert_eq!(fork_events[0].source.as_deref(), Some("Exec"));
        }
    }

    #[test]
    fn keeps_cumulative_baseline_when_skipping_subagent_replay() {
        let fixture = fs_fixture!({
            "2026-05-12T08-03-00-subagent.jsonl": [
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "subagent-abc",
                        "source": {
                            "subagent": {
                                "thread_spawn": {
                                    "parent_thread_id": "parent-xyz"
                                }
                            }
                        }
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 1_500,
                                "cached_input_tokens": 150,
                                "output_tokens": 300,
                                "total_tokens": 1_800,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:04:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 1_600,
                                "cached_input_tokens": 160,
                                "output_tokens": 320,
                                "total_tokens": 1_920,
                            },
                            "model": "gpt-5.2",
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 20);
        assert_eq!(events[0].total_tokens, 120);
    }

    #[test]
    fn skips_replayed_history_across_multiple_subagent_files() {
        let parent_line = json!({
            "timestamp": "2026-05-12T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "total_tokens": 1_200,
                    },
                    "total_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "total_tokens": 1_200,
                    },
                    "model": "gpt-5.2",
                },
            },
        })
        .to_string();

        fn subagent_file(creation_ts: &str, real_ts: &str, input_tokens: u64) -> String {
            [
                json!({
                    "timestamp": creation_ts,
                    "type": "session_meta",
                    "payload": {
                        "id": "subagent",
                        "source": {
                            "subagent": {
                                "thread_spawn": {
                                    "parent_thread_id": "parent"
                                }
                            }
                        }
                    },
                })
                .to_string(),
                json!({
                    "timestamp": creation_ts,
                    "type": "session_meta",
                    "payload": {"id": "parent"},
                })
                .to_string(),
                // replayed entries — two token_count lines with the same timestamp
                json!({
                    "timestamp": creation_ts,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 700,
                                "cached_input_tokens": 70,
                                "output_tokens": 140,
                                "total_tokens": 840,
                            },
                            "total_token_usage": {
                                "input_tokens": 700,
                                "cached_input_tokens": 70,
                                "output_tokens": 140,
                                "total_tokens": 840,
                            },
                        },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": creation_ts,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 300,
                                "cached_input_tokens": 30,
                                "output_tokens": 60,
                                "total_tokens": 360,
                            },
                            "total_token_usage": {
                                "input_tokens": 1_000,
                                "cached_input_tokens": 100,
                                "output_tokens": 200,
                                "total_tokens": 1_200,
                            },
                        },
                    },
                })
                .to_string(),
                // subagent's own entry — different timestamp
                json!({
                    "timestamp": real_ts,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": input_tokens,
                                "cached_input_tokens": 0,
                                "output_tokens": 10,
                                "total_tokens": input_tokens + 10,
                            },
                            "total_token_usage": {
                                "input_tokens": input_tokens,
                                "cached_input_tokens": 0,
                                "output_tokens": 10,
                                "total_tokens": input_tokens + 10,
                            },
                            "model": "gpt-5.2",
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n")
        }

        let fixture = fs_fixture!({
            "2026-05-12T08-01-00-parent.jsonl": parent_line,
            "2026-05-12T08-02-00-subagent-a.jsonl": subagent_file(
                "2026-05-12T08:02:00.000Z",
                "2026-05-12T08:04:00.000Z",
                50,
            ),
            "2026-05-12T08-06-00-subagent-b.jsonl": subagent_file(
                "2026-05-12T08:06:00.000Z",
                "2026-05-12T08:08:00.000Z",
                75,
            ),
            "2026-05-12T08-10-00-subagent-c.jsonl": subagent_file(
                "2026-05-12T08:10:00.000Z",
                "2026-05-12T08:12:00.000Z",
                25,
            ),
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();

            assert_eq!(
                events.len(),
                4,
                "expected 4 events (1 parent + 3 subagent real), got {} with single_thread={}",
                events.len(),
                single_thread
            );

            let total_input: u64 = events.iter().map(|e| e.input_tokens).sum();
            assert_eq!(
                total_input, 1_150,
                "expected 1150 total input (1000 parent + 50 + 75 + 25 subagents)"
            );
        }
    }

    #[test]
    fn bounds_the_replay_at_a_numeric_fork_timestamp() {
        let fixture = fs_fixture!({
            "01-parent.jsonl": [
                replay_metadata("2026-07-10T08:00:00.000Z", "parent", None),
                replay_token_count("2026-07-10T08:01:00.000Z", 100),
                // Written after the child forked.
                replay_token_count("2026-07-10T08:03:00.000Z", 50),
            ]
            .join("\n"),
            "02-child.jsonl": [
                json!({
                    // Epoch seconds for 2026-07-10T08:02:00Z.
                    "timestamp": 1_783_670_520_u64,
                    "type": "session_meta",
                    "payload": {"id": "child", "forked_from_id": "parent"},
                })
                .to_string(),
                replay_token_count("2026-07-10T08:02:00.000Z", 100),
                // Real child usage that happens to equal the parent's next event.
                replay_token_count("2026-07-10T08:04:00.000Z", 50),
            ]
            .join("\n"),
        });

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [
                ("01-parent".to_string(), 100),
                ("01-parent".to_string(), 50),
                ("02-child".to_string(), 50),
            ]
        );
    }

    #[test]
    fn falls_back_to_rewritten_second_when_the_replay_starts_mid_parent_stream() {
        let fixture = fs_fixture!({
            "01-parent.jsonl": [
                replay_metadata("2026-07-10T08:00:00.000Z", "parent", None),
                replay_token_count("2026-07-10T08:01:00.000Z", 100),
                replay_token_count("2026-07-10T08:02:00.000Z", 200),
                replay_token_count("2026-07-10T08:03:00.000Z", 300),
            ]
            .join("\n"),
            // Codex replayed a compacted history, so it does not line up with the
            // start of the parent stream.
            "02-child.jsonl": [
                replay_metadata("2026-07-10T09:00:00.000Z", "child", Some("parent")),
                replay_token_count("2026-07-10T09:00:00.100Z", 200),
                replay_token_count("2026-07-10T09:00:00.200Z", 300),
                replay_token_count("2026-07-10T09:05:00.000Z", 400),
            ]
            .join("\n"),
        });

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [
                ("01-parent".to_string(), 100),
                ("01-parent".to_string(), 200),
                ("01-parent".to_string(), 300),
                ("02-child".to_string(), 400),
            ]
        );
    }

    #[test]
    fn skips_a_rewritten_burst_that_straddles_a_second_boundary() {
        let fixture = fs_fixture!({
            // Codex writes the replayed history in a few milliseconds, so the
            // burst lands on either side of a second tick whenever the fork
            // happens late in a second. Measured against real logs, such a burst
            // spans tens of milliseconds while the child's own first turn follows
            // seconds later, so the run is what identifies it, not the second it
            // was stamped with.
            "child.jsonl": [
                replay_metadata("2026-07-10T09:00:00.985Z", "child", Some("missing-parent")),
                replay_token_count("2026-07-10T09:00:00.986Z", 100),
                replay_token_count("2026-07-10T09:00:00.999Z", 200),
                replay_token_count("2026-07-10T09:00:01.000Z", 300),
                replay_token_count("2026-07-10T09:00:01.009Z", 400),
                // The child's own first turn, after a real pause.
                replay_token_count("2026-07-10T09:00:08.000Z", 500),
            ]
            .join("\n"),
        });

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [("child".to_string(), 500)]
        );
    }

    #[test]
    fn keeps_fork_local_usage_recorded_after_a_rewritten_burst() {
        let fixture = fs_fixture!({
            "child.jsonl": [
                replay_metadata("2026-07-10T09:00:00.000Z", "child", Some("missing-parent")),
                replay_token_count("2026-07-10T09:00:00.100Z", 100),
                replay_token_count("2026-07-10T09:00:00.200Z", 200),
                // Every later turn is the child's own, however many there are.
                replay_token_count("2026-07-10T09:00:30.000Z", 300),
                replay_token_count("2026-07-10T09:01:00.000Z", 400),
                replay_token_count("2026-07-10T09:01:30.000Z", 500),
            ]
            .join("\n"),
        });

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [
                ("child".to_string(), 300),
                ("child".to_string(), 400),
                ("child".to_string(), 500),
            ]
        );
    }

    #[test]
    fn keeps_child_usage_matching_parent_event_after_fork() {
        let fixture = fs_fixture!({
            "parent.jsonl": [
                replay_metadata("2026-05-12T08:00:00.000Z", "parent", None),
                replay_token_count("2026-05-12T08:01:00.000Z", 100),
                // Written after the child forked.
                replay_token_count("2026-05-12T08:03:00.000Z", 50),
            ]
            .join("\n"),
            "child.jsonl": [
                replay_metadata("2026-05-12T08:02:00.000Z", "child", Some("parent")),
                // Replayed parent state at the fork.
                replay_token_count("2026-05-12T08:02:00.000Z", 100),
                // Real child usage that happens to equal the parent's next event.
                replay_token_count("2026-05-12T08:04:00.000Z", 50),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();
            let child_events = events
                .iter()
                .filter(|event| event.session_id == "child")
                .collect::<Vec<_>>();

            assert_eq!(child_events.len(), 1, "single_thread={single_thread}");
            assert_eq!(child_events[0].input_tokens, 50);
        }
    }

    #[test]
    fn keeps_full_parent_stream_when_the_parent_itself_replayed_a_missing_session() {
        let fixture = fs_fixture!({
            // The grandparent log is gone, so this session falls back to skipping
            // its own rewritten second.
            "01-parent.jsonl": [
                replay_metadata("2026-07-10T08:00:00.000Z", "parent", Some("missing-grandparent")),
                replay_token_count("2026-07-10T08:00:00.100Z", 100),
                replay_token_count("2026-07-10T08:00:00.200Z", 200),
                replay_token_count("2026-07-10T08:01:00.000Z", 300),
                replay_token_count("2026-07-10T08:02:00.000Z", 400),
            ]
            .join("\n"),
            "02-child.jsonl": [
                replay_metadata("2026-07-10T09:00:00.000Z", "child", Some("parent")),
                replay_token_count("2026-07-10T09:00:00.100Z", 100),
                replay_token_count("2026-07-10T09:00:00.200Z", 200),
                replay_token_count("2026-07-10T09:00:00.300Z", 300),
                replay_token_count("2026-07-10T09:00:00.400Z", 400),
                replay_token_count("2026-07-10T09:05:00.000Z", 500),
            ]
            .join("\n"),
        });

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [
                ("01-parent".to_string(), 300),
                ("01-parent".to_string(), 400),
                ("02-child".to_string(), 500),
            ]
        );
    }

    #[test]
    fn keeps_usage_of_a_session_that_lists_itself_as_its_own_parent() {
        let fixture = fs_fixture!({
            "self.jsonl": [
                json!({
                    "type": "session_meta",
                    "payload": {"id": "self", "forked_from_id": "self"},
                })
                .to_string(),
                replay_token_count("2026-07-10T08:01:00.000Z", 100),
                replay_token_count("2026-07-10T08:02:00.000Z", 200),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();
            assert_eq!(
                events
                    .iter()
                    .map(|event| (event.session_id.clone(), event.input_tokens))
                    .collect::<Vec<_>>(),
                [("self".to_string(), 100), ("self".to_string(), 200)],
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn skips_missing_parent_replay_when_duplicate_snapshot_is_suppressed() {
        fn token_count(timestamp: &str, input: u64, total_input: u64) -> String {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
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
                        "model": "gpt-5.5",
                    },
                },
            })
            .to_string()
        }

        let fixture = fs_fixture!({
            "child.jsonl": [
                json!({
                    "type": "session_meta",
                    "payload": {"id": "child", "forked_from_id": "missing-parent"},
                })
                .to_string(),
                // The replay repeats a snapshot, which must not end the burst.
                token_count("2026-07-10T08:00:00.100Z", 100, 100),
                token_count("2026-07-10T08:00:00.200Z", 100, 100),
                // The child's own turn, after a real pause.
                token_count("2026-07-10T08:00:08.000Z", 50, 50),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 50);
    }

    #[test]
    fn skips_nested_replays_against_immutable_parent_streams() {
        fn metadata(id: &str, parent: Option<&str>) -> String {
            json!({
                "type": "session_meta",
                "payload": {"id": id, "forked_from_id": parent},
            })
            .to_string()
        }

        fn token_count(timestamp: &str, input: u64) -> String {
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
                    },
                },
            })
            .to_string()
        }

        let fixture = fs_fixture!({
            "01-root.jsonl": [
                metadata("root", None),
                token_count("2026-07-10T08:01:00.000Z", 100),
            ]
            .join("\n"),
            "02-parent.jsonl": [
                metadata("parent", Some("root")),
                token_count("2026-07-10T09:00:00.000Z", 100),
                token_count("2026-07-10T09:01:00.000Z", 50),
            ]
            .join("\n"),
            "03-child.jsonl": [
                metadata("child", Some("parent")),
                token_count("2026-07-10T10:00:00.000Z", 100),
                token_count("2026-07-10T10:00:01.000Z", 50),
                token_count("2026-07-10T10:01:00.000Z", 25),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(
            events
                .iter()
                .map(|event| (event.session_id.as_str(), event.input_tokens))
                .collect::<Vec<_>>(),
            [("01-root", 100), ("02-parent", 50), ("03-child", 25)]
        );
    }

    #[test]
    fn skips_repeated_last_usage_when_cumulative_total_is_unchanged() {
        let usage = json!({
            "last_token_usage": {
                "input_tokens": 100,
                "cached_input_tokens": 20,
                "output_tokens": 10,
                "total_tokens": 110,
            },
            "total_token_usage": {
                "input_tokens": 100,
                "cached_input_tokens": 20,
                "output_tokens": 10,
                "total_tokens": 110,
            },
            "model": "gpt-5.5",
        });
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2026-07-10T08:00:00.000Z",
                    "type": "event_msg",
                    "payload": {"type": "token_count", "info": usage.clone()},
                })
                .to_string(),
                json!({
                    "timestamp": "2026-07-10T08:00:01.000Z",
                    "type": "event_msg",
                    "payload": {"type": "token_count", "info": usage},
                })
                .to_string(),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 110);
    }

    #[test]
    fn bounded_loading_resolves_thread_id_rollout_parent_by_payload_id() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/2025-01-01T08-00-00-thread_id_rollout.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
            "sessions/2026/03/15/child.jsonl": [
                replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent-id")),
                replay_token_count("2026-03-15T08:00:00.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
        });
        let parent =
            fixture.path("sessions/2025/01/01/2025-01-01T08-00-00-thread_id_rollout.jsonl");
        let child = fixture.path("sessions/2026/03/15/child.jsonl");
        crate::paths::set_file_modified(
            &parent,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for single_thread in [true, false] {
            let (events, detected) = load_codex_events_from_sources_with_shared(
                &sources,
                &SharedArgs {
                    single_thread,
                    ..shared.clone()
                },
            )
            .unwrap();

            assert!(detected, "single_thread={single_thread}");
            assert_eq!(events.len(), 1, "single_thread={single_thread}");
            assert_eq!(
                events[0].session_id, "2026/03/15/child",
                "single_thread={single_thread}"
            );
            assert_eq!(events[0].input_tokens, 50, "single_thread={single_thread}");
        }
    }

    #[test]
    fn bounded_loading_does_not_select_a_misleading_filename() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/aaa-target-id.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "different-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 999),
            ]
            .join("\n"),
            "sessions/2025/01/02/zzz-actual-parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "target-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
            "sessions/2026/03/15/child.jsonl": [
                replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("target-id")),
                replay_token_count("2026-03-15T08:00:00.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
        });
        let misleading = fixture.path("sessions/2025/01/01/aaa-target-id.jsonl");
        let parent = fixture.path("sessions/2025/01/02/zzz-actual-parent.jsonl");
        let child = fixture.path("sessions/2026/03/15/child.jsonl");
        for path in [&misleading, &parent] {
            crate::paths::set_file_modified(
                path,
                crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
            );
        }
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for single_thread in [true, false] {
            let _ = crate::replay::take_observed_file_read_events();
            let (events, detected) = load_codex_events_from_sources_with_shared(
                &sources,
                &SharedArgs {
                    single_thread,
                    ..shared.clone()
                },
            )
            .unwrap();

            assert!(detected, "single_thread={single_thread}");
            assert_eq!(events.len(), 1, "single_thread={single_thread}");
            assert_eq!(events[0].input_tokens, 50, "single_thread={single_thread}");
            if single_thread {
                let reads = crate::replay::take_observed_file_read_events();
                assert!(reads.iter().any(|read| matches!(
                    read,
                    crate::replay::ObservedFileRead::ParentUsage(path) if path == &parent
                )));
                assert!(!reads.iter().any(|read| matches!(
                    read,
                    crate::replay::ObservedFileRead::ParentUsage(path) if path == &misleading
                )));
            }
        }
    }

    #[test]
    fn bounded_loading_uses_the_first_duplicate_parent_id() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/aaa-first-parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
            "sessions/2025/01/02/zzz-second-parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 200),
            ]
            .join("\n"),
            "sessions/2026/03/15/child.jsonl": [
                replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent-id")),
                replay_token_count("2026-03-15T08:00:00.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
        });
        let first_parent = fixture.path("sessions/2025/01/01/aaa-first-parent.jsonl");
        let second_parent = fixture.path("sessions/2025/01/02/zzz-second-parent.jsonl");
        let child = fixture.path("sessions/2026/03/15/child.jsonl");
        for path in [&first_parent, &second_parent] {
            crate::paths::set_file_modified(
                path,
                crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
            );
        }
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for single_thread in [true, false] {
            let (events, detected) = load_codex_events_from_sources_with_shared(
                &sources,
                &SharedArgs {
                    single_thread,
                    ..shared.clone()
                },
            )
            .unwrap();

            assert!(detected, "single_thread={single_thread}");
            assert_eq!(events.len(), 1, "single_thread={single_thread}");
            assert_eq!(events[0].session_id, "2026/03/15/child");
            assert_eq!(events[0].input_tokens, 50, "single_thread={single_thread}");
        }
    }

    #[test]
    fn bounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/aaa-child.jsonl": [
                replay_metadata(
                    "2026-03-15T08:00:00.000Z",
                    "shared-id",
                    Some("shared-id"),
                ),
                replay_token_count("2026-03-15T08:00:01.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
            "sessions/2025/01/02/zzz-parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "shared-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
        });
        let child = fixture.path("sessions/2025/01/01/aaa-child.jsonl");
        let parent = fixture.path("sessions/2025/01/02/zzz-parent.jsonl");
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        crate::paths::set_file_modified(
            &parent,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for single_thread in [true, false] {
            let _ = crate::replay::take_observed_file_read_events();
            let (events, detected) = load_codex_events_from_sources_with_shared(
                &sources,
                &SharedArgs {
                    single_thread,
                    ..shared.clone()
                },
            )
            .unwrap();

            assert!(detected, "single_thread={single_thread}");
            assert_eq!(events.len(), 1, "single_thread={single_thread}");
            assert_eq!(events[0].input_tokens, 50, "single_thread={single_thread}");
        }
    }

    #[test]
    fn unbounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/aaa-child.jsonl": [
                replay_metadata(
                    "2026-03-15T08:00:00.000Z",
                    "shared-id",
                    Some("shared-id"),
                ),
                replay_token_count("2026-03-15T08:00:01.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
            "sessions/2025/01/02/zzz-parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "shared-id", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
        });
        let child = fixture.path("sessions/2025/01/01/aaa-child.jsonl");
        let parent = fixture.path("sessions/2025/01/02/zzz-parent.jsonl");
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        crate::paths::set_file_modified(
            &parent,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );

        for single_thread in [true, false] {
            let events =
                load_codex_events_from_directory(&fixture.path("sessions"), single_thread).unwrap();
            let child_events = events
                .iter()
                .filter(|event| event.session_id.ends_with("2025/01/01/aaa-child"))
                .collect::<Vec<_>>();

            assert_eq!(child_events.len(), 1, "single_thread={single_thread}");
            assert_eq!(
                child_events[0].input_tokens, 50,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn bounded_loading_metadata_probes_history_but_reads_parent_usage_only() {
        let fixture = fs_fixture!({
            "sessions/2025/01/01/parent.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 100),
            ]
            .join("\n"),
            "sessions/2025/01/01/unrelated.jsonl": [
                replay_metadata("2025-01-01T08:00:00.000Z", "unrelated", None),
                replay_token_count("2025-01-01T08:01:00.000Z", 999),
            ]
            .join("\n"),
            "sessions/2026/03/15/child.jsonl": [
                replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent")),
                replay_token_count("2026-03-15T08:00:00.000Z", 100),
                replay_token_count("2026-03-15T08:01:00.000Z", 50),
            ]
            .join("\n"),
        });
        let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
        let unrelated = fixture.path("sessions/2025/01/01/unrelated.jsonl");
        let child = fixture.path("sessions/2026/03/15/child.jsonl");
        for path in [&parent, &unrelated] {
            crate::paths::set_file_modified(
                path,
                crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
            );
        }
        crate::paths::set_file_modified(
            &child,
            crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
        );
        let sources = [CodexUsageSource::new_for_test(
            fixture.path("sessions"),
            fixture.root().to_path_buf(),
        )];
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };

        let _ = crate::replay::take_observed_file_reads();
        let (events, detected) =
            load_codex_events_from_sources_with_shared(&sources, &shared).unwrap();

        assert!(detected);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "2026/03/15/child");
        assert_eq!(events[0].input_tokens, 50);
        let reads = crate::replay::take_observed_file_read_events();
        for path in [&child, &parent, &unrelated] {
            assert!(reads.iter().any(|read| matches!(
                read,
                crate::replay::ObservedFileRead::MetadataProbe(observed) if observed == path
            )));
        }
        let parent_usage_reads = reads
            .iter()
            .filter_map(|read| match read {
                crate::replay::ObservedFileRead::ParentUsage(path) => Some(path.clone()),
                crate::replay::ObservedFileRead::MetadataProbe(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(parent_usage_reads, vec![parent]);
    }

    fn replay_metadata(timestamp: &str, id: &str, parent: Option<&str>) -> String {
        json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {"id": id, "forked_from_id": parent},
        })
        .to_string()
    }

    fn replay_token_count(timestamp: &str, input_tokens: u64) -> String {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": 1,
                        "total_tokens": input_tokens + 1,
                    },
                },
            },
        })
        .to_string()
    }

    fn replay_input_tokens_by_session(dir: &Path) -> Vec<(String, u64)> {
        load_codex_events_from_directory(dir, true)
            .unwrap()
            .iter()
            .map(|event| (event.session_id.clone(), event.input_tokens))
            .collect()
    }
}
