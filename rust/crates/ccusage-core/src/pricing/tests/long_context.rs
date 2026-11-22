use super::{MatchKind, Pricing, PricingMap, embedded_models_dev_pricing};

#[test]
fn long_context_view_names_the_existing_source_billing_modes() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "marginal": {"input_cost_per_token": 1, "output_cost_per_token": 2,
                "input_cost_per_token_above_200k_tokens": 3},
            "flat": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
    );
    let marginal = pricing.find("marginal").unwrap().long_context.unwrap();
    assert_eq!(marginal.mode, super::super::LongContextMode::Marginal);
    assert_eq!(marginal.threshold, 200_000);
    assert_eq!(marginal.rates.input, Some(3.0));
    assert!(pricing.find("flat").unwrap().long_context.is_none());

    pricing.load_models_dev_json_for_tests(
        r#"{
            "whole": {"cost": {"input": 1, "output": 2, "tiers": [{
                "input": 3, "tier": {"type": "context", "size": 272000}
            }]}}
        }"#,
    );
    let whole = pricing.find("whole").unwrap().long_context.unwrap();
    assert_eq!(whole.mode, super::super::LongContextMode::WholeRequest);
    assert_eq!(whole.threshold, 272_000);
    assert_eq!(whole.rates.input, Some(3e-6));
}

#[test]
fn embedded_pricing_fills_gpt_long_context_tier_rates() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let gpt_55 = pricing.find("gpt-5.5").unwrap();
    assert_eq!(
        gpt_55.long_context.and_then(|context| context.rates.input),
        Some(10e-6)
    );
    assert_eq!(
        gpt_55.long_context.and_then(|context| context.rates.output),
        Some(45e-6)
    );
    assert_eq!(
        gpt_55
            .long_context
            .and_then(|context| context.rates.cache_read),
        Some(1e-6)
    );
    assert_eq!(
        gpt_55.long_context.map(|context| context.threshold),
        Some(272_000)
    );

    let gpt_54 = pricing.find("gpt-5.4").unwrap();
    assert_eq!(
        gpt_54.long_context.and_then(|context| context.rates.input),
        Some(5e-6)
    );
    assert_eq!(
        gpt_54.long_context.and_then(|context| context.rates.output),
        Some(22.5e-6)
    );
    assert_eq!(
        gpt_54.long_context.map(|context| context.threshold),
        Some(272_000)
    );

    let mini = pricing.find("gpt-5.4-mini").unwrap();
    assert_eq!(
        mini.long_context.and_then(|context| context.rates.input),
        None
    );
    assert_eq!(mini.long_context.map(|context| context.threshold), None);
}

#[test]
fn long_context_overlay_survives_litellm_refresh_and_defers_to_upstream() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    pricing.load_json(
        r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031
                },
                "gpt-5.5-2026-04-23": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031
                }
            }"#,
    );
    pricing.fill_long_context_rates_from_models_dev(embedded_models_dev_pricing());

    let gpt_55 = pricing.find("gpt-5.5").unwrap();
    assert_eq!(gpt_55.input, 6e-6);
    assert_eq!(
        gpt_55.long_context.and_then(|context| context.rates.input),
        Some(10e-6)
    );
    assert_eq!(
        gpt_55.long_context.map(|context| context.threshold),
        Some(272_000)
    );
    let dated = pricing
        .resolve_at(None, &[("gpt-5.5-2026-04-23", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(
        dated.long_context.and_then(|context| context.rates.input),
        Some(10e-6)
    );

    pricing.load_json(
        r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031,
                    "input_cost_per_token_above_200k_tokens": 0.000012
                }
            }"#,
    );
    pricing.fill_long_context_rates_from_models_dev(embedded_models_dev_pricing());

    let gpt_55 = pricing.find("gpt-5.5").unwrap();
    assert_eq!(
        gpt_55.long_context.and_then(|context| context.rates.input),
        Some(12e-6)
    );
    assert_eq!(
        gpt_55.long_context.unwrap().mode,
        super::super::LongContextMode::Marginal
    );
    assert_eq!(gpt_55.long_context.unwrap().threshold, 200_000);
}

