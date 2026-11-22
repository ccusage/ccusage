use super::*;

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
