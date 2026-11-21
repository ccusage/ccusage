use super::model::{
    API_PROVIDER_GOOGLE_GEMINI, API_PROVIDER_GOOGLE_VERTEX, model_candidates, model_from_id,
    normalize_antigravity_model, price_antigravity,
};
use super::proto::{
    ProtoField, ProtoValue, field_bytes, field_bytes_all, field_text, field_varint,
    parse_generator_metadata, parse_step_metadata,
};
use super::test_support::{UsageFixture, metadata_blob_with_steps, step_metadata_blob};
use super::{DEFAULT_MODEL, EventContext, ModelUsage, append_usage_event, merge_usage_event};
use ccusage_core::{TimestampMs, TokenUsageRaw, parse_ts_timestamp};

#[test]
fn merge_prefers_resolved_then_placeholder_then_default_model() {
    let event = |model: Option<&str>| {
        let mut events = Vec::new();
        append_usage_event(
            &mut events,
            &mut std::collections::HashMap::new(),
            ModelUsage {
                input_tokens: 1,
                ..ModelUsage::default()
            },
            EventContext {
                model,
                model_id: None,
                provider: None,
                timestamp: Some((TimestampMs::UNIX_EPOCH, 3)),
                trajectory_timestamp: None,
                diagnostics: &ccusage_core::Diagnostics::new("antigravity"),
                record: (std::path::Path::new("fixture.db"), 1),
                session_id: "session",
                fallback_identities: &[],
            },
        );
        events.remove(0)
    };
    let merged_model = |target: Option<&str>, duplicate: Option<&str>| {
        let mut target = event(target);
        merge_usage_event(&mut target, event(duplicate));
        target.model
    };

    assert_eq!(
        merged_model(None, Some("model_placeholder_m401")),
        "model_placeholder_m401"
    );
    assert_eq!(
        merged_model(Some("model_placeholder_m401"), None),
        "model_placeholder_m401"
    );
    assert_eq!(
        merged_model(
            Some("model_placeholder_m401"),
            Some("claude-opus-5-5-medium")
        ),
        "claude-opus-5-5-medium"
    );
    assert_eq!(
        merged_model(
            Some("claude-opus-5-5-medium"),
            Some("model_placeholder_m401")
        ),
        "claude-opus-5-5-medium"
    );
    assert_eq!(merged_model(None, None), DEFAULT_MODEL);
}

#[test]
fn protobuf_scalars_use_last_duplicate_and_messages_keep_merge_order() {
    let first_text = b"first";
    let last_text = b"last";
    let first_message = b"message-first";
    let second_message = b"message-second";
    let fields = [
        ProtoField {
            number: 1,
            value: ProtoValue::Varint(1),
        },
        ProtoField {
            number: 1,
            value: ProtoValue::Varint(2),
        },
        ProtoField {
            number: 2,
            value: ProtoValue::Bytes(first_text),
        },
        ProtoField {
            number: 2,
            value: ProtoValue::Bytes(last_text),
        },
        ProtoField {
            number: 3,
            value: ProtoValue::Bytes(first_message),
        },
        ProtoField {
            number: 3,
            value: ProtoValue::Bytes(second_message),
        },
    ];

    assert_eq!(field_varint(&fields, 1), Some(2));
    assert_eq!(field_text(&fields, 2).as_deref(), Some("last"));
    assert_eq!(field_bytes(&fields, 3), Some(first_message.as_slice()));
    assert_eq!(
        field_bytes_all(&fields, 3),
        vec![first_message.as_slice(), second_message.as_slice()]
    );
}

#[test]
fn parses_production_step_retry_info_from_field_28() {
    let retry = UsageFixture {
        input_tokens: 11,
        total_output_tokens: 22,
        visible_output_tokens: 20,
        response_id: Some("production-step-retry"),
        ..UsageFixture::default()
    };
    let metadata = parse_step_metadata(&step_metadata_blob(
        None,
        None,
        None,
        &[retry],
        Some(API_PROVIDER_GOOGLE_GEMINI),
    ))
    .unwrap();

    assert_eq!(metadata.retry_usages.len(), 1);
    assert_eq!(metadata.retry_usages[0].input_tokens, 11);
    assert_eq!(
        metadata.retry_usages[0].response_id.as_deref(),
        Some("production-step-retry")
    );
}