#[test]
fn embedded_models_dev_carries_grok_long_context_tiers() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    for model in ["grok-4.5", "grok-4.6"] {
        let entry = pricing.find(model).unwrap();
        assert!((entry.input * 1e6 - 2.0).abs() < 1e-9, "{model} base input");
        assert!(
            (entry
                .long_context
                .and_then(|context| context.rates.input)
                .unwrap()
                * 1e6
                - 4.0)
                .abs()
                < 1e-9,
            "{model} long-context input"
        );
        assert!(
            (entry
                .long_context
                .and_then(|context| context.rates.output)
                .unwrap()
                * 1e6
                - 12.0)
                .abs()
                < 1e-9,
            "{model} long-context output"
        );
        assert_eq!(
            entry.long_context.map(|context| context.threshold),
            Some(199_999),
            "{model}"
        );
    }
    assert_eq!(
        PricingMap::load_embedded()
            .find("grok-4.5")
            .unwrap()
            .long_context_split_threshold(),
        199_999
    );
}

#[test]
fn long_context_split_threshold_is_per_model() {
    crate::test_support::install_pricing_snapshots();
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.6-sol")
            .unwrap()
            .long_context_split_threshold(),
        272_000
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.5")
            .unwrap()
            .long_context_split_threshold(),
        272_000
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.5-pro")
            .unwrap()
            .long_context_split_threshold(),
        272_000
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.5-2026-04-23")
            .unwrap()
            .long_context_split_threshold(),
        272_000
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5")
            .unwrap()
            .long_context_split_threshold(),
        200_000
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.4-mini")
            .unwrap()
            .long_context_split_threshold(),
        200_000
    );
}

#[test]
fn split_threshold_comes_from_the_resolved_pricing_entry() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "private-boundary",
        "custom-boundary",
    )]);
    let mut map = PricingMap::default();
    map.insert_test_pricing(
        "custom-boundary".into(),
        Pricing {
            input: 1e-6,
            output: 2e-6,
            long_context: Some(super::super::LongContext {
                threshold: 123_456,
                mode: super::super::LongContextMode::WholeRequest,
                rates: crate::pricing::LongContextRates::default(),
            }),
            ..Pricing::empty()
        },
    );
    assert_eq!(
        map.find("private-boundary")
            .unwrap()
            .long_context_split_threshold(),
        123_456
    );
    assert_eq!(
        map.find("custom-boundary-high")
            .unwrap()
            .long_context_split_threshold(),
        123_456
    );
    let embedded = PricingMap::load_embedded();
    assert_eq!(
        embedded
            .find("gpt-5-4")
            .unwrap()
            .long_context_split_threshold(),
        272_000
    );
    assert_eq!(Pricing::empty().long_context_split_threshold(), 200_000);
}

#[test]
fn recorded_threshold_migration_groups_use_the_resolved_model_boundary() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, expected) in [
        ("GLM-5-1", 32_000),
        ("anthropic-claude-5-5-haiku", 100_000),
        ("spacexai/grok-4-5", 199_999),
        ("qwen-3-6-plus-high", 256_000),
        ("alibaba/qwen3-5-plus", 256_001),
        ("aihubmix/gpt-5-4-high", 272_000),
        ("minimax-m-3-0-high", 512_000),
    ] {
        assert_eq!(
            pricing.find(model).unwrap().long_context_split_threshold(),
            expected,
            "{model}"
        );
    }
}

