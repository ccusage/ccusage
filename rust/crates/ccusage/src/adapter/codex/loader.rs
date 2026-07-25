use std::{
    path::{Path, PathBuf},
    thread,
};

use compact_str::CompactString;

use crate::{
    CodexTokenUsageEvent, Result, chunk_file_indexes_by_size, cli::SharedArgs, fast::FxHashSet,
    progress,
};

use super::{
    parser::visit_codex_session_file,
    paths::{
        CodexUsageSource, codex_usage_sources, collect_codex_usage_files,
        collect_deduped_codex_usage_files,
    },
    replay::CodexReplayPlan,
};

pub(crate) fn load_codex_events_from_directory(
    sessions_dir: &Path,
    single_thread: bool,
) -> Result<Vec<CodexTokenUsageEvent>> {
    let files = collect_codex_usage_files(sessions_dir);
    let replay_plan = CodexReplayPlan::new([(sessions_dir, files.as_slice())], single_thread);
    let mut events = if single_thread {
        files
            .iter()
            .flat_map(|file| read_codex_session_file(sessions_dir, file, &replay_plan))
            .collect::<Vec<_>>()
    } else {
        read_codex_session_files_parallel(sessions_dir, &files, &replay_plan)
    };
    dedupe_codex_events(&mut events);
    Ok(events)
}

pub(crate) fn load_codex_events(shared: &SharedArgs) -> Result<Vec<CodexTokenUsageEvent>> {
    progress::track_usage_load(progress::UsageLoadAgent::Codex, shared.json, || {
        load_codex_events_inner(shared)
    })
}

fn load_codex_events_inner(shared: &SharedArgs) -> Result<Vec<CodexTokenUsageEvent>> {
    load_codex_events_from_sources(&codex_usage_sources()?, shared.single_thread)
}

fn load_codex_events_from_sources(
    sources: &[CodexUsageSource],
    single_thread: bool,
) -> Result<Vec<CodexTokenUsageEvent>> {
    if let [source] = sources {
        return load_codex_events_from_directory(&source.dir, single_thread);
    }

    let groups = collect_deduped_codex_usage_files(sources);
    let replay_plan = CodexReplayPlan::new(
        groups
            .iter()
            .map(|group| (group.dir.as_path(), group.files.as_slice())),
        single_thread,
    );
    let mut events = Vec::new();
    for group in groups {
        let mut source_events = if single_thread {
            group
                .files
                .iter()
                .flat_map(|file| read_codex_session_file(&group.dir, file, &replay_plan))
                .collect::<Vec<_>>()
        } else {
            read_codex_session_files_parallel(&group.dir, &group.files, &replay_plan)
        };
        events.append(&mut source_events);
    }
    dedupe_codex_events(&mut events);
    Ok(events)
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
    let mut seen = FxHashSet::default();
    events.retain(|event| {
        seen.insert((
            CompactString::new(&event.timestamp),
            event.model.as_deref().map(CompactString::new),
            event.input_tokens,
            event.cached_input_tokens,
            event.output_tokens,
            event.reasoning_output_tokens,
            event.total_tokens,
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use crate::adapter::codex::paths::CodexUsageSource;

    fn codex_event(session_id: &str) -> CodexTokenUsageEvent {
        CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            timestamp: "2026-01-02T00:00:00.000Z".to_string(),
            model: Some("gpt-5".to_string()),
            input_tokens: 100,
            cached_input_tokens: 10,
            output_tokens: 50,
            reasoning_output_tokens: 0,
            total_tokens: 150,
            is_fallback_model: false,
        }
    }

    #[test]
    fn dedupes_matching_codex_usage_events_from_distinct_sessions() {
        let mut events = vec![codex_event("session-a"), codex_event("session-b")];

        dedupe_codex_events(&mut events);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "session-a");
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
                token_count("2026-07-10T08:00:00.100Z", 100, 100),
                token_count("2026-07-10T08:00:00.200Z", 100, 100),
                token_count("2026-07-10T08:00:01.000Z", 50, 50),
            ]
            .join("\n"),
        });

        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 50);
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

        assert_eq!(
            replay_input_tokens_by_session(fixture.root()),
            [("self".to_string(), 100), ("self".to_string(), 200)]
        );
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
        assert_eq!(events[1].timestamp, "2026-01-02T03:05:05.000Z");
        assert_eq!(events[1].model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(events[1].input_tokens, 50);
        assert_eq!(events[1].cached_input_tokens, 5);
        assert_eq!(events[1].output_tokens, 12);
        assert_eq!(events[1].total_tokens, 62);
        assert_eq!(events[2].timestamp, "2026-01-02T03:06:05.000Z");
        assert_eq!(events[2].input_tokens, 9);
        assert_eq!(events[2].output_tokens, 4);
        assert_eq!(events[2].reasoning_output_tokens, 1);
        assert_eq!(events[2].total_tokens, 14);
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
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-12T08:03:00.000Z",
                    "type": "session_meta",
                    "payload": {"id": "parent-xyz"},
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
    fn keeps_child_usage_matching_parent_event_after_fork() {
        fn metadata(timestamp: &str, id: &str, parent: Option<&str>) -> String {
            json!({
                "timestamp": timestamp,
                "type": "session_meta",
                "payload": {"id": id, "forked_from_id": parent},
            })
            .to_string()
        }

        fn token_count(timestamp: &str, input_tokens: u64) -> String {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "output_tokens": 1,
                            "total_tokens": input_tokens + 1,
                        },
                        "model": "gpt-5.2",
                    },
                },
            })
            .to_string()
        }

        let fixture = fs_fixture!({
            "parent.jsonl": [
                metadata("2026-05-12T08:00:00.000Z", "parent", None),
                token_count("2026-05-12T08:01:00.000Z", 100),
                // Written after the child forked.
                token_count("2026-05-12T08:03:00.000Z", 50),
            ]
            .join("\n"),
            "child.jsonl": [
                metadata(
                    "2026-05-12T08:02:00.000Z",
                    "child",
                    Some("parent"),
                ),
                // Replayed parent state at the fork.
                token_count("2026-05-12T08:02:00.000Z", 100),
                // Real child usage that happens to equal the parent's next event.
                token_count("2026-05-12T08:04:00.000Z", 50),
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
}
