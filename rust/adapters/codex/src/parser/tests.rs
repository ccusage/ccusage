use super::*;
use super::{model::codex_model_from_result, usage::normalize_headless_codex_usage};
use ccusage_test_support::fs_fixture;
use serde_json::json;
use std::assert_matches;

#[test]
fn usage_with_only_a_total_is_empty_in_every_record_kind() {
    let total_only = json!({"total_tokens": 50});
    for record in [
        json!({"timestamp": "2026-01-02T00:00:00Z", "type": "event_msg", "payload": {
            "type": "token_count", "info": {"model": "gpt-5", "last_token_usage": total_only},
        }}),
        json!({"timestamp": "2026-01-02T00:00:00Z", "type": "turn.completed", "usage": total_only}),
    ] {
        let fixture = fs_fixture!({ "session.jsonl": record.to_string() });
        assert!(
            crate::test_support::rollout_events(fixture.root()).is_empty(),
            "{record}"
        );
    }
    let fixture = fs_fixture!({
        "session.jsonl": [
            json!({"type": "turn_context", "payload": {"model": "gpt-5"}}),
            json!({"timestamp": "2026-01-02T00:00:00Z", "type": "token_usage_record", "payload": {
                "response_id": "response-1", "usage": total_only,
            }}),
            json!({"type": "compacted", "payload": {"compaction_response_id": "response-1"}}),
        ]
        .map(|line| line.to_string())
        .join("\n"),
    });
    assert!(crate::test_support::rollout_events(fixture.root()).is_empty());
}

#[test]
fn maps_recorded_service_tier_spellings() {
    for (value, expected) in [
        ("standard", Some(CodexServiceTier::Standard)),
        ("default", Some(CodexServiceTier::Standard)),
        ("priority", Some(CodexServiceTier::Fast)),
        ("fast", Some(CodexServiceTier::Fast)),
        ("flex", Some(CodexServiceTier::Flex)),
        ("ultrafast", Some(CodexServiceTier::Ultrafast)),
        ("", None),
    ] {
        assert_eq!(
            CodexServiceTier::from_codex_name(value),
            expected,
            "unexpected tier for {value:?}"
        );
    }
}

#[test]
fn recognizes_thread_settings_applied_as_session_line() {
    for line in [
        br#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#.as_slice(),
        br#"{ "type": "event_msg", "payload": { "type": "thread_settings_applied", "thread_settings": { "service_tier": "default" } } }"#.as_slice(),
    ] {
        assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
    }
}

#[test]
fn loads_large_spaced_token_counts_with_compact_payload_types() {
    let line = serde_json::json!({
        "timestamp": "2026-09-01T00:00:01Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": { "last_token_usage": { "input_tokens": 100, "output_tokens": 20 } },
            "padding": "x".repeat(70 * 1024),
        },
    })
    .to_string()
    .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#);
    let fixture = fs_fixture!({
        "session.jsonl": [
            r#"{"type":"turn_context","payload":{"model":"gpt-5-mini"}}"#,
            line.as_str(),
        ].join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 120);
}

#[test]
fn loads_headless_usage_with_nested_session_type_names() {
    for (entry_type, padding) in [
        (Some("result"), 70 * 1024),
        (None, 70 * 1024),
        (Some("result"), 0),
    ] {
        let mut record = json!({
            "timestamp": "2026-09-01T00:00:01Z",
            "model": "gpt-5-mini",
            "usage": { "input_tokens": 100, "output_tokens": 20 },
            "data": {
                "type": "event_msg",
                "nested": { "type": "token_count" },
                "padding": "x".repeat(padding),
            },
        });
        if let Some(entry_type) = entry_type {
            record["type"] = json!(entry_type);
        }
        let line = record
            .to_string()
            .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#)
            .replace(r#""type":"token_count""#, r#""type" : "token_count""#);
        let fixture = fs_fixture!({ "headless.jsonl": line });

        let events = crate::test_support::rollout_events(fixture.root());

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].output_tokens, 20);
        assert_eq!(events[0].total_tokens, 120);
    }
}

