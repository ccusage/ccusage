use serde_json::json;

use super::{
    MissingTimestamp, OpenCodeMessage, OpenCodeSessionAggregate, SessionMessageKind,
    SessionMessageRow, message_time_created, message_value_to_entry, open_code_model_candidates,
    session_message_value_to_entry_checked, session_value_to_entry,
};
use ccusage_core::{LoadedEntry, PricingMap, cli::CostMode};

fn candidate_names(model: &str, provider: &str) -> Vec<String> {
    open_code_model_candidates(model, provider)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn v2_native_session(data: &str) -> Option<String> {
    session_message_value_to_entry_checked(
        SessionMessageRow {
            kind: SessionMessageKind::Assistant,
            data,
            id: "msg-v2".to_string(),
            session_id: "v2-session".to_string(),
            created: Some(1_767_312_000_000),
        },
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        ccusage_core::cost::PriceTable::Display,
    )
    .unwrap()
    .unwrap()
    .1
    .map(|session| session.to_string())
}

#[test]
fn links_claude_agent_rows_to_their_claude_session() {
    assert_eq!(
        v2_native_session(
            r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"reasoning","text":"…"},{"type":"text","native":{"sessionID":"claude-session-1","messageID":"m"}}]}"#
        )
        .as_deref(),
        Some("claude-session-1")
    );
}

#[test]
fn ignores_native_links_from_other_providers() {
    assert_eq!(
        v2_native_session(
            r#"{"model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":100},"content":[{"native":{"sessionID":"other-session"}}]}"#
        ),
        None
    );
}

#[test]
fn leaves_claude_agent_rows_without_a_native_link_unlinked() {
    assert_eq!(
        v2_native_session(
            r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"text","native":{"sessionID":""}},"not a part"]}"#
        ),
        None
    );
}

#[test]
fn keeps_the_native_link_when_other_parts_are_malformed() {
    for content in [
        r#"[{"native":{"sessionID":"s"}},"junk"]"#,
        r#"[{"native":5},{"native":{"sessionID":7}},[1,{"a":2}],null,{"native":{"sessionID":"s"}}]"#,
        r#"[{"native":{"sessionID":{"x":1}}},{"native":{"sessionID":["y"]}},{"native":{"sessionID":"s"}}]"#,
        r#"[{"native":{"sessionID":"s"}},{"native":{"sessionID":{"x":1}}},{"native":{"sessionID":["y"]}}]"#,
        r#"[{"native":{"sessionID":"s","sessionID":"t"}}]"#,
        r#"[{"native":{"sessionID":5,"sessionID":"s"}}]"#,
        r#"[{"native":7,"native":{"sessionID":"s"},"native":{"sessionID":"t"}}]"#,
    ] {
        assert_eq!(
            v2_native_session(&format!(
                r#"{{"model":{{"id":"opus","providerID":"claude-agent"}},"tokens":{{"input":100}},"content":{content}}}"#
            ))
            .as_deref(),
            Some("s"),
            "{content}"
        );
    }
}

#[test]
fn skips_blank_native_session_ids() {
    assert_eq!(
        v2_native_session(
            r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"text","native":{"sessionID":" "}},{"type":"tool","native":{"sessionID":"claude-session-2","resultState":{"x":[1]}}}]}"#
        )
        .as_deref(),
        Some("claude-session-2")
    );
}

fn message(value: serde_json::Value) -> OpenCodeMessage {
    serde_json::from_value(value).unwrap()
}

