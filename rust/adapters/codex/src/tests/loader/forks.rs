use super::*;

#[test]
fn counts_spawned_responses_once_across_duplicate_snapshot_controls() {
    fn token_count(
        timestamp: &str,
        last: (u64, u64, u64, u64),
        total: (u64, u64, u64, u64),
    ) -> String {
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
                },
            },
        })
        .to_string()
    }

    let first_response = (1_000, 400, 10, 1_010);
    let second_response = (2_000, 800, 20, 2_020);
    let scenarios = [
        (
            "single missing-parent response",
            vec![(
                "child.jsonl",
                [
                    replay_metadata("2026-01-05T15:00:00.000Z", "child", Some("missing-parent")),
                    token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                ]
                .join("\n"),
            )],
            vec![("child".to_string(), 1_000, 1_010)],
        ),
        (
            "CLI duplicate snapshot",
            vec![(
                "cli.jsonl",
                [
                    json!({
                        "timestamp": "2026-01-05T15:00:00.000Z",
                        "type": "session_meta",
                        "payload": {"id": "cli", "source": "cli"},
                    })
                    .to_string(),
                    token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                    token_count("2026-01-05T15:00:02.150Z", first_response, first_response),
                ]
                .join("\n"),
            )],
            vec![("cli".to_string(), 1_000, 1_010)],
        ),
        (
            "missing-parent duplicate after two seconds",
            vec![(
                "child.jsonl",
                [
                    replay_metadata("2026-01-05T15:00:00.000Z", "child", Some("missing-parent")),
                    token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                    token_count("2026-01-05T15:00:04.000Z", first_response, first_response),
                ]
                .join("\n"),
            )],
            vec![("child".to_string(), 1_000, 1_010)],
        ),
        (
            "present parent with unrelated history",
            vec![
                (
                    "parent.jsonl",
                    [
                        replay_metadata("2026-01-05T14:00:00.000Z", "parent", None),
                        replay_token_count("2026-01-05T14:01:00.000Z", 50),
                    ]
                    .join("\n"),
                ),
                (
                    "child.jsonl",
                    [
                        replay_metadata("2026-01-05T15:00:00.000Z", "child", Some("parent")),
                        token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                        token_count("2026-01-05T15:00:02.150Z", first_response, first_response),
                    ]
                    .join("\n"),
                ),
            ],
            vec![
                ("child".to_string(), 1_000, 1_010),
                ("parent".to_string(), 50, 51),
            ],
        ),
        (
            "new response after two seconds totals 3030",
            vec![(
                "child.jsonl",
                [
                    replay_metadata("2026-01-05T15:00:00.000Z", "child", Some("missing-parent")),
                    token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                    token_count("2026-01-05T15:00:02.150Z", first_response, first_response),
                    token_count(
                        "2026-01-05T15:00:04.150Z",
                        second_response,
                        (3_000, 1_200, 30, 3_030),
                    ),
                ]
                .join("\n"),
            )],
            vec![
                ("child".to_string(), 1_000, 1_010),
                ("child".to_string(), 2_000, 2_020),
            ],
        ),
        (
            "repeated second response stays counted once",
            vec![(
                "child.jsonl",
                [
                    replay_metadata("2026-01-05T15:00:00.000Z", "child", Some("missing-parent")),
                    token_count("2026-01-05T15:00:02.000Z", first_response, first_response),
                    token_count("2026-01-05T15:00:02.150Z", first_response, first_response),
                    token_count(
                        "2026-01-05T15:00:04.150Z",
                        second_response,
                        (3_000, 1_200, 30, 3_030),
                    ),
                    token_count(
                        "2026-01-05T15:00:04.300Z",
                        second_response,
                        (3_000, 1_200, 30, 3_030),
                    ),
                ]
                .join("\n"),
            )],
            vec![
                ("child".to_string(), 1_000, 1_010),
                ("child".to_string(), 2_000, 2_020),
            ],
        ),
    ];

    for (name, files, expected) in scenarios {
        let fixture = fs_fixture!({});
        for (path, contents) in files {
            fixture.write_file(path, contents);
        }
        let mut expected = expected;
        expected.sort_unstable();

        check_replay_modes(fixture.root(), |single_thread, events| {
            let mut events = events
                .iter()
                .map(|event| {
                    (
                        event.session_id.clone(),
                        event.input_tokens,
                        event.total_tokens,
                    )
                })
                .collect::<Vec<_>>();
            events.sort_unstable();

            assert_eq!(
                events, expected,
                "scenario: {name}, single_thread={single_thread}"
            );
        });
    }
}