#[test]
fn loads_headless_created_at_after_session_candidate_decode_fails() {
    for timestamp in [json!(false), json!({"SECRET_CANARY": true})] {
        let record = json!({
            "type": "result",
            "timestamp": timestamp,
            "created_at": "2026-09-01T00:00:01Z",
            "model": "gpt-5-mini",
            "usage": { "input_tokens": 100, "output_tokens": 20 },
            "data": {
                "type": "event_msg",
                "nested": { "type": "token_count" },
                "padding": "x".repeat(70 * 1024),
            },
        });
        let line = record
            .to_string()
            .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#)
            .replace(r#""type":"token_count""#, r#""type" : "token_count""#);
        let fixture = fs_fixture!({ "headless.jsonl": line });

        let events = crate::test_support::rollout_events(fixture.root());

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].last_activity, "2026-09-01T00:00:01.000Z");
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].output_tokens, 20);
        assert_eq!(events[0].total_tokens, 120);
    }
}

#[test]
fn headless_decoder_recovers_malformed_optional_fields_and_usage() {
    for line in [
        r#"{"timestamp":false,"created_at":"2026-09-01T00:00:01Z","model":{},"model_name":"gpt-5-mini","usage":{"input_tokens":10,"output_tokens":2}}"#,
        r#"{"timestamp":[],"metadata":{"model":false},"data":{"model_name":"gpt-5-mini","createdAt":1788220801000,"usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"usage":{"input_tokens":[]},"response":{"usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"usage":{"input_tokens":{}},"result":{"usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"payload":{"info":{"last_token_usage":{"input_tokens":{}}}},"usage":{"input_tokens":10,"output_tokens":2}}"#,
        r#"{"metadata":{"model":[]},"response":{"metadata":{"model":{}},"usage":{"input_tokens":10,"output_tokens":2}}}"#,
    ] {
        let typed: CodexLogEntry<'_> = serde_json::from_str(line).unwrap();
        let usage = normalize_headless_codex_usage(&typed).unwrap().unwrap();
        assert_eq!(usage.input_tokens, 10, "{line}");
        assert_eq!(usage.output_tokens, 2, "{line}");
        let fixture = fs_fixture!({ "headless.jsonl": line });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.len(), 1, "{line}");
        assert_eq!(events[0].input_tokens, 10, "{line}");
        assert_eq!(events[0].output_tokens, 2, "{line}");
    }
}

#[test]
fn headless_decoder_keeps_the_last_duplicate_field() {
    for line in [
        r#"{"model":"old","model":"gpt-5-mini","usage":{"input_tokens":999},"usage":{"input_tokens":10,"output_tokens":2}}"#,
        r#"{"data":{"model":"old","model":"gpt-5-mini","usage":{"input_tokens":999,"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"data":{"usage":{"input_tokens":999}},"data":{"model":"gpt-5-mini","usage":{"input_tokens":10,"output_tokens":2}}}"#,
    ] {
        let typed: CodexLogEntry<'_> = serde_json::from_str(line).unwrap();
        assert_eq!(
            codex_model_from_result(&typed).as_deref(),
            Some("gpt-5-mini")
        );
        assert_eq!(
            normalize_headless_codex_usage(&typed)
                .unwrap()
                .unwrap()
                .input_tokens,
            10
        );
        let fixture = fs_fixture!({ "headless.jsonl": line });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model, "gpt-5-mini");
        assert_eq!(events[0].input_tokens, 10);
    }
}

#[test]
fn session_usage_ignores_unrelated_headless_fields() {
    let fixture = fs_fixture!({
        "session.jsonl": r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","model":{},"created_at":false,"data":{"model":[]},"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
    });
    let events = crate::test_support::rollout_events(fixture.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, 10);
    assert_eq!(events[0].model, "gpt-5");
}

#[test]
fn recognizes_compaction_usage_lines_with_json_whitespace() {
    for line in [
        br#"{"type":"token_usage_record","payload":{}}"#.as_slice(),
        br#"{ "type" : "token_usage_record", "payload" : {} }"#.as_slice(),
        br#"{ "type" : "compacted", "payload" : {} }"#.as_slice(),
    ] {
        assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
    }
}