fn entry_snapshot(entry: &LoadedEntry) -> serde_json::Value {
    json!({
        "date": entry.date.to_string(),
        "timestamp": entry.timestamp.as_millis(),
        "sessionId": entry.session_id.as_ref(),
        "project": entry.project.as_ref(),
        "projectPath": entry.project_path.as_ref(),
        "model": entry.model.as_deref(),
        "messageId": entry.message_id.as_deref(),
        "version": entry.version.as_deref(),
        "usage": {
            "inputTokens": entry.usage.input_tokens,
            "outputTokens": entry.usage.output_tokens,
            "cacheCreationInputTokens": entry.usage.cache_creation_input_tokens,
            "cacheReadInputTokens": entry.usage.cache_read_input_tokens,
        },
        "extraTotalTokens": entry.extra_total_tokens,
        "cost": entry.cost,
        "recordedCost": entry.recorded_cost,
        "storedCost": entry.stored_cost,
        "missingPricingModel": entry.missing_pricing_model.as_deref(),
    })
}

#[test]
fn calculates_cost_when_opencode_stores_zero_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                }
            }"#,
    );
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "input": 100,
                "output": 10,
                "cache": { "read": 50 }
            },
            "cost": 0
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::Auto(&pricing),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.cost, 0.000205);
}

#[test]
fn resolves_a_provider_identity_before_fuzzy_matches_for_both_token_sides() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(r#"{
            "azure/us-gov/text-embedding-3-small": {"input_cost_per_token": 0.025, "output_cost_per_token": 0},
            "ambient/small": {"input_cost_per_token": 0.001, "output_cost_per_token": 0.001}
        }"#);
    for (case, tokens) in [
        ("73_input_only", json!({"input": 100})),
        ("74_output_only", json!({"output": 100})),
    ] {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "ambient",
                "modelID": "small",
                "time": {"created": 1767312000000i64},
                "tokens": tokens
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, 0.1, "{case}");
        assert_eq!(entry.missing_pricing_model, None, "{case}");
    }
}

#[test]
fn a_logged_bedrock_provider_bills_the_bedrock_listing_ahead_of_the_vendor_price() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
            "deepseek/deepseek-v3.2": {"litellm_provider": "deepseek", "input_cost_per_token": 0.00000028, "output_cost_per_token": 0.0000004},
            "deepseek.v3.2": {"litellm_provider": "bedrock_converse", "input_cost_per_token": 0.00000062, "output_cost_per_token": 0.00000185}
        }"#,
    );
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "amazon-bedrock",
            "modelID": "deepseek.v3.2",
            "time": {"created": 1767312000000i64},
            "tokens": {"input": 1000000}
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();
    assert!((entry.cost - 0.62).abs() < 1e-9, "{}", entry.cost);
}

#[test]
fn keeps_the_same_model_when_its_output_rate_is_zero() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
            "small": {"input_cost_per_token": 0.001, "output_cost_per_token": 0.001},
            "ambient/small": {"input_cost_per_token": 0.025, "output_cost_per_token": 0}
        }"#,
    );
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "ambient",
            "modelID": "small",
            "time": {"created": 1767312000000i64},
            "tokens": {"output": 100}
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();
    assert_eq!(entry.cost, 0.0);
    assert_eq!(entry.missing_pricing_model, None);
}

#[test]
fn keeps_positive_opencode_cost() {
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "input": 100
            },
            "cost": 0.02
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::for_map(
            CostMode::Auto,
            &ccusage_core::PricingMap::default(),
        ),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.cost, 0.02);
}

#[test]
fn honors_cost_modes_for_recorded_and_unrecorded_costs() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing
        .load_json(r#"{"gpt-test":{"input_cost_per_token":0.001,"output_cost_per_token":0.01}}"#);
    for (mode, recorded, expected, stored) in [
        (CostMode::Calculate, Some(9.0), 0.2, None),
        (CostMode::Display, None, 0.0, None),
        (CostMode::Display, Some(0.0), 0.0, None),
        (CostMode::Display, Some(9.0), 9.0, None),
        (CostMode::Auto, Some(9.0), 9.0, Some(9.0)),
        (CostMode::Auto, Some(0.0), 0.2, None),
        (CostMode::Auto, Some(-1.0), 0.2, None),
        (CostMode::Auto, None, 0.2, None),
        (CostMode::Calculate, None, 0.2, None),
    ] {
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "openai", "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 100, "output": 10 }, "cost": recorded
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            mode,
            ccusage_core::cost::PriceTable::for_map(mode, &pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, expected, "{mode:?}, {recorded:?}");
        assert_eq!(entry.stored_cost, stored, "{mode:?}, {recorded:?}");
        assert_eq!(entry.missing_pricing_model, None);
    }
}

