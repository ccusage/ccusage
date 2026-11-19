use std::path::Path;

use ccusage_core::Diagnostics;
use ccusage_test_support::fs_fixture;
use serde_json::{Value, json};

use crate::{
    cli::{AgentReportKind, SharedArgs},
    paths::CodexUsageSource,
    replay::CodexReplayPlan,
    test_support::load_groups_from_directory,
};

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

    let events = crate::test_support::rollout_events(fixture.root());

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

#[test]
fn records_flex_service_tier_for_following_usage() {
    let fixture = fs_fixture!({
        "session.jsonl": [
            json!({
                "timestamp": "2026-07-22T00:00:04.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": { "service_tier": "flex" },
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-07-22T00:00:05.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-6-astra",
                        "last_token_usage": {
                            "input_tokens": 10,
                            "output_tokens": 1,
                            "total_tokens": 11,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].service_tier, Some(crate::CodexServiceTier::Flex));
    assert_eq!(events[0].input_tokens, 10);
}

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
            settings("2026-07-22T00:00:02.000Z", json!({"model": "codex-auto-review"})),
            token_count("2026-07-22T00:00:03.000Z", 20),
            settings("2026-07-22T00:00:04.000Z", json!({"service_tier": "standard"})),
            token_count("2026-07-22T00:00:05.000Z", 30),
            settings("2026-07-22T00:00:06.000Z", json!({"service_tier": "unknown"})),
            token_count("2026-07-22T00:00:07.000Z", 40),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

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
                    "thread_settings": { "service_tier": "unknown" },
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].service_tier, None);
    assert_eq!(events[1].service_tier, None);
}

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
fn resolves_codex_auto_review_at_documented_model_boundaries() {
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
            json!({
                "type": "turn.completed",
                "timestamp": "2026-07-29T23:59:59.999Z",
                "model": "codex-auto-review",
                "usage": {
                    "input_tokens": 40,
                    "output_tokens": 20,
                    "total_tokens": 60,
                },
            })
            .to_string(),
            json!({
                "type": "turn.completed",
                "timestamp": "2026-07-30T00:00:00.000Z",
                "model": "codex-auto-review",
                "usage": {
                    "input_tokens": 50,
                    "output_tokens": 25,
                    "total_tokens": 75,
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 5);
    assert_eq!(events[0].model.as_str(), "gpt-5.3-codex");
    assert_eq!(events[1].model.as_str(), "gpt-5.4");
    assert_eq!(events[2].model.as_str(), "gpt-5.4");
    assert_eq!(events[3].model.as_str(), "gpt-5.4");
    assert_eq!(events[4].model.as_str(), "gpt-5.6-luna");
    assert!(events.iter().all(|event| event.is_fallback_model));
}

#[test]
fn resolves_codex_auto_review_with_invalid_event_date_falls_back_to_file_mtime() {
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.model.starts_with("gpt-5")));
    assert!(events.iter().all(|event| event.is_fallback_model));
}

#[test]
fn resolves_codex_auto_review_uses_created_at_when_top_level_timestamp_is_malformed() {
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model.as_str(), "gpt-5.2-codex");
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
            json!({
                "timestamp": "2026-07-30T00:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 30,
                            "output_tokens": 15,
                            "total_tokens": 45,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].model.as_str(), "gpt-5.2-codex");
    assert_eq!(events[1].model.as_str(), "gpt-5.4");
    assert_eq!(events[2].model.as_str(), "gpt-5.6-luna");
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

    for single_thread in [true, false] {
        let events = crate::test_support::rollout_events(fixture.root());

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

    for single_thread in [true, false] {
        let events = crate::test_support::rollout_events(fixture.root());

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

    let events = crate::test_support::rollout_events(fixture.root());

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
        let events = crate::test_support::rollout_events(fixture.root());

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
fn counts_first_response_when_missing_parent_history_begins_after_fork() {
    let fixture = fs_fixture!({
        "child.jsonl": r#"{"timestamp":"2026-01-05T15:00:00.000Z","type":"session_meta","payload":{"id":"child","source":{"subagent":{"thread_spawn":{"parent_thread_id":"missing-parent"}}}}}
{"timestamp":"2026-01-05T15:00:01.000Z","type":"turn_context","payload":{"model":"gpt-5","model_name":null,"metadata":null}}
{"timestamp":"2026-01-05T15:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010}}}}
{"timestamp":"2026-01-05T15:00:02.150Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":1010}}}}"#,
    });
    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model.as_str(), "gpt-5");
    assert_eq!(events[0].input_tokens, 1_000);
    assert_eq!(events[0].cached_input_tokens, 400);
    assert_eq!(events[0].output_tokens, 10);
    assert_eq!(events[0].reasoning_output_tokens, 0);
    assert_eq!(events[0].total_tokens, 1_010);
}

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
        let mut events = crate::test_support::rollout_events(fixture.root())
            .into_iter()
            .map(|event| (event.session_id, event.input_tokens, event.total_tokens))
            .collect::<Vec<_>>();
        events.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();

        assert_eq!(events, expected, "scenario: {name}");
    }
}