#[test]
fn parses_step_indices_and_drops_mirror_retry() {
    let unkeyed = UsageFixture {
        input_tokens: 15_027,
        total_output_tokens: 49,
        visible_output_tokens: 49,
        ..UsageFixture::default()
    };
    let metadata = parse_generator_metadata(&metadata_blob_with_steps(
        Some("gemini-4-argon"),
        Some(unkeyed),
        None,
        &[unkeyed],
        &[1],
    ))
    .unwrap();

    assert_eq!(metadata.step_indices, vec![1]);
    assert!(metadata.usage.is_some());
    assert!(metadata.retry_usages.is_empty());
}

#[test]
fn keeps_distinct_retry_attempt_for_deduplication() {
    let primary = UsageFixture {
        input_tokens: 100,
        total_output_tokens: 50,
        visible_output_tokens: 40,
        reasoning_tokens: 10,
        response_id: Some("response-1"),
        provider_assigned_message_id: Some("provider-1"),
        ..UsageFixture::default()
    };
    let retry = UsageFixture {
        input_tokens: 80,
        total_output_tokens: 30,
        visible_output_tokens: 25,
        reasoning_tokens: 5,
        response_id: Some("retry-response"),
        provider_assigned_message_id: Some("provider-1"),
        ..UsageFixture::default()
    };
    let metadata = parse_generator_metadata(&metadata_blob_with_steps(
        Some("gemini-3-pro"),
        Some(primary),
        None,
        &[retry],
        &[1],
    ))
    .unwrap();

    assert_eq!(metadata.retry_usages.len(), 1);
}

#[test]
fn only_google_providers_receive_google_model_candidates() {
    let bare = "gemini-unpriced".to_string();
    assert_eq!(
        model_candidates("gemini-unpriced", Some(26)),
        vec![bare.clone()]
    );
    assert_eq!(
        model_candidates("gemini-unpriced", None),
        vec![bare.clone()]
    );
    assert_eq!(
        model_candidates("gemini-unpriced", Some(API_PROVIDER_GOOGLE_GEMINI)),
        vec![
            bare,
            "google/gemini-unpriced".to_string(),
            "gemini/gemini-unpriced".to_string(),
            "vertex_ai/gemini-unpriced".to_string(),
            "openrouter/google/gemini-unpriced".to_string(),
        ]
    );
    assert_eq!(
        model_candidates("gemini-unpriced", Some(API_PROVIDER_GOOGLE_VERTEX)),
        vec![
            "vertex_ai/gemini-unpriced".to_string(),
            "gemini-unpriced".to_string(),
        ]
    );
}