#[test]
fn keeps_a_free_first_candidate_instead_of_a_paid_provider_candidate() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test":{"litellm_provider": "openai", "input_cost_per_token":0,"output_cost_per_token":0},
                "openai/gpt-test":{"input_cost_per_token":1,"output_cost_per_token":2}
            }"#,
    );
    let entry = message_value_to_entry(
        message(json!({
            "providerID": "openai", "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": { "input": 100, "output": 10 }, "cost": 0
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::Auto(&pricing),
    )
    .unwrap()
    .unwrap();
    assert_eq!(entry.cost, 0.0);
    assert_eq!(entry.missing_pricing_model, None);
}

#[test]
fn reports_missing_calculated_prices_even_with_a_recorded_cost() {
    let pricing = ccusage_core::PricingMap::default();
    for (mode, expected_cost, missing) in [
        (CostMode::Calculate, 0.0, Some("opencode-unpriced-model")),
        (CostMode::Auto, 9.0, None),
        (CostMode::Display, 9.0, None),
    ] {
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "openai", "modelID": "opencode-unpriced-model",
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 100 }, "cost": 9
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            mode,
            ccusage_core::cost::PriceTable::for_map(mode, &pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, expected_cost, "{mode:?}");
        assert_eq!(entry.missing_pricing_model.as_deref(), missing, "{mode:?}");
    }
}

#[test]
fn prices_the_logged_id_before_its_opencode_alias() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "k2p6":{"input_cost_per_token":1,"output_cost_per_token":1},
                "kimi-k2.6":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
    );
    let entry = message_value_to_entry(
        message(json!({
            "providerID": "openrouter", "modelID": "k2p6",
            "time": { "created": 1767312000000i64 },
            "tokens": { "input": 100 }
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();
    assert_eq!(entry.cost, 100.0);
}

#[test]
fn prices_a_derived_model_at_the_vendor_rate_until_the_host_id_is_logged() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gemini-3-pro-preview":{"input_cost_per_token":1,"output_cost_per_token":1},
            "vertex_ai/gemini-3-pro-preview":{"input_cost_per_token":2,"output_cost_per_token":2}
        }"#,
    );
    for (model, expected) in [
        ("gemini-3-pro-high", 100.0),
        ("gemini-3-pro-preview", 200.0),
    ] {
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "google-vertex", "modelID": model,
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 100 }
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, expected, "{model}");
    }
}

#[test]
fn primary_rates_outrank_models_dev_for_qualified_candidates() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::load_embedded();
    pricing.load_json(
        r#"{"claude-sonnet-4":{"litellm_provider":"anthropic","input_cost_per_token":0.125,"output_cost_per_token":0.25}}"#,
    );
    for model in [
        "anthropic/claude-sonnet-4",
        "openrouter/anthropic/claude-sonnet-4",
    ] {
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "openrouter", "modelID": model,
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 100, "output": 10 }, "cost": 0
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, 15.0, "{model}");
    }
}