#[test]
fn skips_a_rewritten_burst_that_straddles_a_second_boundary() {
    let fixture = fs_fixture!({
        "child.jsonl": [
            replay_metadata("2026-07-10T09:00:00.985Z", "child", Some("missing-parent")),
            replay_token_count("2026-07-10T09:00:00.986Z", 100),
            replay_token_count("2026-07-10T09:00:00.999Z", 200),
            replay_token_count("2026-07-10T09:00:01.000Z", 300),
            replay_token_count("2026-07-10T09:00:01.009Z", 400),
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
            token_count(usage(990_000, 0, 10_000), usage(990_000, 0, 10_000)),
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(
        events
            .iter()
            .map(|event| (
                event.input_tokens,
                event.cached_input_tokens,
                event.output_tokens
            ))
            .collect::<Vec<_>>(),
        [(10_000, 2_000, 100), (20_000, 3_000, 200)]
    );
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

    assert_eq!(
        replay_input_tokens_by_session(fixture.root()),
        [
            ("child".to_string(), 100),
            ("child".to_string(), 200),
            ("child".to_string(), 300),
        ]
    );
}

#[test]
fn skips_usage_records_copied_into_a_rewritten_user_fork_burst() {
    let fixture = fs_fixture!({
        "child.jsonl": [
            replay_metadata("2026-11-01T09:00:00.000Z", "child", Some("missing-parent")),
            replay_token_count("2026-11-01T09:00:00.100Z", 100),
            json!({
                "timestamp": "2026-11-01T09:00:00.150Z",
                "type": "token_usage_record",
                "payload": {"response_id": "resp_parent", "usage": {"input_tokens": 200, "output_tokens": 1}},
            })
            .to_string(),
            replay_token_count("2026-11-01T09:00:00.200Z", 200),
            replay_token_count("2026-11-01T09:00:08.000Z", 500),
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

    for single_thread in [true, false] {
        let events = crate::test_support::rollout_events(fixture.root());
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
        let events = crate::test_support::rollout_events(fixture.root());
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
            token_count("2026-07-10T08:00:00.100Z", 100, 100),
            token_count("2026-07-10T08:00:00.200Z", 100, 100),
            token_count("2026-07-10T08:00:08.000Z", 50, 50),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, 50);
}

#[test]
fn skips_missing_parent_duplicate_only_replay_at_known_session_start() {
    let token_count = |timestamp: &str, input_tokens: u64| {
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
                    "total_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": 1,
                        "total_tokens": input_tokens + 1,
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
            token_count("2026-07-10T08:00:00.100Z", 100),
            token_count("2026-07-10T08:00:00.200Z", 100),
            token_count("2026-07-10T08:00:08.000Z", 50),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, 50);
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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, 50);
    assert_eq!(events[0].output_tokens, 1);
}

#[test]
fn skips_missing_parent_replay_with_duplicate_snapshots_and_advancing_usage() {
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
            token_count("2026-07-10T08:00:00.300Z", 200, 300),
            token_count("2026-07-10T08:00:08.000Z", 50, 50),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

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

    let events = crate::test_support::rollout_events(fixture.root());

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

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 110);
}

#[test]
fn bounded_loading_resolves_thread_id_rollout_parent_by_payload_id() {
    ccusage_test_support::install_pricing_snapshots();
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
    let parent = fixture.path("sessions/2025/01/01/2025-01-01T08-00-00-thread_id_rollout.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_does_not_select_a_misleading_filename() {
    ccusage_test_support::install_pricing_snapshots();
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
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_uses_the_first_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
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
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
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
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2025/01/01/aaa-child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn unbounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
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
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            ..SharedArgs::default()
        };
        let groups = load_groups_from_directory(
            &fixture.path("sessions"),
            &shared,
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(
            groups["2025/01/01/aaa-child"].input_tokens, 50,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_metadata_probes_history_but_reads_parent_usage_only() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
        "sessions/2025/01/01/unrelated.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "unrelated", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 999),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
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
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        single_thread: true,
        ..SharedArgs::default()
    };

    let diagnostics = Diagnostics::new("codex");
    let (sessions, detected) = session_input_tokens(&sources, &shared, &diagnostics);

    assert!(detected);
    assert_eq!(sessions, [("2026/03/15/child".to_string(), 50)]);
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
        0,
        "the unrelated history was read past its first line"
    );
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

#[test]
fn parent_replay_records_skips_once_however_often_the_parent_is_read() {
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
            replay_token_count("2025-01-01T08:02:00.000Z", 120),
        ].join("\n") + "\n",
        "sessions/2025/01/02/child.jsonl": [
            replay_metadata("2025-01-02T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2025-01-02T08:00:00.000Z", 120),
        ].join("\n") + "\n",
    });
    let sessions = fixture.path("sessions");
    let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
    let child = fixture.path("sessions/2025/01/02/child.jsonl");
    let files = vec![parent.clone(), child];
    let diagnostics = Diagnostics::new("codex");

    let plan_files = [(sessions.as_path(), files.as_slice())];
    let plan = CodexReplayPlan::for_selection(&plan_files, plan_files, true, &diagnostics);

    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    for file in &files {
        crate::parser::visit_codex_session_file_with_diagnostics(
            &sessions,
            file,
            plan.fork_replay(file),
            |_| {},
            &diagnostics,
        );
    }
    let _ = CodexReplayPlan::for_selection(&plan_files, plan_files, true, &diagnostics);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
}

#[test]
fn the_fused_fork_pass_records_a_parent_skip_once() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
            replay_token_count("2025-01-01T08:02:00.000Z", 120),
        ].join("\n") + "\n",
        "sessions/2025/01/02/child.jsonl": [
            replay_metadata("2025-01-02T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2025-01-02T08:00:00.000Z", 120),
            replay_token_count("2025-01-02T08:01:00.000Z", 150),
        ].join("\n") + "\n",
    });
    let totals = [true, false].map(|single_thread| {
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("codex");
        let (groups, _) = crate::aggregate::load_groups_from_sources_with_diagnostics(
            &[CodexUsageSource::new(
                fixture.path("sessions"),
                fixture.path("sessions"),
            )],
            &shared,
            ccusage_core::cli::AgentReportKind::Daily,
            Some(&crate::PricingMap::load_embedded()),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        groups.values().map(|group| group.input_tokens).sum::<u64>()
    });
    assert_eq!(totals[0], totals[1]);
}