#[test]
fn counts_only_usage_records_linked_to_compaction_events() {
    let token_count = |timestamp: &str| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 80,
                        "output_tokens": 20,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 120,
                    },
                },
            },
        })
        .to_string()
    };
    let token_usage_record = |timestamp: &str, response_id: &str, input_tokens: u64| {
        json!({
            "timestamp": timestamp,
            "type": "token_usage_record",
            "payload": {
                "response_id": response_id,
                "usage": {
                    "input_tokens": input_tokens,
                    "cached_input_tokens": input_tokens - 20,
                    "output_tokens": 20,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input_tokens + 20,
                },
            },
        })
        .to_string()
    };
    let fixture = fs_fixture!({
        "session.jsonl": [
            json!({
                "timestamp": "2026-09-01T00:00:00.000Z",
                "type": "turn_context",
                "payload": { "model": "gpt-reserve" },
            })
            .to_string(),
            token_count("2026-09-01T00:00:01.000Z"),
            token_usage_record("2026-09-01T00:00:02.000Z", "compact-before", 300),
            json!({
                "timestamp": "2026-09-01T00:00:03.000Z",
                "type": "compacted",
                "payload": { "compaction_response_id": "compact-before" },
            })
            .to_string(),
            json!({
                "timestamp": "2026-09-01T00:00:04.000Z",
                "type": "compacted",
                "payload": { "compaction_response_id": "compact-before" },
            })
            .to_string(),
            json!({
                "timestamp": "2026-09-01T00:00:05.000Z",
                "type": "compacted",
                "payload": { "compaction_response_id": "compact-after" },
            })
            .to_string(),
            token_usage_record("2026-09-01T00:00:06.000Z", "compact-after", 200),
            token_usage_record("2026-09-01T00:00:07.000Z", "not-a-compaction", 500),
            token_count("2026-09-01T00:00:08.000Z"),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 3);
    let compacted_usages = events
        .iter()
        .filter(|event| event.input_tokens > 100)
        .collect::<Vec<_>>();
    assert_eq!(compacted_usages.len(), 2);
    assert_eq!(compacted_usages[0].input_tokens, 300);
    assert_eq!(compacted_usages[0].cached_input_tokens, 280);
    assert_eq!(compacted_usages[0].output_tokens, 20);
    assert_eq!(compacted_usages[0].total_tokens, 320);
    assert_eq!(compacted_usages[0].model.as_str(), "gpt-reserve");
    assert!(compacted_usages[0].is_fallback_model);
    assert_eq!(compacted_usages[1].input_tokens, 200);
}

#[test]
fn usage_record_models_do_not_replace_the_active_turn_model() {
    for marker in [
        r#"{"type":"compacted","payload":{"compaction_response_id":"response-1"}}"#,
        r#"{"type":"compacted","payload":{"compaction_response_id":"different-response"}}"#,
    ] {
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
                r#"{"timestamp":"2026-09-01T00:00:01Z","type":"token_usage_record","payload":{"response_id":"response-1","model":"gpt-5-mini","usage":{"input_tokens":300,"output_tokens":30}}}"#,
                marker,
                r#"{"timestamp":"2026-09-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            ].join("\n"),
        });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.last().unwrap().model.as_str(), "gpt-5");
        assert!(!events.last().unwrap().is_fallback_model);
    }
}

#[test]
fn recognizes_spaced_compaction_types_with_compact_nested_types() {
    for line in [
        br#"{"type" : "token_usage_record","payload":{"type":"request"}}"#.as_slice(),
        br#"{"type" : "compacted","payload":{"type":"summary"}}"#.as_slice(),
    ] {
        assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
    }
}

#[test]
fn headless_usage_field_search_matches_the_previous_implementation() {
    let markers: [&[u8]; 3] = [
        br#""usage":"#,
        br#""input_tokens":"#,
        br#""prompt_tokens":"#,
    ];
    let fragments: &[&[u8]] = &[
        br#""usage":"#,
        br#""input_tokens":"#,
        br#""prompt_tokens":"#,
        br#""usage" :"#,
        br#""cached_input_tokens":"#,
        br#""my_usage":"#,
        br#""usage_details":"#,
        br#"\"usage\":"#,
        br#"\\u0061"#,
        br#"\u0061"#,
        br#""#,
        b":",
        b"{",
        b"}",
        b"\\",
        b"\\\\",
        b" ",
    ];
    let finders = markers.map(Finder::new);
    let mut state = 0xbb67_ae85_84ca_a73b_u64;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).unwrap_or_default()
    };
    for _ in 0..50_000 {
        let line = (0..next(32))
            .flat_map(|_| fragments[next(fragments.len())].iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(
            has_headless_usage_field(&line),
            finders.iter().any(|finder| finder.find(&line).is_some()),
            "{}",
            String::from_utf8_lossy(&line)
        );
    }
}