#[test]
fn missing_pricing_token_total_saturates() {
    let pricing = ccusage_core::PricingMap::default();
    let result = std::panic::catch_unwind(|| {
        price_antigravity(
            "antigravity-unpriced",
            None,
            None,
            TokenUsageRaw {
                input_tokens: u64::MAX,
                output_tokens: u64::MAX,
                cache_creation_input_tokens: u64::MAX,
                cache_read_input_tokens: u64::MAX,
                speed: None,
                cache_creation: None,
            },
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .1
    });

    assert!(result.is_ok());
    assert!(result.unwrap().is_some());
}

#[test]
fn normalizes_newer_gemini_model_placeholders_and_ids() {
    let cases = [
        (
            1318,
            "model_placeholder_m318",
            "Gemini 3.8 Flash (High)",
            "gemini-3.8-flash-high",
        ),
        (
            1319,
            "model_placeholder_m319",
            "Gemini 3.8 Flash (Medium)",
            "gemini-3.8-flash-medium",
        ),
        (
            1320,
            "model_placeholder_m320",
            "Gemini 3.8 Flash (Low)",
            "gemini-3.8-flash-low",
        ),
        (
            1298,
            "model_placeholder_m298",
            "Gemini 3.7 Flash (High)",
            "gemini-3.7-flash-high",
        ),
        (
            1299,
            "model_placeholder_m299",
            "Gemini 3.7 Flash (Medium)",
            "gemini-3.7-flash-medium",
        ),
        (
            1300,
            "model_placeholder_m300",
            "Gemini 3.7 Flash (Low)",
            "gemini-3.7-flash-low",
        ),
        (
            1071,
            "model_placeholder_m71",
            "Gemini 3.6 Flash (High)",
            "gemini-3.6-flash-high",
        ),
        (
            1072,
            "model_placeholder_m72",
            "Gemini 3.6 Flash (Medium)",
            "gemini-3.6-flash-medium",
        ),
        (
            1073,
            "model_placeholder_m73",
            "Gemini 3.6 Flash (Low)",
            "gemini-3.6-flash-low",
        ),
        (
            1417,
            "model_placeholder_m417",
            "Gemini 4 Argon",
            "gemini-4-argon",
        ),
        (
            1418,
            "model_placeholder_m418",
            "Gemini 4 Argon",
            "gemini-4-argon",
        ),
        (
            1419,
            "model_placeholder_m419",
            "Gemini 4 Argon",
            "gemini-4-argon",
        ),
        (
            1050,
            "model_placeholder_m50",
            "Gemini 3.1 Flash Lite",
            "gemini-3.1-flash-lite",
        ),
    ];

    for (model_id, placeholder, display_name, expected) in cases {
        assert_eq!(model_from_id(model_id).as_deref(), Some(expected));
        assert_eq!(
            normalize_antigravity_model(placeholder).as_deref(),
            Some(expected)
        );
        assert_eq!(
            normalize_antigravity_model(display_name).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        normalize_antigravity_model("Gemini 3.8 Flash").as_deref(),
        Some("gemini-3.8-flash")
    );
    for display_name in ["gemini 3.8 flash(high)", "gemini-3.8-flash (high)"] {
        assert_eq!(
            normalize_antigravity_model(display_name).as_deref(),
            Some("gemini-3.8-flash"),
            "{display_name}"
        );
    }
}

#[test]
fn maps_claude_opus_5_5_id_and_placeholder_to_priced_model() {
    ccusage_test_support::install_pricing_snapshots();
    assert_eq!(model_from_id(1402).as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        normalize_antigravity_model("model_placeholder_m402").as_deref(),
        Some("claude-opus-5-5")
    );

    let pricing = ccusage_core::PricingMap::load_embedded();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };
    let cost = price_antigravity(
        "claude-opus-5-5",
        None,
        None,
        usage,
        TimestampMs::UNIX_EPOCH,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .0;
    assert!(cost > 0.0);
    assert!(
        price_antigravity(
            "claude-opus-5-5",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .1
        .is_none()
    );
}

#[test]
fn newer_gemini_effort_variants_fall_back_to_base_pricing() {
    assert_eq!(
        model_candidates("gemini-3.8-flash-high", None),
        vec![
            "gemini-3.8-flash-high".to_string(),
            "gemini-3.8-flash".to_string(),
        ]
    );
    assert_eq!(
        model_candidates("gemini-3.7-flash-medium", Some(API_PROVIDER_GOOGLE_GEMINI)),
        vec![
            "gemini-3.7-flash-medium".to_string(),
            "google/gemini-3.7-flash-medium".to_string(),
            "gemini/gemini-3.7-flash-medium".to_string(),
            "vertex_ai/gemini-3.7-flash-medium".to_string(),
            "openrouter/google/gemini-3.7-flash-medium".to_string(),
            "gemini-3.7-flash".to_string(),
            "google/gemini-3.7-flash".to_string(),
            "gemini/gemini-3.7-flash".to_string(),
            "vertex_ai/gemini-3.7-flash".to_string(),
            "openrouter/google/gemini-3.7-flash".to_string(),
        ]
    );
}

#[test]
fn provider_specific_variant_pricing_precedes_base_fallback() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "google/gemini-3.7-flash-medium": {
                        "input_cost_per_token": 2,
                        "output_cost_per_token": 2
                    },
                    "gemini-3.7-flash": {
                        "input_cost_per_token": 1,
                        "output_cost_per_token": 1
                    }
                }"#
        ),
        2
    );

    let cost = price_antigravity(
        "gemini-3.7-flash-medium",
        None,
        Some(API_PROVIDER_GOOGLE_GEMINI),
        TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            speed: None,
            cache_creation: None,
        },
        TimestampMs::UNIX_EPOCH,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .0;

    assert_eq!(cost, 2.0);
}

#[test]
fn a_gpt_thinking_id_prices_at_the_stripped_base_rate() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
            "gpt-oss-120b": {"input_cost_per_token": 1, "output_cost_per_token": 1}
        }"#,
    );

    let (cost, missing, _) = price_antigravity(
        "gpt-oss-120b-thinking",
        None,
        None,
        TokenUsageRaw {
            input_tokens: 1,
            ..TokenUsageRaw::default()
        },
        TimestampMs::UNIX_EPOCH,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    );

    assert_eq!(cost, 1.0);
    assert!(missing.is_none());
}