#[test]
fn bounded_parent_usage_failures_warn_once_without_changing_replay() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"SECRET_CANARY","#.to_string(),
        ].join("\n") + "\n",
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ].join("\n"),
    });
    let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];

    for (since, parent_tokens) in [("20260315", 0), ("20250101", 100)] {
        let shared = SharedArgs {
            since: Some(since.parse().unwrap()),
            until: Some("20260315".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread: true,
            debug: true,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("codex");
        let (sessions, _) = session_input_tokens(&sources, &shared, &diagnostics);

        let expected = [
            ("2025/01/01/parent", parent_tokens),
            ("2026/03/15/child", 50),
        ]
        .into_iter()
        .filter(|(_, tokens)| *tokens > 0)
        .map(|(session, tokens)| (session.to_string(), tokens))
        .collect::<Vec<_>>();
        assert_eq!(sessions, expected);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("SECRET_CANARY")
        );

        let diagnostics = Diagnostics::new("codex");
        let (groups, _) = crate::aggregate::load_groups_from_sources_with_diagnostics(
            &[CodexUsageSource::new(
                fixture.path("sessions"),
                fixture.path("sessions"),
            )],
            &shared,
            ccusage_core::cli::AgentReportKind::Daily,
            Some(&crate::PricingMap::load_embedded()),
            &diagnostics,
        )
        .unwrap();
        assert_eq!(
            groups.values().map(|group| group.input_tokens).sum::<u64>(),
            50 + parent_tokens
        );
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }
}

