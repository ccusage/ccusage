use ccusage_test_support::fs_fixture;
use serde_json::json;

use super::{
    otel::{is_otel_usage_envelope, timestamp_from_parts},
    session_state::is_session_state_usage_envelope,
    *,
};
use ccusage_core::Diagnostics;

#[test]
fn scalar_timestamps_follow_the_shared_epoch_unit_boundary() {
    let fixture = ccusage_test_support::Fixture::new();
    for (raw, expected) in [
        (1_767_312_000_u64, 1_767_312_000_000_i64),
        (10_000_000_000, 10_000_000_000),
        (1_767_312_000_000, 1_767_312_000_000),
        (1_767_312_000_000_000, 1_767_312_000_000),
        (1_767_312_000_000_000_000, 1_767_312_000_000),
    ] {
        let path = fixture.write_file(
            "otel.jsonl",
            json!({
                "body": "GenAI inference:",
                "timestamp": raw,
                "attributes": {"gen_ai.usage.input_tokens": 10, "gen_ai.usage.output_tokens": 2},
            })
            .to_string(),
        );
        let diagnostics = Diagnostics::new("copilot");
        let entries = parse_otel_file(&path, &diagnostics).unwrap();
        assert_eq!(entries.len(), 1, "{raw}");
        assert_eq!(entries[0].timestamp.as_millis(), expected, "{raw}");
    }
}

#[test]
fn fixed_unit_timestamps_reject_unsupported_instants() {
    assert_eq!(
        timestamp_from_parts(Some(&json!([10_000_000_000_000_u64, 0]))),
        None
    );
}

#[test]
fn diagnoses_otel_usage_without_a_timestamp() {
    let fixture = ccusage_test_support::fs_fixture!({
        "otel.jsonl": r#"{"body":"GenAI inference:","attributes":{"gen_ai.usage.input_tokens":10,"gen_ai.usage.output_tokens":2,"gen_ai.request.model":"gpt-5"}}"#,
    });
    let diagnostics = Diagnostics::new("copilot");
    let events = parse_otel_file(&fixture.path("otel.jsonl"), &diagnostics).unwrap();
    assert!(events.is_empty());
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
        1
    );
}

#[test]
fn diagnoses_session_state_usage_without_a_timestamp() {
    let fixture = ccusage_test_support::fs_fixture!({
        "session/events.jsonl": r#"{"type":"session.shutdown","data":{"modelMetrics":{"gpt-5":{"usage":{"inputTokens":10,"outputTokens":2}}}}}"#,
    });
    let diagnostics = Diagnostics::new("copilot");
    let events =
        parse_session_state_file(&fixture.path("session/events.jsonl"), &diagnostics).unwrap();
    assert!(events.entries.is_empty());
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
        1
    );
}

#[test]
fn undated_watermark_checkpoint_still_marks_later_shutdowns_as_covered() {
    let fixture = ccusage_test_support::fs_fixture!({
        "session/events.jsonl": concat!(
            r#"{"type":"session.usage_checkpoint","data":{"totalNanoAiu":5,"usageAccountingWatermarks":{"session":1}}}"#,
            "\n",
            r#"{"type":"session.shutdown","timestamp":"2026-02-05T09:00:00.000Z","data":{"modelMetrics":{"gpt-5":{"usage":{"inputTokens":10,"outputTokens":2}}}}}"#,
            "\n",
        ),
    });
    let diagnostics = Diagnostics::new("copilot");

    let parsed =
        parse_session_state_file(&fixture.path("session/events.jsonl"), &diagnostics).unwrap();

    assert!(parsed.entries.is_empty());
    assert!(parsed.credit_snapshots.is_empty());
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
        0
    );
}

