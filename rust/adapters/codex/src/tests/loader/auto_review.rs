use super::*;

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