#[test]
fn claude_long_context_tiers_bill_anthropics_published_rates_whichever_row_wins() {
    use super::super::{LongContext, LongContextMode, LongContextRates};
    crate::test_support::install_pricing_snapshots();
    let sonnet_4 = LongContext {
        threshold: 200_000,
        mode: LongContextMode::WholeRequest,
        rates: LongContextRates {
            input: Some(6e-6),
            output: Some(22.5e-6),
            cache_create: Some(7.5e-6),
            cache_create_1h: Some(12e-6),
            cache_read: Some(0.6e-6),
        },
    };
    let haiku_5_5 = LongContext {
        threshold: 100_000,
        mode: LongContextMode::WholeRequest,
        rates: LongContextRates {
            input: Some(0.5e-6),
            output: Some(2.5e-6),
            cache_create: Some(0.625e-6),
            cache_create_1h: Some(1e-6),
            cache_read: Some(0.05e-6),
        },
    };
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(super::build_time_pricing_json()));
        Ok(())
    });
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for (model, expected) in [
            ("claude-sonnet-4", sonnet_4),
            ("claude-sonnet-4-20250514", sonnet_4),
            ("anthropic.claude-sonnet-4-20250514-v1:0", sonnet_4),
            ("claude-sonnet-4.5", sonnet_4),
            ("claude-sonnet-4-5", sonnet_4),
            ("claude-sonnet-4-5-20250929", sonnet_4),
            ("claude-sonnet-4-5@20250929", sonnet_4),
            ("claude-sonnet-4-5-20250929-thinking", sonnet_4),
            ("claude-haiku-5-5", haiku_5_5),
        ] {
            let rates = pricing
                .resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(rates.long_context, Some(expected), "{mode} {model}");
        }
        let regional = pricing
            .find("us.anthropic.claude-sonnet-4-5-20250929-v1:0")
            .unwrap();
        assert_eq!(
            regional
                .long_context
                .and_then(|context| context.rates.input),
            Some(6.6e-6),
            "{mode}: a row off Anthropic's list price keeps its own tier"
        );
    }
}

#[test]
fn claude_4_6_and_later_bill_a_long_request_at_standard_rates_whichever_row_wins() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(super::build_time_pricing_json()));
        Ok(())
    });
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for model in [
            "claude-sonnet-4-6",
            "claude-sonnet-4-6-thinking",
            "anthropic-claude-4.6-sonnet",
            "anthropic/claude-sonnet-4.6",
            "claude-opus-4-6",
            "anthropic-claude-opus-4.6",
            "anthropic/claude-opus-4.6",
            "anthropic/claude-opus-4.7",
        ] {
            let rates = pricing
                .resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(rates.long_context, None, "{mode} {model}");
        }
    }
}

#[test]
fn claude_4_6_long_requests_paid_the_premium_until_the_1m_window_left_beta() {
    use super::super::{LongContext, LongContextMode, LongContextRates};
    crate::test_support::install_pricing_snapshots();
    let tier = |[input, output, cache_create, cache_create_1h, cache_read]: [f64; 5]| LongContext {
        threshold: 200_000,
        mode: LongContextMode::WholeRequest,
        rates: LongContextRates {
            input: Some(input),
            output: Some(output),
            cache_create: Some(cache_create),
            cache_create_1h: Some(cache_create_1h),
            cache_read: Some(cache_read),
        },
    };
    let sonnet = tier([6e-6, 22.5e-6, 7.5e-6, 12e-6, 0.6e-6]);
    let opus = tier([10e-6, 37.5e-6, 12.5e-6, 20e-6, 1e-6]);
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(super::build_time_pricing_json()));
        Ok(())
    });
    let before = super::timestamp("2026-03-12T23:59:59Z");
    let after = super::timestamp("2026-03-13T00:00:00Z");
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for (model, premium, [cost_before, cost_after]) in [
            ("claude-sonnet-4-6", sonnet, [1.8225, 0.915]),
            ("claude-sonnet-4.6", sonnet, [1.8225, 0.915]),
            ("claude-sonnet-4-6-thinking", sonnet, [1.8225, 0.915]),
            ("anthropic-claude-4.6-sonnet", sonnet, [1.8225, 0.915]),
            ("anthropic/claude-sonnet-4.6", sonnet, [1.8225, 0.915]),
            ("claude-opus-4-6", opus, [3.0375, 1.525]),
            ("anthropic-claude-opus-4.6", opus, [3.0375, 1.525]),
            ("anthropic/claude-opus-4.6", opus, [3.0375, 1.525]),
        ] {
            for (instant, expected) in [(before, cost_before), (after, cost_after)] {
                let candidates = [(model, MatchKind::Fuzzy)];
                let cost = crate::cost::price_entry(
                    crate::cost::EntryPricing {
                        granularity: crate::cost::UsageGranularity::Request,
                        usage: crate::TokenUsageRaw {
                            input_tokens: 300_000,
                            output_tokens: 1_000,
                            ..Default::default()
                        },
                        reasoning_tokens: 0,
                        reported_model: Some(model),
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: Some(instant),
                    },
                    crate::cost::PriceTable::Calculate(pricing),
                )
                .cost;
                assert!(
                    (cost - expected).abs() < 1e-9,
                    "{mode} {model}: {cost} != {expected}"
                );
            }
            let early = pricing.find_at(model, before).unwrap();
            assert_eq!(early.long_context, Some(premium), "{mode} {model} before");
            let late = pricing.find_at(model, after).unwrap();
            let long_input = late
                .long_context
                .and_then(|context| context.rates.input)
                .unwrap_or(late.input);
            assert_eq!(long_input, late.input, "{mode} {model} after");
        }
    }
}