#[test]
fn confirms_usage_diagnostics_from_token_or_credit_fields() {
    assert!(is_otel_usage_envelope(&json!({
        "attributes": {"gen_ai.usage.input_tokens": 0}
    })));
    assert!(!is_otel_usage_envelope(&json!({
        "attributes": {"gen_ai.request.model": "gpt-5"}
    })));
    assert!(!is_otel_usage_envelope(
        &json!({"attributes": "SECRET_CANARY"})
    ));

    assert!(is_session_state_usage_envelope(&json!({
        "type": "session.shutdown",
        "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 0}}}}
    })));
    assert!(is_session_state_usage_envelope(&json!({
        "type": "session.usage_checkpoint",
        "data": {"totalNanoAiu": 0}
    })));
    assert!(!is_session_state_usage_envelope(&json!({
        "type": "tool",
        "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 1}}}}
    })));
    assert!(!is_session_state_usage_envelope(&json!({
        "type": "session.shutdown",
        "data": {"message": "SECRET_CANARY"}
    })));
    assert!(is_session_state_usage_envelope(&json!({
        "type": "session.usage_record",
        "data": {"usage": {"model": "gpt-5", "inputTokens": 0}}
    })));
    assert!(is_session_state_usage_envelope(&json!({
        "type": "session.usage_record",
        "data": {"usage": {"model": "gpt-5"}}
    })));
    assert!(!is_session_state_usage_envelope(&json!({
        "type": "session.usage_record",
        "data": {"usage": "SECRET_CANARY"}
    })));
    assert!(!is_session_state_usage_envelope(&json!({
        "type": "session.usage_record",
        "data": {"modelMetrics": {"gpt-5": {"usage": {"inputTokens": 1}}}}
    })));
}

#[test]
fn reports_a_usage_record_that_does_not_decode_and_keeps_the_next_one() {
    let record = |id: &str| {
        json!({
            "type": "session.usage_record",
            "id": id,
            "timestamp": "2026-02-05T09:00:00.000Z",
            "data": {"usage": {
                "model": "gpt-5",
                "inputTokens": 100,
                "outputTokens": 10,
                "accounting": {"sourceSessionId": "session-1", "sequence": 1},
            }},
        })
        .to_string()
    };
    let repeated = record("record-1").replacen(
        r#""id":"record-1""#,
        r#""id":"record-1","id":"record-1""#,
        1,
    );
    let fixture = fs_fixture!({
        "session-1/events.jsonl": format!("{repeated}
{}
", record("record-2")),
    });
    let diagnostics = Diagnostics::new("copilot");

    let parsed =
        parse_session_state_file(&fixture.path("session-1/events.jsonl"), &diagnostics).unwrap();

    assert_eq!(parsed.usage_records.len(), 1);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::UndecodedUsage),
        0
    );
}

#[test]
fn reports_a_tokenless_usage_record_that_does_not_decode() {
    let record = |id: &str| {
        json!({
            "type": "session.usage_record",
            "id": id,
            "timestamp": "2026-02-05T09:00:00.000Z",
            "data": {"usage": {"model": "gpt-5"}},
        })
        .to_string()
    };
    let repeated = record("record-1").replacen(
        r#""id":"record-1""#,
        r#""id":"record-1","id":"record-1""#,
        1,
    );
    let fixture = fs_fixture!({
        "session-1/events.jsonl": format!("{repeated}
{}
", record("record-2")),
    });
    let diagnostics = Diagnostics::new("copilot");

    let parsed =
        parse_session_state_file(&fixture.path("session-1/events.jsonl"), &diagnostics).unwrap();

    assert_eq!(parsed.usage_records.len(), 1);
    assert_eq!(parsed.usage_records[0].entry.request_count, 1);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::UndecodedUsage),
        0
    );
}

#[test]
fn parses_session_shutdown_model_metrics() {
    let fixture = fs_fixture!({
        "session-1/events.jsonl": json!({
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
                        }
                    }
                }
            }
        })
        .to_string(),
    });

    let entries = parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .expect("session-state file should parse")
    .entries;

    assert_eq!(entries.len(), 1);
    assert_eq!(
        crate::format_rfc3339_millis(entries[0].timestamp),
        "2026-04-15T09:52:27.352Z"
    );
    assert_eq!(entries[0].session_id, "session-1");
    assert_eq!(entries[0].model, "test-model");
    assert_eq!(entries[0].input_tokens, 70);
    assert_eq!(entries[0].output_tokens, 50);
    assert_eq!(entries[0].cache_creation_tokens, 20);
    assert_eq!(entries[0].cache_read_tokens, 10);
    assert_eq!(entries[0].reasoning_output_tokens, 5);
    assert_eq!(
        entries[0].dedup_key,
        "shutdown:session-1:shutdown-1:test-model"
    );
}

#[test]
fn normalizes_copilot_internal_model_ids() {
    assert_eq!(
        normalize_copilot_model(" claude-opus-4.7-1m-internal "),
        "claude-opus-4.7"
    );
    assert_eq!(
        normalize_copilot_model("claude-opus-4.6-1m"),
        "claude-opus-4.6"
    );
    assert_eq!(normalize_copilot_model("gpt-5.4"), "gpt-5.4");
}

