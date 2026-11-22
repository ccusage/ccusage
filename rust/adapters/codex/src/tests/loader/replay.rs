use super::*;

#[test]
fn skips_replayed_parent_token_history_in_thread_spawn_subagent_files() {
    let fixture = fs_fixture!({
        "2026-05-12T08-00-00-parent.jsonl": [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
            })
            .to_string(),
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
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
            })
            .to_string(),
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

    check_replay_modes(fixture.root(), |single_thread, events| {
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
    });
}

#[test]
fn skips_replayed_parent_token_history_in_forked_session_files() {
    let fixture = fs_fixture!({
        "2026-05-12T08-00-00-parent.jsonl": [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
            })
            .to_string(),
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

    check_replay_modes(fixture.root(), |single_thread, events| {
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
    });
}

#[test]
fn keeps_cumulative_baseline_when_skipping_subagent_replay() {
    let fixture = fs_fixture!({
        "2026-05-12T08-00-00-parent.jsonl": [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        let events = events
            .iter()
            .filter(|event| event.session_id.contains("subagent"))
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 1, "single_thread={single_thread}");
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 20);
        assert_eq!(events[0].total_tokens, 120);
    });
}

#[test]
fn skips_replayed_history_across_multiple_subagent_files() {
    let parent_usage =
        |timestamp: &str, last: (u64, u64, u64, u64), total: (u64, u64, u64, u64)| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": last.0,
                            "cached_input_tokens": last.1,
                            "output_tokens": last.2,
                            "total_tokens": last.3,
                        },
                        "total_token_usage": {
                            "input_tokens": total.0,
                            "cached_input_tokens": total.1,
                            "output_tokens": total.2,
                            "total_tokens": total.3,
                        },
                        "model": "gpt-5.2",
                    },
                },
            })
            .to_string()
        };
    let parent = [
        json!({
            "timestamp": "2026-05-12T08:00:00.000Z",
            "type": "session_meta",
            "payload": {"id": "parent"},
        })
        .to_string(),
        parent_usage(
            "2026-05-12T08:00:30.000Z",
            (700, 70, 140, 840),
            (700, 70, 140, 840),
        ),
        parent_usage(
            "2026-05-12T08:01:00.000Z",
            (300, 30, 60, 360),
            (1_000, 100, 200, 1_200),
        ),
    ]
    .join("\n");

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
        "2026-05-12T08-01-00-parent.jsonl": parent,
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            events.len(),
            5,
            "expected 5 events (2 parent + 3 subagent real), got {} with single_thread={}",
            events.len(),
            single_thread
        );

        let total_input: u64 = events.iter().map(|event| event.input_tokens).sum();
        assert_eq!(
            total_input, 1_150,
            "expected 1150 total input (1000 parent + 50 + 75 + 25 subagents)"
        );

        let mut input_by_session = replay_input_tokens_by_session(events);
        input_by_session.sort_unstable();
        assert_eq!(
            input_by_session,
            [
                ("2026-05-12T08-01-00-parent".to_string(), 300),
                ("2026-05-12T08-01-00-parent".to_string(), 700),
                ("2026-05-12T08-02-00-subagent-a".to_string(), 50),
                ("2026-05-12T08-06-00-subagent-b".to_string(), 75),
                ("2026-05-12T08-10-00-subagent-c".to_string(), 25),
            ],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn bounds_the_replay_at_a_numeric_fork_timestamp() {
    let fixture = fs_fixture!({
        "01-parent.jsonl": [
            replay_metadata("2026-07-10T08:00:00.000Z", "parent", None),
            replay_token_count("2026-07-10T08:01:00.000Z", 100),
            replay_token_count("2026-07-10T08:03:00.000Z", 50),
        ]
        .join("\n"),
        "02-child.jsonl": [
            json!({
                "timestamp": 1_783_670_520_u64,
                "type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "parent"},
            })
            .to_string(),
            replay_token_count("2026-07-10T08:02:00.000Z", 100),
            replay_token_count("2026-07-10T08:04:00.000Z", 50),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            replay_input_tokens_by_session(events),
            [
                ("01-parent".to_string(), 100),
                ("01-parent".to_string(), 50),
                ("02-child".to_string(), 50),
            ],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn skips_a_replayed_parent_suffix_that_starts_mid_stream() {
    let fixture = fs_fixture!({
        "01-parent.jsonl": [
            replay_metadata("2026-07-10T08:00:00.000Z", "parent", None),
            replay_token_count("2026-07-10T08:01:00.000Z", 100),
            replay_token_count("2026-07-10T08:02:00.000Z", 200),
            replay_token_count("2026-07-10T08:03:00.000Z", 300),
        ]
        .join("\n"),
        "02-child.jsonl": [
            replay_metadata("2026-07-10T09:00:00.000Z", "child", Some("parent")),
            replay_token_count("2026-07-10T09:00:00.100Z", 200),
            replay_token_count("2026-07-10T09:00:00.200Z", 300),
            replay_token_count("2026-07-10T09:05:00.000Z", 400),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            replay_input_tokens_by_session(events),
            [
                ("01-parent".to_string(), 100),
                ("01-parent".to_string(), 200),
                ("01-parent".to_string(), 300),
                ("02-child".to_string(), 400),
            ],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn counts_first_response_when_missing_parent_history_begins_after_fork() {
    let fixture = fs_fixture!({
        "child.jsonl": r#"{"timestamp":"2026-01-05T15:00:00.000Z","type":"session_meta","payload":{"id":"child","source":{"subagent":{"thread_spawn":{"parent_thread_id":"missing-parent"}}}}}
{"timestamp":"2026-01-05T15:00:01.000Z","type":"turn_context","payload":{"model":"gpt-5","model_name":null,"metadata":null}}
{"timestamp":"2026-01-05T15:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010}}}}
{"timestamp":"2026-01-05T15:00:02.150Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010}}}}"#,
    });
    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(events.len(), 1, "single_thread={single_thread}");
        assert_eq!(events[0].model.as_str(), "gpt-5");
        assert_eq!(events[0].input_tokens, 1_000);
        assert_eq!(events[0].cached_input_tokens, 400);
        assert_eq!(events[0].output_tokens, 10);
        assert_eq!(events[0].reasoning_output_tokens, 0);
        assert_eq!(events[0].total_tokens, 1_010);
    });
}