#[test]
fn fast_mode_claude_opus_4_6_bills_no_1m_beta_premium_before_ga() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let model = "claude-opus-4-6";
    let candidates = [(model, MatchKind::Fuzzy)];
    for (speed, expected) in [
        (None, 300_000.0 * 10e-6 + 1_000.0 * 37.5e-6),
        (
            Some(crate::Speed::Fast),
            (300_000.0 * 5e-6 + 1_000.0 * 25e-6) * 6.0,
        ),
    ] {
        let cost = crate::cost::price_entry(
            crate::cost::EntryPricing {
                granularity: crate::cost::UsageGranularity::Request,
                usage: crate::TokenUsageRaw {
                    input_tokens: 300_000,
                    output_tokens: 1_000,
                    speed,
                    ..Default::default()
                },
                reasoning_tokens: 0,
                reported_model: Some(model),
                candidates: &candidates,
                stored_cost: None,
                timestamp: Some(super::timestamp("2026-03-12T12:00:00Z")),
            },
            crate::cost::PriceTable::Calculate(&pricing),
        )
        .cost;
        assert!(
            (cost - expected).abs() < 1e-9,
            "{speed:?}: {cost} != {expected}"
        );
    }
}

#[test]
fn a_litellm_gemini_request_above_200k_uses_the_published_tier_for_all_tokens() {
    crate::test_support::install_pricing_snapshots();
    let model = "gemini-test-model";
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gemini-test-model": {
                "litellm_provider": "gemini",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6,
                "input_cost_per_token_above_200k_tokens": 3e-6,
                "output_cost_per_token_above_200k_tokens": 5e-6
            }
        }"#,
    );

    for (input_tokens, expected) in [(199_999, 0.201999), (300_000, 0.905)] {
        let cost = request_cost(&pricing, model, input_tokens, None, None);
        assert!(
            (cost - expected).abs() < 1e-9,
            "{input_tokens} input tokens: {cost} != {expected}"
        );
    }
}

#[test]
fn litellm_long_context_mode_follows_the_model_on_any_host() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let cases = [
        (
            "router/anthropic/claude-sonnet-4-5",
            Some("router"),
            200_000,
            0.905,
        ),
        (
            "vertex_ai/gemini-3-pro-preview",
            Some("vertex_ai"),
            200_000,
            0.905,
        ),
        (
            "databricks/databricks-gemini-3-1-pro",
            Some("databricks"),
            200_000,
            0.905,
        ),
        ("router/grok-4-fast", None, 199_999, 0.905),
        ("gmi/xai/grok4.7", Some("gmi"), 199_999, 0.905),
        ("router/gpt-5", Some("gemini"), 200_000, 0.502),
    ];
    let mut rows = serde_json::Map::new();
    for (model, provider, _, _) in cases {
        rows.insert(
            model.to_owned(),
            serde_json::json!({
                "litellm_provider": provider,
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6,
                "input_cost_per_token_above_200k_tokens": 3e-6,
                "output_cost_per_token_above_200k_tokens": 5e-6
            }),
        );
    }
    assert_eq!(
        pricing.load_json(&serde_json::Value::Object(rows).to_string()),
        cases.len()
    );
    for (model, _, threshold, expected) in cases {
        let rates = pricing.find(model).unwrap();
        assert_eq!(rates.long_context_split_threshold(), threshold, "{model}");
        let cost = request_cost(&pricing, model, 300_000, None, None);
        assert!(
            (cost - expected).abs() < 1e-9,
            "{model}: {cost} != {expected}"
        );
    }
}