#[test]
fn counts_spawned_child_requests_stamped_with_the_fork_instant() {
    let at_fork = "2026-11-01T09:42:09.923Z";
    let usage = |input: u64, cached: u64, output: u64| {
        json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
            "reasoning_output_tokens": 0,
            "total_tokens": input + output,
        })
    };
    let token_count = |last: Value, total: Value| {
        json!({
            "timestamp": at_fork,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {"last_token_usage": last, "total_token_usage": total},
            },
        })
        .to_string()
    };
    let usage_record = |response_id: &str, usage: Value| {
        json!({
            "timestamp": at_fork,
            "type": "token_usage_record",
            "payload": {
                "thread_id": "child",
                "response_id": response_id,
                "usage": usage,
            },
        })
        .to_string()
    };
    let fixture = fs_fixture!({
        "child.jsonl": [
            json!({
                "timestamp": at_fork,
                "type": "session_meta",
                "payload": {
                    "id": "child",
                    "forked_from_id": "missing-parent",
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "missing-parent"}}},
                },
            })
            .to_string(),
            json!({"timestamp": at_fork, "type": "compacted", "payload": {"message": "", "replacement_history": []}}).to_string(),
            json!({"timestamp": at_fork, "type": "turn_context", "payload": {"model": "gpt-5.2"}}).to_string(),
            usage_record("resp_child_1", usage(10_000, 2_000, 100)),
            token_count(
                usage(10_000, 2_000, 100),
                usage(1_000_000, 2_000, 10_100),
            ),
            usage_record("resp_child_2", usage(20_000, 3_000, 200)),
            token_count(
                usage(20_000, 3_000, 200),
                usage(1_020_000, 5_000, 10_300),
            ),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            events
                .iter()
                .map(|event| (
                    event.input_tokens,
                    event.cached_input_tokens,
                    event.output_tokens
                ))
                .collect::<Vec<_>>(),
            [(10_000, 2_000, 100), (20_000, 3_000, 200)],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn keeps_own_usage_a_spawned_child_wrote_before_its_first_usage_record() {
    let fixture = fs_fixture!({
        "child.jsonl": [
            json!({
                "timestamp": "2026-09-02T18:09:32.557Z",
                "type": "session_meta",
                "payload": {
                    "id": "child",
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "missing-parent"}}},
                },
            })
            .to_string(),
            replay_token_count("2026-09-02T18:09:57.815Z", 100),
            replay_token_count("2026-09-02T18:10:21.182Z", 200),
            json!({
                "timestamp": "2026-09-04T09:02:00.103Z",
                "type": "token_usage_record",
                "payload": {"response_id": "resp_child", "usage": {"input_tokens": 300, "output_tokens": 1}},
            })
            .to_string(),
            replay_token_count("2026-09-04T09:02:00.903Z", 300),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            replay_input_tokens_by_session(events),
            [
                ("child".to_string(), 100),
                ("child".to_string(), 200),
                ("child".to_string(), 300),
            ],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn skips_usage_between_a_copied_record_and_the_forks_own_settings() {
    let usage_record = |timestamp: &str, thread_id: &str, response_id: &str, input: u64| {
        json!({
            "timestamp": timestamp,
            "type": "token_usage_record",
            "payload": {
                "thread_id": thread_id,
                "response_id": response_id,
                "usage": {"input_tokens": input, "output_tokens": 1, "total_tokens": input + 1},
            },
        })
        .to_string()
    };
    let fixture = fs_fixture!({
        "child.jsonl": [
            replay_metadata("2026-11-01T09:00:00.000Z", "child", Some("missing-parent")),
            replay_token_count("2026-11-01T09:00:00.100Z", 100),
            usage_record("2026-11-01T09:00:00.150Z", "missing-parent", "resp_parent", 200),
            replay_token_count("2026-11-01T09:00:00.200Z", 200),
            json!({
                "timestamp": "2026-11-01T09:00:00.200Z",
                "type": "event_msg",
                "payload": {"type": "thread_settings_applied", "thread_id": "child", "thread_settings": {}},
            })
            .to_string(),
            usage_record("2026-11-01T09:00:08.000Z", "child", "resp_child", 500),
            replay_token_count("2026-11-01T09:00:08.000Z", 500),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            replay_input_tokens_by_session(events),
            [("child".to_string(), 100), ("child".to_string(), 500)],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn keeps_child_usage_matching_parent_event_after_fork() {
    let fixture = fs_fixture!({
        "parent.jsonl": [
            replay_metadata("2026-05-12T08:00:00.000Z", "parent", None),
            replay_token_count("2026-05-12T08:01:00.000Z", 100),
            replay_token_count("2026-05-12T08:03:00.000Z", 50),
        ]
        .join("\n"),
        "child.jsonl": [
            replay_metadata("2026-05-12T08:02:00.000Z", "child", Some("parent")),
            replay_token_count("2026-05-12T08:02:00.000Z", 100),
            replay_token_count("2026-05-12T08:04:00.000Z", 50),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        let child_events = events
            .iter()
            .filter(|event| event.session_id == "child")
            .collect::<Vec<_>>();

        assert_eq!(child_events.len(), 1, "single_thread={single_thread}");
        assert_eq!(child_events[0].input_tokens, 50);
    });
}

#[test]
fn keeps_full_parent_stream_when_the_parent_itself_replayed_a_missing_session() {
    let fixture = fs_fixture!({
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            replay_input_tokens_by_session(events),
            [
                ("01-parent".to_string(), 100),
                ("01-parent".to_string(), 200),
                ("01-parent".to_string(), 300),
                ("01-parent".to_string(), 400),
                ("02-child".to_string(), 500),
            ],
            "single_thread={single_thread}"
        );
    });
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            events
                .iter()
                .map(|event| (event.session_id.clone(), event.input_tokens))
                .collect::<Vec<_>>(),
            [("self".to_string(), 100), ("self".to_string(), 200)],
            "single_thread={single_thread}"
        );
    });
}

#[test]
fn keeps_first_child_response_after_zero_usage_initialization() {
    let token_count = |timestamp: &str, input_tokens: u64, output_tokens: u64| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                    },
                    "total_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                    },
                },
            },
        })
        .to_string()
    };

    let fixture = fs_fixture!({
        "child.jsonl": [
            replay_metadata(
                "2026-07-10T08:00:00.000Z",
                "child",
                Some("missing-parent"),
            ),
            token_count("2026-07-10T08:00:00.100Z", 0, 0),
            token_count("2026-07-10T08:00:00.200Z", 0, 0),
            token_count("2026-07-10T08:00:00.500Z", 50, 1),
        ]
        .join("\n"),
    });

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(events.len(), 1, "single_thread={single_thread}");
        assert_eq!(events[0].input_tokens, 50);
        assert_eq!(events[0].output_tokens, 1);
    });
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(
            events
                .iter()
                .map(|event| (event.session_id.as_str(), event.input_tokens))
                .collect::<Vec<_>>(),
            [("01-root", 100), ("02-parent", 50), ("03-child", 25)],
            "single_thread={single_thread}"
        );
    });
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

    check_replay_modes(fixture.root(), |single_thread, events| {
        assert_eq!(events.len(), 1, "single_thread={single_thread}");
        assert_eq!(events[0].total_tokens, 110);
    });
}

#[test]
fn releases_a_forks_single_usage_observation_at_end_of_file() {
    let fixture = fs_fixture!({
        "sessions/child.jsonl": [
            replay_metadata("2026-01-02T00:00:00Z", "child", Some("missing")),
            replay_token_count("2026-01-02T00:00:00Z", 100),
        ].join("\n"),
    });
    check_replay_modes(&fixture.path("sessions"), |single_thread, events| {
        assert_eq!(events.len(), 1, "single_thread={single_thread}");
        assert_eq!(events[0].input_tokens, 100);
    });
}