#[test]
fn skips_malformed_lines_and_non_shutdown_events() {
    let fixture = fs_fixture!({
        "session-1/events.jsonl": format!(
            "not json\n{}\n{}",
            json!({
                "type": "tool",
                "data": {"message": "session.shutdown"}
            }),
            json!({
                "type": "session.shutdown",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "modelMetrics": {
                        "test-model": {
                            "usage": {
                                "inputTokens": 1,
                                "outputTokens": 2
                            }
                        }
                    }
                }
            })
        ),
    });

    let entries = parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .unwrap()
    .entries;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].input_tokens, 1);
    assert_eq!(entries[0].output_tokens, 2);
    assert!(
        entries[0]
            .dedup_key
            .starts_with("shutdown:session-1:2026-04-15T09:52:27.352Z:test-model:")
    );
}

#[test]
fn parses_each_model_and_ignores_request_cost_and_empty_usage() {
    let fixture = fs_fixture!({
        "session-1/events.jsonl": json!({
            "type": "session.shutdown",
            "id": "shutdown-1",
            "timestamp": "2026-04-15T09:52:27.352Z",
            "data": {
                "modelMetrics": {
                    "first-model": {
                        "usage": {
                            "inputTokens": 10,
                            "outputTokens": 20
                        },
                        "requests": {"count": 1, "cost": 999}
                    },
                    "empty-model": {
                        "usage": {
                            "inputTokens": 0,
                            "outputTokens": 0
                        }
                    },
                    "second-model": {
                        "usage": {
                            "cacheReadTokens": 3,
                            "cacheWriteTokens": 4
                        }
                    }
                }
            }
        })
        .to_string(),
    });

    let entries = parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .unwrap()
    .entries;

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.model.as_str())
            .collect::<Vec<_>>(),
        ["first-model", "second-model"]
    );
    assert_eq!(entries[1].cache_creation_tokens, 4);
    assert_eq!(entries[1].cache_read_tokens, 3);
    assert_eq!(entries[0].request_count, 1);
}

#[test]
fn parses_credit_totals_from_checkpoints_and_shutdowns() {
    let fixture = fs_fixture!({
        "session-1/events.jsonl": [
            json!({
                "type": "session.usage_checkpoint",
                "id": "checkpoint-1",
                "timestamp": "2026-04-15T09:50:00.000Z",
                "data": {"totalNanoAiu": 51_322_830_000_u64, "promptCacheBreakState": []}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "totalNanoAiu": 93_524_970_000.0,
                    "modelMetrics": {
                        "test-model": {
                            "usage": {"inputTokens": 100, "outputTokens": 50},
                            "totalNanoAiu": 42_202_140_000.0
                        }
                    }
                }
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-2",
                "timestamp": "2026-04-16T09:00:00.000Z",
                "data": {"totalNanoAiu": 93_524_970_000_u64, "modelMetrics": {}}
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let parsed = parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .unwrap();

    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].nano_aiu, Some(42_202_140_000));
    assert_eq!(
        parsed
            .credit_snapshots
            .iter()
            .map(|snapshot| (
                snapshot.dedup_key.as_str(),
                snapshot.is_shutdown,
                snapshot.total_nano_aiu
            ))
            .collect::<Vec<_>>(),
        [
            (
                "credits:session-1:checkpoint-1",
                false,
                Some(51_322_830_000)
            ),
            ("credits:session-1:shutdown-1", true, Some(93_524_970_000)),
            ("credits:session-1:shutdown-2", true, Some(93_524_970_000)),
        ]
    );
}

#[test]
fn marks_shutdown_credits_unknown_when_a_total_is_missing() {
    let fixture = fs_fixture!({
        "session-1/events.jsonl": [
            json!({
                "type": "session.shutdown",
                "id": "without-model-total",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "totalNanoAiu": 93_524_970_000_u64,
                    "modelMetrics": {
                        "test-model": {"usage": {"inputTokens": 100, "outputTokens": 50}}
                    }
                }
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "without-session-total",
                "timestamp": "2026-04-16T09:52:27.352Z",
                "data": {"modelMetrics": {}}
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let parsed = parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .unwrap();

    assert_eq!(
        parsed
            .credit_snapshots
            .iter()
            .map(|snapshot| snapshot.total_nano_aiu)
            .collect::<Vec<_>>(),
        [None, None]
    );
}