#[test]
fn bills_reasoning_and_preserves_unbilled_residual_in_total() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing
        .load_json(r#"{"gpt-test":{"input_cost_per_token":0.125,"output_cost_per_token":0.25}}"#);
    let entry = message_value_to_entry(
        message(json!({
            "providerID": "openai", "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": { "input": 8, "output": 4, "reasoning": 2, "total": 20 }
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();
    assert_eq!(entry.usage.output_tokens, 4);
    assert_eq!(entry.extra_total_tokens, 8);
    assert_eq!(entry.cost, 2.5);
}

#[test]
fn keeps_opencode_record_when_cache_field_is_not_an_object() {
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "input": 100,
                "output": 10,
                "cache": 0
            },
            "cost": 0.02
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::for_map(
            CostMode::Auto,
            &ccusage_core::PricingMap::default(),
        ),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.usage.input_tokens, 100);
    assert_eq!(entry.usage.output_tokens, 10);
    assert_eq!(entry.usage.cache_creation_input_tokens, 0);
    assert_eq!(entry.usage.cache_read_input_tokens, 0);
    assert_eq!(entry.cost, 0.02);
}

#[test]
fn keeps_total_only_usage_out_of_output_tokens() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{"gpt-test":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "total": 123
            }
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.usage.output_tokens, 0);
    assert_eq!(entry.extra_total_tokens, 123);
    assert_eq!(entry.cost, 0.0);
}

#[test]
fn creates_open_code_provider_and_normalized_model_candidates() {
    assert_eq!(
        candidate_names("claude-sonnet-4.5", "github-copilot"),
        vec![
            "claude-sonnet-4.5",
            "claude-sonnet-4-5",
            "github_copilot/claude-sonnet-4.5",
            "github_copilot/claude-sonnet-4-5",
        ]
    );
}

#[test]
fn preserves_legacy_prices_for_provider_qualified_models() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = ccusage_core::PricingMap::load_embedded();
    let models = [
        ("anthropic", "claude-sonnet-4-20250514"),
        ("anthropic", "claude-sonnet-4.5"),
        ("anthropic", "claude-sonnet-45"),
        ("anthropic", "claude-opus-4-20250514"),
        ("anthropic", "claude-haiku-4-5-20251001"),
        ("openai", "gpt-4o"),
        ("openai", "gpt-4.1"),
        ("openai", "gpt-5"),
        ("openai", "gpt-5-mini"),
        ("openai", "o3"),
        ("openai", "o4-mini"),
        ("google", "gemini-2.5-pro"),
        ("google", "gemini-2.5-flash"),
        ("google", "gemini-3-pro-high"),
        ("google", "gemini-3-pro-preview"),
        ("deepseek", "deepseek-chat"),
        ("deepseek", "deepseek-v4-flash"),
        ("xai", "grok-3"),
        ("xai", "grok-4"),
        ("moonshotai", "kimi-k2.6"),
        ("kimi-for-coding", "k2p6"),
        ("github-copilot", "claude-sonnet-4.5"),
        ("qwen", "qwen3-coder"),
        ("unknown", "opencode-unpriced-model"),
    ];
    let usage = crate::TokenUsageRaw {
        input_tokens: 1234,
        output_tokens: 567,
        cache_read_input_tokens: 89,
        cache_creation_input_tokens: 23,
        ..Default::default()
    };
    let mut comparisons = 0;
    let mut priced = 0;
    for (provider, model) in models {
        for (provider, model) in [
            (provider, model.to_string()),
            (provider, format!("{provider}/{model}")),
            ("openrouter", format!("{provider}/{model}")),
            ("openrouter", format!("openrouter/{provider}/{model}")),
        ] {
            for timestamp in ["2026-08-17T01:00:00Z", "2026-08-17T13:00:00Z"] {
                let timestamp = ccusage_core::parse_ts_timestamp(timestamp).unwrap();
                let old_cost = candidate_names(&model, provider)
                    .into_iter()
                    .map(|candidate| {
                        ccusage_core::cost::price_resolved_usage(
                            &candidate,
                            usage,
                            timestamp,
                            pricing.find_handle(&candidate).as_ref(),
                        )
                        .0
                    })
                    .find(|cost| *cost > 0.0)
                    .unwrap_or(0.0);
                let entry = message_value_to_entry(
                    message(json!({
                        "providerID": provider, "modelID": model,
                        "time": { "created": timestamp.as_millis() },
                        "tokens": { "input": 1234, "output": 567,
                            "cache": { "read": 89, "write": 23 } },
                        "cost": 0
                    })),
                    None,
                    None,
                    &jiff::tz::TimeZone::UTC,
                    CostMode::Auto,
                    ccusage_core::cost::PriceTable::Auto(&pricing),
                )
                .unwrap()
                .unwrap();
                // NOTE(2026-11-20, #2332): A foreign-routed DeepSeek V4 request keeps the flat rate instead of DeepSeek's schedule. Its legacy price therefore differs, and the flat price is pinned here.
                if provider == "openrouter" && model.contains("deepseek-v4-flash") {
                    assert!(
                        (entry.cost - 0.001051134).abs() < 1e-12,
                        "{provider}/{model} at {timestamp:?}: {}",
                        entry.cost
                    );
                } else {
                    assert_eq!(entry.cost, old_cost, "{provider}/{model} at {timestamp:?}");
                }
                comparisons += 1;
                priced += usize::from(old_cost > 0.0);
            }
        }
    }
    assert_eq!(comparisons, 192);
    assert!(
        priced >= 144,
        "only {priced}/{comparisons} cases were priced"
    );
    eprintln!("Legacy pricing parity: {comparisons} cases, {priced} priced");
}