#[test]
fn a_row_for_the_thinking_id_precedes_the_stripped_fallback() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
            "gemini-2.5-flash": {"input_cost_per_token": 1, "output_cost_per_token": 1},
            "vertex_ai/gemini-2.5-flash-thinking": {"input_cost_per_token": 2, "output_cost_per_token": 2}
        }"#,
    );

    let cost = price_antigravity(
        "gemini-2.5-flash-thinking",
        None,
        None,
        TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            speed: None,
            cache_creation: None,
        },
        TimestampMs::UNIX_EPOCH,
        ccusage_core::cost::PriceTable::Calculate(&pricing),
    )
    .0;

    assert_eq!(cost, 2.0);
}

#[test]
fn sibling_effort_variant_does_not_satisfy_base_pricing_fallback() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "gemini-3.8-flash-high": {
                        "input_cost_per_token": 2,
                        "output_cost_per_token": 2
                    }
                }"#
        ),
        1
    );
    let usage = TokenUsageRaw {
        input_tokens: 1,
        output_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };

    assert_eq!(
        price_antigravity(
            "gemini-3.8-flash-medium",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .0,
        0.0
    );
    assert_eq!(
        price_antigravity(
            "gemini-3.8-flash-medium",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .1
        .as_deref(),
        Some("gemini-3.8-flash-medium")
    );
}

#[test]
fn effort_variant_honors_explicit_model_alias() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("gemini-3.6-flash-medium", "custom-flash-medium"),
        ("gemini-3.8-flash-low", "custom-unpriced-flash-low"),
    ]);
    let mut pricing = ccusage_core::PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "custom-flash-medium": {
                        "input_cost_per_token": 3,
                        "output_cost_per_token": 3
                    },
                    "gemini-3.6-flash": {
                        "input_cost_per_token": 1,
                        "output_cost_per_token": 1
                    }
                }"#
        ),
        2
    );
    let usage = TokenUsageRaw {
        input_tokens: 1,
        ..TokenUsageRaw::default()
    };

    assert_eq!(
        price_antigravity(
            "gemini-3.6-flash-medium",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .0,
        3.0
    );
    assert!(
        price_antigravity(
            "gemini-3.6-flash-medium",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .1
        .is_none()
    );
    assert_eq!(
        price_antigravity(
            "gemini-3.8-flash-low",
            None,
            None,
            usage,
            TimestampMs::UNIX_EPOCH,
            ccusage_core::cost::PriceTable::Calculate(&pricing)
        )
        .1
        .as_deref(),
        Some("custom-unpriced-flash-low")
    );
}

#[test]
fn legacy_models_keep_timestamp_aware_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = ccusage_core::PricingMap::load_embedded();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };
    let cost_at = |timestamp| {
        price_antigravity(
            "deepseek-v4-flash",
            None,
            None,
            usage,
            parse_ts_timestamp(timestamp).unwrap(),
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .0
    };

    assert!((cost_at("2026-08-17T01:00:00Z") - 0.44).abs() < 1e-12);
    assert!((cost_at("2026-08-17T12:00:00Z") - 0.22).abs() < 1e-12);
}

#[test]
fn a_vertex_request_bills_vertex_or_falls_back_to_googles_dated_rate() {
    ccusage_test_support::install_pricing_snapshots();
    let after_the_step = parse_ts_timestamp("2026-12-02T00:00:00Z").unwrap();
    let price = |rows: &str, provider| {
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(rows);
        price_antigravity(
            "gemini-4-argon",
            None,
            Some(provider),
            TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            after_the_step,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .0
    };
    let google = r#"{"gemini/gemini-4-argon": {"litellm_provider": "gemini", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5}}"#;
    let with_vertex = r#"{
        "gemini/gemini-4-argon": {"litellm_provider": "gemini", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5},
        "vertex_ai/gemini-4-argon": {"litellm_provider": "vertex_ai-language-models", "input_cost_per_token": 2.5e-6, "output_cost_per_token": 1.2e-5}
    }"#;

    assert!((price(with_vertex, API_PROVIDER_GOOGLE_VERTEX) - 2.5).abs() < 1e-9);
    assert!((price(google, API_PROVIDER_GOOGLE_VERTEX) - 4.0).abs() < 1e-9);
    assert!((price(google, API_PROVIDER_GOOGLE_GEMINI) - 4.0).abs() < 1e-9);
}