#[test]
fn regional_bedrock_sonnet_bills_the_whole_request_at_its_own_rates() {
    use crate::cost::{EntryPricing, PriceTable, UsageGranularity, price_entry};

    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (region, expected) in [
        ("us", 0.38115),
        ("eu", 0.38115),
        ("au", 0.38115),
        ("jp", 0.38115),
        ("us-gov", 0.4158),
    ] {
        let model = format!("{region}.anthropic.claude-sonnet-4-5-20250929-v1:0");
        let cost = price_entry(
            EntryPricing {
                usage: crate::TokenUsageRaw {
                    input_tokens: 10_000,
                    cache_read_input_tokens: 290_000,
                    output_tokens: 5_000,
                    ..Default::default()
                },
                granularity: UsageGranularity::Request,
                reasoning_tokens: 0,
                reported_model: Some(&model),
                candidates: &[(&model, MatchKind::StrictExact)],
                stored_cost: None,
                timestamp: None,
            },
            PriceTable::Calculate(&pricing),
        )
        .cost;
        assert!(
            (cost - expected).abs() < 1e-9,
            "{model}: {cost} != {expected}"
        );
    }
}

#[test]
fn an_override_tier_uses_its_catalogue_vendors_billing_mode_and_threshold() {
    crate::test_support::install_pricing_snapshots();
    let mut catalogue = PricingMap::default();
    catalogue.load_json(
        r#"{
            "proxy/gemini-flat": {"litellm_provider": "proxy", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "flat-marginal": {"litellm_provider": "deepseek", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "flat-openai": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "proxy/gemini-tiered": {"litellm_provider": "proxy", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6, "input_cost_per_token_above_200k_tokens": 4e-6},
            "proxy/grok-tiered": {"litellm_provider": "proxy", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6, "input_cost_per_token_above_200k_tokens": 4e-6},
            "tiered-marginal": {"litellm_provider": "deepseek", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6, "input_cost_per_token_above_200k_tokens": 4e-6}
        }"#,
    );
    let mut secondary = PricingMap::default();
    secondary.load_models_dev_json_for_tests(
        r#"{
            "gemini-secondary-flat": {"provider": "proxy", "cost": {"input": 1, "output": 2}},
            "gemini-secondary-tiered": {"provider": "proxy", "cost": {"input": 1, "output": 2,
                "tiers": [{"input": 4, "tier": {"type": "context", "size": 128000}}]}},
            "secondary-published": {"provider": "minimax", "cost": {"input": 1, "output": 2,
                "tiers": [{"input": 4, "tier": {"type": "context", "size": 128000}}]}}
        }"#,
    );
    let catalogue = super::layered(&catalogue, Some(&secondary));
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token_above_200k_tokens: Some(3e-6),
        output_cost_per_token_above_200k_tokens: Some(5e-6),
        ..Default::default()
    };

    for (model, threshold, expected) in [
        ("proxy/gemini-flat", 200_000, [0.202, 0.605003]),
        ("flat-marginal", 200_000, [0.202, 0.202003]),
        ("flat-openai", 200_000, [0.202, 0.202003]),
        ("proxy/gemini-tiered", 200_000, [0.202, 0.605003]),
        ("proxy/grok-tiered", 199_999, [0.201999, 0.605]),
        ("tiered-marginal", 200_000, [0.202, 0.202003]),
        ("gemini-secondary-flat", 200_000, [0.202, 0.605003]),
        ("gemini-secondary-tiered", 128_000, [0.13, 0.389003]),
        ("secondary-published", 128_000, [0.13, 0.389003]),
    ] {
        let key = model.to_owned();
        let pricing = catalogue.with_user_overrides([(&key, &override_value)]);
        for (input_tokens, expected) in [threshold, threshold + 1].into_iter().zip(expected) {
            let cost = request_cost(&pricing, model, input_tokens, None, None);
            assert!(
                (cost - expected).abs() < 1e-9,
                "{model}, {input_tokens} input tokens: {cost} != {expected}"
            );
        }
    }
}