#[test]
fn calculates_cost_for_k2p6_when_opencode_stores_zero_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = ccusage_core::PricingMap::load_embedded();
    let entry = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "kimi-for-coding",
            "modelID": "k2p6",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "input": 100,
                "output": 10,
                "cache": { "read": 50 }
            },
            "cost": 0
        })),
        None,
        None,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::Auto(&pricing),
    )
    .unwrap()
    .unwrap();

    assert!(
        (entry.cost - 0.000143).abs() < 1e-12,
        "k2p6 cost was {}",
        entry.cost
    );
}

#[test]
fn snapshots_message_to_entry_variants_and_model_candidates() {
    ccusage_test_support::install_pricing_snapshots();
    let tz = ccusage_core::resolve_tz(Some(&ccusage_core::cli::ReportTimezone::UTC));
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "github_copilot/claude-sonnet-4-5": {
                    "input_cost_per_token": 0.125,
                    "output_cost_per_token": 0.25,
                    "cache_read_input_token_cost": 0.0625
                }
            }"#,
    );
    let calculated = message_value_to_entry(
        message(json!({
            "id": "message-a",
            "sessionID": "session-a",
            "providerID": "github-copilot",
            "modelID": "claude-sonnet-4.5",
            "time": { "created": 1767312000000i64 },
            "tokens": {
                "input": 100,
                "output": 10,
                "cache": { "read": 50, "write": 25 },
                "total": 185
            },
            "cost": 0
        })),
        None,
        None,
        &tz,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::Auto(&pricing),
    )
    .unwrap()
    .unwrap();
    let display_cost = message_value_to_entry(
        message(json!({
            "id": "message-b",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": { "total": 123 },
            "cost": 0.02
        })),
        None,
        Some("explicit-session".to_string()),
        &tz,
        CostMode::Display,
        ccusage_core::cost::PriceTable::Display,
    )
    .unwrap()
    .unwrap();
    let unknown_session = message_value_to_entry(
        message(json!({
            "id": "message-c",
            "providerID": "openai",
            "modelID": "gpt-test",
            "time": { "created": 1767312000000i64 },
            "tokens": { "input": 7, "cache": { "write": 3 } },
            "cost": 0.03
        })),
        None,
        None,
        &tz,
        CostMode::Auto,
        ccusage_core::cost::PriceTable::Auto(&pricing),
    )
    .unwrap()
    .unwrap();

    insta::assert_json_snapshot!(json!({
        "calculated": entry_snapshot(&calculated),
        "displayCost": entry_snapshot(&display_cost),
        "unknownSession": entry_snapshot(&unknown_session),
        "candidates": {
            "anthropic": candidate_names("claude-sonnet-4.5", "anthropic"),
            "copilot": candidate_names("claude-sonnet-4.5", "github-copilot"),
            "geminiAlias": candidate_names("gemini-3-pro-high", "google"),
            "unknownProvider": candidate_names("gpt-test", "unknown"),
        }
    }));
}