#[test]
fn recognizes_headless_usage_fields_only_as_whole_keys() {
    for line in [
        br#"{"type":"turn.completed","usage":{"input_tokens":1}}"#.as_slice(),
        br#"{"stats":{"input_tokens":1}}"#.as_slice(),
        br#"{"x":1,"prompt_tokens":2}"#.as_slice(),
        br#"{"type":"turn.completed","us\u0061ge":{"input_tokens":1}}"#.as_slice(),
    ] {
        assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Headless));
    }
    for line in [
        br#"{"usage" :{"input_tokens" :1}}"#.as_slice(),
        br#"{"text":"\"usage\":1","cached_input_tokens":1}"#.as_slice(),
        br#"{"my_usage":1,"prompt_tokens_details":{}}"#.as_slice(),
    ] {
        assert_matches!(codex_line_usage_kind(line), None);
    }
}

#[test]
fn recognizes_usage_lines_whose_markers_are_spelled_with_escapes() {
    for (line, kind) in [
        (
            br#"{"type":"turn\u005fcontext","payload":{}}"#.as_slice(),
            CodexLineKind::Escaped,
        ),
        (
            br#"{"type":"event_msg","payload":{"type":"token\u005fcount"}}"#.as_slice(),
            CodexLineKind::Escaped,
        ),
        (
            br#"{"type":"compact\u0065d","payload":{}}"#.as_slice(),
            CodexLineKind::Escaped,
        ),
        (
            br#"{"type":"turn.completed","us\u0061ge":{"output_tokens":1}}"#.as_slice(),
            CodexLineKind::Escaped,
        ),
    ] {
        assert_eq!(
            codex_line_usage_kind(line),
            Some(kind),
            "{}",
            String::from_utf8_lossy(line)
        );
    }
    assert_eq!(
        codex_line_usage_kind(br#"{"type":"response_item","payload":{"text":"a\nb \u001b[1m"}}"#),
        None
    );
}

#[test]
fn loads_token_counts_whose_types_are_spelled_with_escapes() {
    let fixture = fs_fixture!({
        "session.jsonl": [
            r#"{"type":"turn\u005fcontext","payload":{"model":"gpt-5-mini"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event\u005fmsg","payload":{"type":"token\u005fcount","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
        ].join("\n"),
    });
    let events = crate::test_support::rollout_events(fixture.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 120);
    assert_eq!(events[0].model.as_str(), "gpt-5-mini");
}

#[test]
fn loads_token_counts_with_spaces_after_type_colons() {
    let fixture = fs_fixture!({
        "session.jsonl": [
            r#"{"type": "turn_context", "payload": {"model": "gpt-5-mini"}}"#,
            r#"{"timestamp": "2026-09-01T00:00:01Z", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 20}}}}"#,
        ].join("\n"),
    });
    let events = crate::test_support::rollout_events(fixture.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 120);
    assert_eq!(events[0].model.as_str(), "gpt-5-mini");
}
#[test]
fn counts_compaction_usage_without_changing_cumulative_baseline() {
    let fixture = ccusage_test_support::fs_fixture!({
        "session.jsonl": [
            r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"token_usage_record","payload":{"response_id":"compaction-1","usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":30}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:03Z","type":"compacted","payload":{"compaction_response_id":"compaction-1"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"output_tokens":30}}}}"#,
        ].join("\n"),
    });
    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .map(|event| event.total_tokens)
            .collect::<Vec<_>>(),
        [120, 330, 60]
    );
    assert_eq!(events[1].cached_input_tokens, 200);
    assert_eq!(events[1].model.as_str(), "gpt-5");
    assert!(events[1].is_fallback_model);
}
