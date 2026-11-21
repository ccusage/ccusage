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
            Some(200_000),
            "{model}"
        );
    }
    assert_eq!(
        PricingMap::load_embedded()
            .find("grok-4.5")
            .unwrap()
            .long_context_split_threshold(),
        200_000
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
        ("spacexai/grok-4-5", 200_001),
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
                        residual_tokens: 0,
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
                residual_tokens: 0,
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
            residual_tokens: 0,
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