#[test]
fn override_tier_metadata_follows_normalized_model_ids_and_uses_the_default_cutoff() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"custom-gemini-v1": {"litellm_provider": "gemini",
            "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6}}"#,
    );
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(1e-6),
        output_cost_per_token: Some(2e-6),
        input_cost_per_token_above_200k_tokens: Some(3e-6),
        output_cost_per_token_above_200k_tokens: Some(5e-6),
        ..Default::default()
    };

    for model in [
        "[agent] custom.gemini.v1",
        "xai/custom-untiered",
        "anthropic-claude-sonnet-private",
        "anthropic@claude-sonnet-private",
        "databricks-claude-sonnet-4-6",
        "anthropic--claude-4.6-sonnet",
        "databricks/databricks-grok-4-6",
    ] {
        let key = model.to_owned();
        let pricing = pricing.with_user_overrides([(&key, &override_value)]);
        for (input_tokens, expected) in [(200_000, 0.202), (300_000, 0.905)] {
            let cost = request_cost(&pricing, model, input_tokens, None, None);
            assert!(
                (cost - expected).abs() < 1e-9,
                "{model}, {input_tokens} input tokens: {cost} != {expected}"
            );
        }
    }
}

#[test]
fn fast_opus_uses_whole_request_override_rates_after_the_1m_window_left_beta() {
    crate::test_support::install_pricing_snapshots();
    let model = "claude-opus-4-6".to_owned();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token_above_200k_tokens: Some(8e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_embedded().with_user_overrides([(&model, &override_value)]);

    for (input_tokens, expected) in [(199_999, 6.14997), (200_000, 6.15), (300_000, 14.55)] {
        let cost = request_cost(
            &pricing,
            &model,
            input_tokens,
            Some(crate::Speed::Fast),
            Some(super::timestamp("2026-03-13T00:00:00Z")),
        );
        assert!(
            (cost - expected).abs() < 1e-9,
            "{input_tokens} input tokens: {cost} != {expected}"
        );
    }
}

fn request_cost(
    pricing: &PricingMap,
    model: &str,
    input_tokens: u64,
    speed: Option<crate::Speed>,
    timestamp: Option<crate::TimestampMs>,
) -> f64 {
    crate::cost::price_entry(
        crate::cost::EntryPricing {
            granularity: crate::cost::UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens,
                output_tokens: 1_000,
                speed,
                ..Default::default()
            },
            reasoning_tokens: 0,
            reported_model: Some(model),
            candidates: &[(model, MatchKind::Fuzzy)],
            stored_cost: None,
            timestamp,
        },
        crate::cost::PriceTable::Calculate(pricing),
    )
    .cost
}

fn long_request_cost_with_override(model: &str, input: f64, output: f64, instant: &str) -> f64 {
    crate::test_support::install_pricing_snapshots();
    let key = model.to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(input),
        output_cost_per_token: Some(output),
        ..Default::default()
    };
    let pricing = PricingMap::load_embedded().with_user_overrides([(&key, &override_value)]);
    let candidates = [(model, MatchKind::Fuzzy)];
    crate::cost::price_entry(
        crate::cost::EntryPricing {
            granularity: crate::cost::UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens: 300_000,
                output_tokens: 1_000,
                ..Default::default()
            },
            reasoning_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(super::timestamp(instant)),
        },
        crate::cost::PriceTable::Calculate(&pricing),
    )
    .cost
}

#[test]
fn a_base_only_override_bills_no_scheduled_tier_on_either_side_of_its_date() {
    let expected = 300_000.0 * 4e-6 + 1_000.0 * 20e-6;
    for instant in ["2026-03-12T00:00:00Z", "2026-03-13T00:00:00Z"] {
        let cost = long_request_cost_with_override("claude-sonnet-4-6", 4e-6, 20e-6, instant);
        assert!(
            (cost - expected).abs() < 1e-9,
            "{instant}: {cost} != {expected}"
        );
    }
    let expected = 300_000.0 * 1e-6 + 1_000.0 * 2e-6;
    for instant in ["2026-08-01T00:00:00Z", "2026-11-17T03:00:00Z"] {
        let cost = long_request_cost_with_override("deepseek-v4-pro", 1e-6, 2e-6, instant);
        assert!(
            (cost - expected).abs() < 1e-9,
            "deepseek {instant}: {cost} != {expected}"
        );
    }
}