fn deepseek_pricing(input: f64) -> PricingMap {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(&format!(
        "{{\"deepseek-v4-flash\":{{\"litellm_provider\":\"deepseek\",\"input_cost_per_token\":{input},\"output_cost_per_token\":0.00000028}}}}"
    ));
    pricing
}

#[test]
fn reads_only_the_messages_own_creation_time() {
    for (payload, expected) in [
        (
            r#"{"time":{"created":1767312000000}}"#,
            Some(1_767_312_000_000),
        ),
        ("{\n  \"time\": {\n    \"created\": 42\n  }\n}", Some(42)),
        (
            r#"{"metadata":{"time":{"created":1}},"time":{"created":1767312000000}}"#,
            Some(1_767_312_000_000),
        ),
        (r#"{"parts":[{"time":{"created":1}}]}"#, None),
        (r#"{"unit":"time","created":1767312000000}"#, None),
        (r#"{"time":{"created":"1767312000000"}}"#, None),
        (r#"{"time":{"created":-5}}"#, None),
        (r#"{"time":{"completed":1767312000000}}"#, None),
        (r#"{"time":"2026-01-02"}"#, None),
        (r#"{"\u0074ime":{"created":42}}"#, Some(42)),
        (r#"{"id":"#, None),
        (r#"[{"time":{"created":1767312000000}}]"#, None),
    ] {
        assert_eq!(
            message_time_created(payload.as_bytes()),
            expected,
            "{payload}"
        );
    }
}

#[test]
fn rejects_usage_without_a_positive_creation_time() {
    for time in [
        json!(null),
        json!({}),
        json!({ "created": 0 }),
        json!({ "created": -1 }),
    ] {
        assert_eq!(
            message_value_to_entry(
                message(json!({
                    "id": "message-a",
                    "sessionID": "session-a",
                    "providerID": "openai",
                    "modelID": "gpt-test",
                    "time": time,
                    "tokens": { "input": 10 }
                })),
                None,
                None,
                &jiff::tz::TimeZone::UTC,
                CostMode::Display,
                ccusage_core::cost::PriceTable::Display,
            )
            .map(|entry| entry.is_some()),
            Err(MissingTimestamp),
            "{time}"
        );
    }
}

#[test]
fn rejects_session_aggregates_without_a_positive_creation_time() {
    let aggregate = OpenCodeSessionAggregate {
        session_id: "session-a".to_string(),
        created: Some(0),
        model: "gpt-test".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 0,
        reasoning_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost: None,
    };

    assert_eq!(
        session_value_to_entry(
            aggregate,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            ccusage_core::cost::PriceTable::Display
        )
        .map(|entry| entry.is_some()),
        Err(MissingTimestamp)
    );
}

#[test]
fn prices_cumulative_session_aggregates_with_static_rates() {
    let pricing = deepseek_pricing(0.00000014);
    let created = ccusage_core::parse_ts_timestamp("2026-08-17T01:00:00Z")
        .unwrap()
        .as_millis();
    let entry = session_value_to_entry(
        OpenCodeSessionAggregate {
            session_id: "session-a".to_string(),
            created: Some(created),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: None,
        },
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.timestamp.as_millis(), created);
    assert_eq!(entry.cost, 0.14);
}

#[test]
fn prices_a_session_aggregate_at_the_base_tier() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"tiered-test":{"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":1,"input_cost_per_token_above_200k_tokens":2}}"#,
    );
    let created = ccusage_core::parse_ts_timestamp("2026-08-17T01:00:00Z")
        .unwrap()
        .as_millis();
    let entry = session_value_to_entry(
        OpenCodeSessionAggregate {
            session_id: "session-a".to_string(),
            created: Some(created),
            model: "tiered-test".to_string(),
            provider: "openai".to_string(),
            input_tokens: 250_000,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: None,
        },
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.cost, 250_000.0);
}

#[test]
fn prices_a_foreign_router_without_the_deepseek_schedule() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let created = ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z")
        .unwrap()
        .as_millis();
    for (provider, expected) in [("openrouter", 0.0015), ("deepseek", 0.00075)] {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-deepseek",
                "sessionID": "session-a",
                "providerID": provider,
                "modelID": "deepseek-v4-flash",
                "time": { "created": created },
                "tokens": { "input": 1000, "output": 1000 }
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();

        assert!(
            (entry.cost - expected).abs() < 1e-12,
            "{provider}: {}",
            entry.cost
        );
        assert_eq!(entry.missing_pricing_model, None, "{provider}");
    }
}

#[test]
fn reports_claude_agent_model_aliases_as_missing_pricing_instead_of_regional_rates() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "claude-opus-5-5": {"litellm_provider": "anthropic", "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5},
            "us-gov.anthropic.claude-opus-5": {"litellm_provider": "bedrock_converse", "input_cost_per_token": 6e-6, "output_cost_per_token": 3e-5},
            "claude-sonnet-5-5": {"litellm_provider": "anthropic", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5},
            "bedrock/us-gov-west-1/anthropic.claude-sonnet-5-5": {"litellm_provider": "bedrock", "input_cost_per_token": 3.6e-6, "output_cost_per_token": 1.8e-5},
            "claude-haiku-5-5": {"litellm_provider": "anthropic", "input_cost_per_token": 1e-6, "output_cost_per_token": 5e-6},
            "us-gov.anthropic.claude-haiku-5-5": {"litellm_provider": "bedrock_converse", "input_cost_per_token": 1.2e-6, "output_cost_per_token": 6e-6},
            "claude-fable-5-2": {"litellm_provider": "anthropic", "input_cost_per_token": 8e-6, "output_cost_per_token": 4e-5},
            "us-gov.anthropic.claude-fable-5-1": {"litellm_provider": "bedrock_converse", "input_cost_per_token": 1.2e-5, "output_cost_per_token": 6e-5}
        }"#,
    );
    for (model, expected) in [
        ("claude-opus-5-5", (4.0, None)),
        ("opus", (0.0, Some("opus"))),
        ("sonnet", (0.0, Some("sonnet"))),
        ("haiku", (0.0, Some("haiku"))),
        ("fable", (0.0, Some("fable"))),
    ] {
        let data = format!(
            r#"{{"model":{{"id":"{model}","providerID":"claude-agent"}},"tokens":{{"input":1000000}},"cost":0}}"#
        );
        let (entry, _) = session_message_value_to_entry_checked(
            SessionMessageRow {
                kind: SessionMessageKind::Assistant,
                data: &data,
                id: "msg-v2".to_string(),
                session_id: "v2-session".to_string(),
                created: Some(1_767_312_000_000),
            },
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (entry.cost, entry.missing_pricing_model.as_deref()),
            expected,
            "{model}"
        );
    }
}

#[test]
fn a_hyphenated_paid_host_marks_a_free_vendor_model_as_an_estimate() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (provider, estimated) in [("novita-ai", true), ("ollama", false)] {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": provider,
                "modelID": "glm-4.7-flash",
                "time": {"created": 1767312000000i64},
                "tokens": {"input": 1000000, "output": 100000}
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.estimated_pricing, estimated, "{provider}");
        assert_eq!(entry.cost > 0.0, estimated, "{provider}: {}", entry.cost);
    }
}
