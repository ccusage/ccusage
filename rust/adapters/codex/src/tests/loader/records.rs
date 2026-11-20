use super::*;

#[test]
fn dedupes_copied_branch_history_across_session_files() {
    ccusage_test_support::install_pricing_snapshots();
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
        let shared = SharedArgs {
            single_thread,
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Monthly).unwrap();
        let usage = groups
            .iter()
            .map(|(month, group)| {
                (
                    month.as_str(),
                    [
                        group.input_tokens,
                        group.cached_input_tokens,
                        group.output_tokens,
                        group.reasoning_output_tokens,
                        group.total_tokens,
                    ],
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            usage,
            [("2026-05", [1_600, 300, 450, 40, 2_050])],
            "single_thread={single_thread}"
        );
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].session_id, "run");
    assert_eq!(events[0].last_activity, "2026-01-02T03:04:05.000Z");
    assert_eq!(events[0].model.as_str(), "gpt-5.2-codex");
    assert_eq!(events[0].input_tokens, 120);
    assert_eq!(events[0].cached_input_tokens, 20);
    assert_eq!(events[0].output_tokens, 30);
    assert_eq!(events[0].total_tokens, 150);
    assert_eq!(events[1].last_activity, "2026-01-02T03:05:05.000Z");
    assert_eq!(events[1].model.as_str(), "gpt-5.2-codex");
    assert_eq!(events[1].input_tokens, 50);
    assert_eq!(events[1].cached_input_tokens, 5);
    assert_eq!(events[1].output_tokens, 12);
    assert_eq!(events[1].total_tokens, 62);
    assert_eq!(events[2].last_activity, "2026-01-02T03:06:05.000Z");
    assert_eq!(events[2].input_tokens, 9);
    assert_eq!(events[2].output_tokens, 4);
    assert_eq!(events[2].reasoning_output_tokens, 1);
    assert_eq!(events[2].total_tokens, 13);
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "session");
    assert_eq!(events[0].last_activity, "2026-01-02T00:00:01.000Z");
    assert_eq!(events[0].model.as_str(), "gpt-5");
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].last_activity, "2026-01-02T00:00:01.000Z");
    assert_eq!(events[0].model.as_str(), "gpt-5");
    assert_eq!(events[0].input_tokens, 100);
    assert_eq!(events[0].cached_input_tokens, 10);
    assert_eq!(events[0].output_tokens, 50);
    assert_eq!(events[0].total_tokens, 150);
    assert_eq!(events[1].last_activity, "2026-01-02T00:00:02.000Z");
    assert_eq!(events[1].input_tokens, 100);
    assert_eq!(events[1].cached_input_tokens, 10);
    assert_eq!(events[1].output_tokens, 25);
    assert_eq!(events[1].total_tokens, 125);
}

#[test]
fn skips_truncated_type_fields_without_losing_valid_usage() {
    let usage_line = r#"{"timestamp":"2026-01-02T00:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"total_tokens":150}}}}"#;
    for truncated in [
        r#"{"type" :"#,
        "{\"type\" : \t\r",
        r#"{"type" : ""#,
        r#"{"type":"event_msg","payload":{"type":"#,
        r#"{"type":"event_msg","payload":{"type":""#,
    ] {
        let fixture = fs_fixture!({
            "session.jsonl": [truncated, usage_line, truncated].join("\n"),
        });

        let events = crate::test_support::rollout_events(fixture.root());

        assert_eq!(events.len(), 1, "{truncated:?}");
        assert_eq!(events[0].model.as_str(), "gpt-5");
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].total_tokens, 150);
    }
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

    let events = crate::test_support::rollout_events(fixture.root());

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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "run");
    assert_eq!(events[0].model.as_str(), "gpt-5");
    assert!(events[0].is_fallback_model);
    assert_eq!(events[0].input_tokens, 120);
    assert_eq!(events[0].cached_input_tokens, 20);
    assert_eq!(events[0].output_tokens, 30);
    assert_eq!(events[0].total_tokens, 150);
    assert_eq!(events[0].service_tier, None);
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "run");
    assert_eq!(events[0].last_activity, "2026-01-02T03:04:05.000Z");
    assert_eq!(events[0].model.as_str(), "gpt-5.2-codex");
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "solo");
    assert_eq!(events[0].model.as_str(), "gpt-5.2-codex");
    assert_eq!(events[0].input_tokens, 10);
    assert_eq!(events[0].output_tokens, 5);
    assert_eq!(events[0].total_tokens, 15);
}

#[test]
fn loads_codex_token_count_events() {
    let fixture = fs_fixture!({
        "sessions/codex-session.jsonl": [
            r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150},"model":"gpt-5"}}}"#,
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(&fixture.path("sessions"));

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "codex-session");
    assert_eq!(events[0].model.as_str(), "gpt-5");
    assert_eq!(events[0].input_tokens, 100);
    assert_eq!(events[0].cached_input_tokens, 10);
    assert_eq!(events[0].output_tokens, 50);
    assert_eq!(events[0].reasoning_output_tokens, 0);
    assert_eq!(events[0].total_tokens, 150);
}

#[test]
fn loads_codex_token_count_groups_in_parallel() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-01-02T00:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150},"model":"gpt-5"}}}"#,
        ]
        .join("\n"),
        "sessions/session-b.jsonl": [
            r#"{"timestamp":"2026-01-02T00:01:00.000Z","type":"turn_context","payload":{"model":"gpt-5-mini"}}"#,
            r#"{"timestamp":"2026-01-02T00:01:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":40,"cached_input_tokens":4,"output_tokens":20,"reasoning_output_tokens":2,"total_tokens":62},"model":"gpt-5-mini"}}}"#,
        ]
        .join("\n"),
    });

    let [single_thread_groups, parallel_groups] = [true, false].map(|single_thread| {
        let shared = SharedArgs {
            single_thread,
            ..SharedArgs::default()
        };
        load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Session)
            .unwrap()
    });

    for groups in [single_thread_groups, parallel_groups] {
        assert_eq!(
            groups
                .iter()
                .map(|(session, group)| (session.as_str(), group.total_tokens))
                .collect::<Vec<_>>(),
            [("session-a", 150), ("session-b", 62)]
        );
    }
}