#[test]
fn anthropic_billed_claude_4_6_usage_keeps_the_premium_before_ga() {
    use super::BillingRoute;
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in ["claude-sonnet-4-6", "anthropic/claude-sonnet-4.6"] {
        let candidates = [(model, MatchKind::Fuzzy)];
        for (route, instant, premium) in [
            (
                BillingRoute::Direct("anthropic"),
                "2026-03-12T23:59:59Z",
                Some(6e-6),
            ),
            (
                BillingRoute::Direct("anthropic"),
                "2026-03-13T00:00:00Z",
                None,
            ),
            (
                BillingRoute::Direct("deepseek"),
                "2026-03-12T23:59:59Z",
                None,
            ),
            (BillingRoute::Foreign, "2026-03-12T23:59:59Z", None),
        ] {
            let (_, rates) = pricing
                .resolve_on_route_at(
                    Some(model),
                    &candidates,
                    route,
                    Some(super::timestamp(instant)),
                )
                .unwrap();
            let long_input = rates
                .long_context
                .and_then(|context| context.rates.input)
                .filter(|rate| (*rate - rates.input).abs() > f64::EPSILON);
            assert_eq!(long_input, premium, "{model} {route:?} {instant}");
        }
    }
}

#[test]
fn a_multiplier_only_override_keeps_the_claude_4_6_premium_tier() {
    crate::test_support::install_pricing_snapshots();
    let key = "claude-sonnet-4-6".to_string();
    let override_value = ccusage_cli::PricingOverride {
        fast_multiplier: Some(2.0),
        ..Default::default()
    };
    let overridden = PricingMap::load_embedded().with_user_overrides([(&key, &override_value)]);
    let plain = PricingMap::load_embedded();
    let instant = super::timestamp("2026-03-12T00:00:00Z");
    let scheduled = plain.find_at("claude-sonnet-4-6", instant).unwrap();
    let rates = overridden.find_at("claude-sonnet-4-6", instant).unwrap();
    assert_eq!(rates.long_context, scheduled.long_context);
    assert_eq!(
        rates
            .long_context
            .and_then(|context| context.rates.cache_create_1h),
        Some(12e-6)
    );
    assert_eq!(rates.fast_multiplier, 2.0);
}

#[test]
fn a_long_rate_only_override_keeps_the_scheduled_one_hour_write_rate() {
    crate::test_support::install_pricing_snapshots();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token_above_200k_tokens: Some(7e-6),
        ..Default::default()
    };
    for (model, instant, one_hour) in [
        ("claude-sonnet-4-6", "2026-03-12T00:00:00Z", Some(12e-6)),
        ("grok-code-fast-1", "2026-05-16T00:00:00Z", None),
    ] {
        let key = model.to_string();
        let pricing = PricingMap::load_embedded().with_user_overrides([(&key, &override_value)]);
        let context = pricing
            .find_at(model, super::timestamp(instant))
            .unwrap()
            .long_context
            .unwrap();
        assert_eq!(context.rates.input, Some(7e-6), "{model}");
        assert_eq!(context.rates.cache_create_1h, one_hour, "{model}");
    }
}

#[test]
fn fast_mode_keeps_an_overrides_explicit_long_rates_before_ga() {
    crate::test_support::install_pricing_snapshots();
    let model = "claude-opus-4-6";
    let key = model.to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token_above_200k_tokens: Some(8e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_embedded().with_user_overrides([(&key, &override_value)]);
    let candidates = [(model, MatchKind::Fuzzy)];
    let cost = crate::cost::price_entry(
        crate::cost::EntryPricing {
            granularity: crate::cost::UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens: 300_000,
                output_tokens: 1_000,
                cache_creation_input_tokens: 10_000,
                cache_creation: Some(crate::CacheCreationRaw {
                    ephemeral_5m_input_tokens: 0,
                    ephemeral_1h_input_tokens: 10_000,
                }),
                speed: Some(crate::Speed::Fast),
                ..Default::default()
            },
            reasoning_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(super::timestamp("2026-03-12T12:00:00Z")),
        },
        crate::cost::PriceTable::Calculate(&pricing),
    )
    .cost;
    let fast_rates = pricing
        .find_at(model, super::timestamp("2026-03-12T12:00:00Z"))
        .unwrap()
        .at_speed(true);
    assert_eq!(
        fast_rates.one_hour_cache_creation_rate_above_200k(),
        Some(10e-6)
    );
    let expected = (300_000.0 * 8e-6 + 1_000.0 * 25e-6 + 10_000.0 * 10e-6) * 6.0;
    assert!((cost - expected).abs() < 1e-9, "{cost} != {expected}");
}