fn session_input_tokens(
    sources: &[CodexUsageSource],
    shared: &SharedArgs,
    diagnostics: &Diagnostics,
) -> (Vec<(String, u64)>, bool) {
    let (groups, detected) = crate::aggregate::load_groups_from_sources_with_diagnostics(
        sources,
        shared,
        AgentReportKind::Session,
        Some(&crate::PricingMap::load_embedded()),
        diagnostics,
    )
    .unwrap();
    let sessions = groups
        .into_iter()
        .map(|(session, group)| (session, group.input_tokens))
        .collect();
    (sessions, detected)
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
    crate::test_support::rollout_events(dir)
        .iter()
        .map(|event| (event.session_id.clone(), event.input_tokens))
        .collect()
}

#[test]
fn auto_review_is_dated_by_the_utc_day_of_the_event_instant() {
    let fixture = fs_fixture!({
        "headless.jsonl": json!({
            "type": "turn.completed",
            "timestamp": "2026-07-30T00:30:00+01:00",
            "model": "codex-auto-review",
            "usage": { "input_tokens": 10, "output_tokens": 2 },
        }).to_string(),
    });
    let events = crate::test_support::rollout_events(fixture.root());
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].timestamp,
        crate::parse_ts_timestamp("2026-07-29T23:30:00Z").unwrap()
    );
    assert_eq!(events[0].last_activity, "2026-07-29T23:30:00.000Z");
    assert_eq!(events[0].model, "gpt-5.4");
    assert!(events[0].is_fallback_model);
}

#[test]
fn auto_review_is_dated_by_the_field_that_dates_the_event() {
    let fixture = fs_fixture!({
        "headless.jsonl": json!({
            "type": "turn.completed",
            "timestamp": "2026-07-30Tnot-a-time",
            "created_at": "2026-03-05T00:00:00Z",
            "model": "codex-auto-review",
            "usage": { "input_tokens": 10, "output_tokens": 2 },
        }).to_string(),
    });
    let events = crate::test_support::rollout_events(fixture.root());
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].timestamp,
        crate::parse_ts_timestamp("2026-03-05T00:00:00Z").unwrap()
    );
    assert_eq!(events[0].last_activity, "2026-03-05T00:00:00.000Z");
    assert_eq!(events[0].model, "gpt-5.4");
    assert!(events[0].is_fallback_model);
}

#[test]
fn releases_a_forks_single_usage_observation_at_end_of_file() {
    let fixture = fs_fixture!({
        "sessions/child.jsonl": [
            replay_metadata("2026-01-02T00:00:00Z", "child", Some("missing")),
            replay_token_count("2026-01-02T00:00:00Z", 100),
        ].join("\n"),
    });
    let events = crate::test_support::rollout_events(&fixture.path("sessions"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, 100);
}
