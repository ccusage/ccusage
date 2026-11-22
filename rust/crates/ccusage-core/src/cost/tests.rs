fn calculate_cost_for_usage_at(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    cost_usd: Option<f64>,
    timestamp: Option<crate::TimestampMs>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    match mode {
        CostMode::Display => cost_usd.unwrap_or(0.0),
        CostMode::Auto => {
            cost_usd.unwrap_or_else(|| calculate_cost_from_tokens(model, usage, timestamp, pricing))
        }
        CostMode::Calculate => calculate_cost_from_tokens(model, usage, timestamp, pricing),
    }
}

fn calculate_cost_from_tokens(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    timestamp: Option<crate::TimestampMs>,
    pricing: Option<&PricingMap>,
) -> f64 {
    let Some(model) = model else {
        return 0.0;
    };
    let Some(pricing) = pricing.and_then(|pricing| {
        timestamp.map_or_else(
            || pricing.find(model),
            |timestamp| pricing.find_at(model, timestamp),
        )
    }) else {
        return 0.0;
    };
    cost_at_speed(usage, &pricing)
}

fn calculate_cost_for_usage(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    cost_usd: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    calculate_cost_for_usage_at(model, usage, cost_usd, None, mode, pricing)
}

use super::*;

#[test]
fn candidate_entry_pricing_bills_reasoning_and_ignores_residual_tokens() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    map.load_json(
        r#"{"priced": {
            "input_cost_per_token": 2, "output_cost_per_token": 3
        }}"#,
    );
    let candidates = [("priced", crate::pricing::MatchKind::Exact)];
    let usage = crate::TokenUsageRaw {
        input_tokens: 2,
        output_tokens: 3,
        ..Default::default()
    };
    let additional = crate::AdditionalTokens::from_total(usage, 4, 14);
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage,
            reasoning_tokens: additional.reasoning,
            reported_model: None,
            candidates: &candidates,
            stored_cost: Some(99.0),
            timestamp: None,
        },
        PriceTable::Calculate(&map),
    );
    assert_eq!(priced.cost, 25.0);
    assert_eq!(priced.matched, Some("priced"));
    assert!(!priced.missing_pricing);
}

#[test]
fn candidate_entry_modes_keep_stored_costs_separate_from_lookup_results() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    map.load_json(r#"{"free": {"litellm_provider": "openai", "input_cost_per_token": 0, "output_cost_per_token": 0}}"#);
    let candidates = [("free", crate::pricing::MatchKind::Exact)];
    for (mode, stored_cost, expected, matched) in [
        (CostMode::Display, Some(7.0), 7.0, None),
        (CostMode::Display, None, 0.0, None),
        (CostMode::Auto, Some(0.0), 0.0, None),
        (CostMode::Auto, Some(7.0), 7.0, None),
        (CostMode::Auto, None, 0.0, Some("free")),
        (CostMode::Calculate, Some(7.0), 0.0, Some("free")),
    ] {
        let result = price_entry(
            EntryPricing {
                granularity: UsageGranularity::Request,
                usage: crate::TokenUsageRaw {
                    input_tokens: 100,
                    ..Default::default()
                },
                reasoning_tokens: 0,
                reported_model: None,
                candidates: &candidates,
                stored_cost,
                timestamp: None,
            },
            PriceTable::for_map(mode, &map),
        );
        assert_eq!(result.cost, expected);
        assert_eq!(result.matched, matched);
        assert!(!result.missing_pricing);
    }
}

#[test]
fn candidate_entry_missing_pricing_requires_billable_usage_and_a_lookup() {
    let map = PricingMap::default();
    let candidates = [("missing", crate::pricing::MatchKind::Fuzzy)];
    for (input_tokens, reasoning_tokens, mode, stored_cost, missing) in [
        (0, 0, CostMode::Calculate, None, false),
        (1, 0, CostMode::Calculate, None, true),
        (0, 1, CostMode::Calculate, None, true),
        (1, 0, CostMode::Display, None, false),
        (1, 0, CostMode::Auto, Some(0.0), false),
    ] {
        let result = price_entry(
            EntryPricing {
                granularity: UsageGranularity::Request,
                usage: crate::TokenUsageRaw {
                    input_tokens,
                    ..Default::default()
                },
                reasoning_tokens,
                reported_model: None,
                candidates: &candidates,
                stored_cost,
                timestamp: None,
            },
            PriceTable::for_map(mode, &map),
        );
        assert_eq!(result.cost, 0.0);
        assert_eq!(result.matched, None);
        assert_eq!(result.missing_pricing, missing);
    }
}

#[test]
fn candidate_entry_pricing_uses_the_selected_schedule_and_fast_multiplier() {
    let mut map = deepseek_pricing();
    let model = "deepseek-v4-flash".to_string();
    let value = ccusage_cli::PricingOverride {
        fast_multiplier: Some(3.0),
        ..Default::default()
    };
    map = map.with_user_overrides([(&model, &value)]);
    let candidates = [("deepseek.v4.flash", crate::pricing::MatchKind::Exact)];
    let result = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens: 1_000_000,
                speed: Some(Speed::Fast),
                ..Default::default()
            },
            reasoning_tokens: 0,
            reported_model: None,
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(timestamp("2026-08-17T01:00:00Z")),
        },
        PriceTable::Calculate(&map),
    );
    assert!((result.cost - 1.32).abs() < 1e-12);
    assert_eq!(result.matched, Some("deepseek.v4.flash"));
    assert!(!result.missing_pricing);
}

#[test]
fn an_input_override_keeps_the_published_base_cache_rate_and_drops_the_tier() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"claude-test": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 4,
            "output_cost_per_token": 8,
            "cache_creation_input_token_cost": 7,
            "cache_creation_input_token_cost_above_200k_tokens": 9,
            "cache_read_input_token_cost_above_200k_tokens": 3
        }}"#,
    );
    let overrides = std::collections::BTreeMap::from([(
        "claude-test".to_string(),
        ccusage_cli::PricingOverride {
            input_cost_per_token: Some(2.0),
            ..Default::default()
        },
    )]);
    let pricing = pricing.with_user_overrides(&overrides);
    let entry = pricing.find("claude-test").unwrap();
    let usage = crate::TokenUsageRaw {
        cache_creation_input_tokens: 200_001,
        cache_read_input_tokens: 200_001,
        ..Default::default()
    };
    assert_eq!(calculate_cost_from_pricing(usage, &entry), 1_440_007.2);
}

#[test]
fn published_one_hour_cache_rate_prices_the_duration_bucket() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"claude-test": {
            "input_cost_per_token": 4,
            "output_cost_per_token": 8,
            "cache_creation_input_token_cost_above_1hr": 11
        }}"#,
    );
    let usage = crate::TokenUsageRaw {
        cache_creation: Some(crate::CacheCreationRaw {
            ephemeral_5m_input_tokens: 0,
            ephemeral_1h_input_tokens: 10,
        }),
        ..Default::default()
    };
    assert_eq!(
        calculate_cost_from_pricing(usage, &pricing.find("claude-test").unwrap()),
        110.0
    );
}

#[test]
fn a_flat_one_hour_rate_keeps_the_long_context_premium_on_the_tier() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "tier-published": {
                "input_cost_per_token": 3,
                "output_cost_per_token": 15,
                "input_cost_per_token_above_200k_tokens": 6,
                "cache_creation_input_token_cost_above_1hr": 6,
                "cache_creation_input_token_cost_above_1hr_above_200k_tokens": 13
            },
            "tier-derived": {
                "input_cost_per_token": 3,
                "output_cost_per_token": 15,
                "input_cost_per_token_above_200k_tokens": 6,
                "cache_creation_input_token_cost_above_1hr": 6
            }
        }"#,
    );
    let usage = crate::TokenUsageRaw {
        cache_creation: Some(crate::CacheCreationRaw {
            ephemeral_5m_input_tokens: 0,
            ephemeral_1h_input_tokens: 300_000,
        }),
        ..Default::default()
    };
    assert_eq!(
        calculate_cost_from_pricing(usage, &pricing.find("tier-published").unwrap()),
        200_000.0 * 6.0 + 100_000.0 * 13.0
    );
    assert_eq!(
        calculate_cost_from_pricing(usage, &pricing.find("tier-derived").unwrap()),
        200_000.0 * 6.0 + 100_000.0 * 12.0
    );
}

#[test]
fn absent_cache_writes_follow_family_and_reads_preserve_their_fallback() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gpt-test": {"input_cost_per_token": 2, "output_cost_per_token": 8, "litellm_provider": "openai"},
            "claude-test": {"input_cost_per_token": 2, "output_cost_per_token": 8, "litellm_provider": "anthropic"},
            "other-test": {"input_cost_per_token": 2, "output_cost_per_token": 8}
        }"#,
    );
    let usage = crate::TokenUsageRaw {
        cache_creation: Some(crate::CacheCreationRaw {
            ephemeral_5m_input_tokens: 10,
            ephemeral_1h_input_tokens: 1,
        }),
        cache_read_input_tokens: 100,
        ..Default::default()
    };
    let cost = |model| calculate_cost_from_pricing(usage, &pricing.find(model).unwrap());
    assert_eq!(cost("gpt-test"), 10.0 * 2.0 + 2.0 + 100.0 * 0.2);
    assert_eq!(cost("claude-test"), 10.0 * 2.5 + 4.0 + 100.0 * 0.2);
    assert_eq!(cost("other-test"), cost("claude-test"));
}

#[test]
fn an_empty_cost_sum_is_positive_zero() {
    let total = sum_costs(std::iter::empty());
    assert_eq!(total.to_bits(), 0.0_f64.to_bits());
    assert_eq!(crate::format_currency(total), "$0.00");
    assert_eq!(sum_costs([0.25, 0.5]), 0.75);
}

#[test]
fn stored_costs_suppress_missing_pricing_detection() {
    let pricing = PricingMap::default();
    let usage = crate::TokenUsageRaw {
        input_tokens: 100,
        output_tokens: 10,
        ..crate::TokenUsageRaw::default()
    };

    for (table, stored_cost, missing) in [
        (PriceTable::Auto(&pricing), None, true),
        (PriceTable::Auto(&pricing), Some(1.5), false),
        (PriceTable::Display, None, false),
        (PriceTable::Calculate(&pricing), None, true),
    ] {
        let price = price_entry(
            EntryPricing {
                granularity: UsageGranularity::Request,
                usage,
                reasoning_tokens: 0,
                reported_model: Some("unknown-model"),
                candidates: &[("unknown-model", crate::pricing::MatchKind::Fuzzy)],
                stored_cost,
                timestamp: None,
            },
            table,
        );
        assert_eq!(price.missing_pricing, missing);
    }
}

use crate::{
    cli::CostMode,
    pricing::PricingMap,
    types::{CacheCreationRaw, TokenUsageRaw},
};

fn pricing() -> PricingMap {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "test-model": {
                    "input_cost_per_token": 1.0,
                    "output_cost_per_token": 10.0,
                    "cache_creation_input_token_cost": 1.25,
                    "cache_read_input_token_cost": 0.1,
                    "input_cost_per_token_above_200k_tokens": 2.0,
                    "cache_creation_input_token_cost_above_200k_tokens": 1.5
                }
            }"#,
    );
    pricing
}

#[test]
fn prices_cache_creation_breakdown_by_duration() {
    let usage = TokenUsageRaw {
        cache_creation_input_tokens: 999,
        cache_read_input_tokens: 30,
        cache_creation: Some(CacheCreationRaw {
            ephemeral_5m_input_tokens: 10,
            ephemeral_1h_input_tokens: 20,
        }),
        ..TokenUsageRaw::default()
    };

    let cost = calculate_cost_for_usage(
        Some("test-model"),
        usage,
        None,
        CostMode::Calculate,
        Some(&pricing()),
    );

    assert!((cost - 55.5).abs() < f64::EPSILON);
}

#[test]
fn falls_back_to_flat_cache_creation_rate_without_breakdown() {
    let usage = TokenUsageRaw {
        cache_creation_input_tokens: 10,
        ..TokenUsageRaw::default()
    };

    let cost = calculate_cost_for_usage(
        Some("test-model"),
        usage,
        None,
        CostMode::Calculate,
        Some(&pricing()),
    );

    assert!((cost - 12.5).abs() < f64::EPSILON);
}

#[test]
fn prices_combined_cache_creation_durations() {
    let usage = TokenUsageRaw {
        cache_creation_input_tokens: 30,
        cache_read_input_tokens: 30,
        cache_creation: Some(CacheCreationRaw::from_splits(10, 20)),
        ..TokenUsageRaw::default()
    };

    let cost = calculate_cost_for_usage(
        Some("test-model"),
        usage,
        None,
        CostMode::Calculate,
        Some(&pricing()),
    );

    assert!((cost - 55.5).abs() < f64::EPSILON);
}

#[test]
fn prices_two_stage_model_as_whole_request_at_long_context_rates() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    assert_eq!(
        pricing.load_models_dev_json_for_tests(
            r#"{
                    "two-stage-test": {
                        "cost": {
                            "input": 1,
                            "output": 10,
                            "cache_read": 0.1,
                            "tiers": [{
                                "input": 2,
                                "output": 20,
                                "cache_read": 0.2,
                                "tier": { "type": "context", "size": 100 }
                            }]
                        }
                    }
                }"#,
        ),
        Some(1)
    );

    let cache_read_tokens = 40_u32;
    let output_tokens = 3_u32;
    let base_rates = (1.0, 10.0, 0.1);
    let long_context_rates = (2.0, 20.0, 0.2);
    let cases = [
        ("below the boundary", 59_u32, base_rates),
        ("at the boundary", 60_u32, base_rates),
        ("above the boundary", 61_u32, long_context_rates),
    ];
    for (case, input_tokens, (input_rate, output_rate, cache_read_rate)) in cases {
        let usage = TokenUsageRaw {
            input_tokens: u64::from(input_tokens),
            output_tokens: u64::from(output_tokens),
            cache_read_input_tokens: u64::from(cache_read_tokens),
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("two-stage-test"),
            usage,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        let expected = (f64::from(input_tokens) * input_rate
            + f64::from(output_tokens) * output_rate
            + f64::from(cache_read_tokens) * cache_read_rate)
            / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-12, "{case}: {cost}");
    }
}

#[test]
fn derived_whole_request_cache_rates_are_floored_at_the_base_rates() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"claude-sonnet-5-5-example": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 1e-6,
            "output_cost_per_token": 5e-6,
            "input_cost_per_token_above_200k_tokens": 1.5e-6,
            "cache_read_input_token_cost": 0.1e-6,
            "cache_creation_input_token_cost": 2e-6,
            "cache_creation_input_token_cost_above_1hr": 4e-6
        }}"#,
    );
    let rates = pricing.find("claude-sonnet-5-5-example").unwrap();
    let usage = crate::TokenUsageRaw {
        cache_read_input_tokens: 250_000,
        cache_creation: Some(crate::CacheCreationRaw {
            ephemeral_5m_input_tokens: 25_000,
            ephemeral_1h_input_tokens: 25_000,
        }),
        ..Default::default()
    };

    let cost = calculate_cost_from_pricing(usage, &rates);
    assert!((cost - 0.175).abs() < 1e-12, "{cost}");
}

#[test]
fn gemini_pro_previews_bill_the_same_long_context_cache_reads() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let usage = TokenUsageRaw {
        input_tokens: 10_000,
        output_tokens: 5_000,
        cache_read_input_tokens: 290_000,
        ..TokenUsageRaw::default()
    };

    for model in ["gemini-3-pro-preview", "gemini-3.1-pro-preview"] {
        let cost = calculate_cost_for_usage(
            Some(model),
            usage,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!((cost - 0.246).abs() < 1e-12, "{model}: {cost}");
    }
}

#[test]
fn cached_context_selects_the_long_context_tier() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let uncached_input_tokens = 10_000_u32;
    let output_tokens = 1_000_u32;
    let tiers = [
        ("long context", 500_000_u32, (4.0, 12.0, 0.6)),
        ("short context", 100_000_u32, (2.0, 6.0, 0.3)),
    ];
    for (tier, cache_read_tokens, (input_rate, output_rate, cache_read_rate)) in tiers {
        let usage = TokenUsageRaw {
            input_tokens: u64::from(uncached_input_tokens),
            output_tokens: u64::from(output_tokens),
            cache_read_input_tokens: u64::from(cache_read_tokens),
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("grok-4.5"),
            usage,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        let expected = (f64::from(uncached_input_tokens) * input_rate
            + f64::from(output_tokens) * output_rate
            + f64::from(cache_read_tokens) * cache_read_rate)
            / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-9, "{tier} cost was {cost}");
    }
}

fn deepseek_pricing() -> PricingMap {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                },
                "deepseek-v4-pro": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.000000435,
                    "output_cost_per_token": 0.00000087,
                    "cache_creation_input_token_cost": 0.000000456,
                    "cache_read_input_token_cost": 0.000000003625
                }
            }"#,
    );
    pricing
}

fn timestamp(value: &str) -> crate::TimestampMs {
    crate::parse_ts_timestamp(value).unwrap()
}

#[test]
fn calculates_all_deepseek_v4_token_buckets_at_each_scheduled_rate() {
    let pricing = deepseek_pricing();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        cache_creation_input_tokens: 1_000_000,
        cache_read_input_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };
    let cases = [
        (
            "2026-08-16T15:59:59Z",
            [("deepseek-v4-flash", 0.5628), ("deepseek-v4-pro", 1.743625)],
        ),
        (
            "2026-08-17T12:00:00Z",
            [("deepseek-v4-flash", 1.107), ("deepseek-v4-pro", 3.322)],
        ),
        (
            "2026-08-17T01:00:00Z",
            [("deepseek-v4-flash", 2.214), ("deepseek-v4-pro", 6.644)],
        ),
    ];

    for (timestamp_text, models) in cases {
        for (model, expected) in models {
            let cost = calculate_cost_for_usage_at(
                Some(model),
                usage,
                None,
                Some(timestamp(timestamp_text)),
                CostMode::Calculate,
                Some(&pricing),
            );
            assert!((cost - expected).abs() < 1e-12, "{model} cost was {cost}");
        }
    }
}

#[test]
fn scheduled_rates_preserve_display_and_auto_cost_semantics() {
    let pricing = deepseek_pricing();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };
    let timestamp = Some(timestamp("2026-08-17T01:00:00Z"));

    assert_eq!(
        calculate_cost_for_usage_at(
            Some("deepseek-v4-flash"),
            usage,
            Some(42.0),
            timestamp,
            CostMode::Display,
            Some(&pricing),
        ),
        42.0
    );
    assert_eq!(
        calculate_cost_for_usage_at(
            Some("deepseek-v4-flash"),
            usage,
            Some(42.0),
            timestamp,
            CostMode::Auto,
            Some(&pricing),
        ),
        42.0
    );
    let calculated = calculate_cost_for_usage_at(
        Some("deepseek-v4-flash"),
        usage,
        Some(42.0),
        timestamp,
        CostMode::Calculate,
        Some(&pricing),
    );
    assert!((calculated - 0.44).abs() < 1e-12);
    let automatic = calculate_cost_for_usage_at(
        Some("deepseek-v4-flash"),
        usage,
        None,
        timestamp,
        CostMode::Auto,
        Some(&pricing),
    );
    assert!((automatic - 0.44).abs() < 1e-12);
}

#[test]
fn scheduled_base_only_override_bills_every_input_token_at_the_override() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &value)]);
    let cost = calculate_cost_for_usage_at(
        Some("deepseek-v4-flash"),
        TokenUsageRaw {
            input_tokens: 300_000,
            ..TokenUsageRaw::default()
        },
        None,
        Some(timestamp("2026-08-17T01:00:00Z")),
        CostMode::Calculate,
        Some(&pricing),
    );

    assert!((cost - 2.7).abs() < 1e-12, "cost was {cost}");
}

#[test]
fn legacy_cost_api_keeps_static_lookup_without_a_timestamp() {
    let pricing = deepseek_pricing();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };

    assert_eq!(
        calculate_cost_for_usage(
            Some("deepseek-v4-flash"),
            usage,
            None,
            CostMode::Calculate,
            Some(&pricing),
        ),
        0.14
    );
}

#[test]
fn usage_entry_cost_wrapper_uses_event_timestamp() {
    let pricing = deepseek_pricing();
    let usage = TokenUsageRaw {
        input_tokens: 1_000_000,
        ..TokenUsageRaw::default()
    };
    for (timestamp, expected) in [
        (crate::parse_ts_timestamp("2026-08-17T01:00:00Z"), 0.44),
        (None, 0.14),
    ] {
        let cost = calculate_cost_for_usage_at(
            Some("deepseek-v4-flash"),
            usage,
            None,
            timestamp,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!((cost - expected).abs() < 1e-12, "{cost}");
    }
}

#[test]
fn prices_a_qwen_plus_thinking_request_above_256k_at_alibabas_long_context_rates() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let output_tokens = 1_000_u64;
    for (case, input_tokens, (input_rate, output_rate)) in [
        ("below the boundary", 255_999_u64, (0.4, 4.0)),
        ("at the boundary", 256_000, (0.4, 4.0)),
        ("above the boundary", 256_001, (1.2, 12.0)),
    ] {
        let usage = TokenUsageRaw {
            input_tokens,
            output_tokens,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("qwen-plus-thinking"),
            usage,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        let expected =
            (input_tokens as f64 * input_rate + output_tokens as f64 * output_rate) / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-12, "{case}: {cost}");
    }
}

fn price_with_granularity(map: &PricingMap, model: &str, granularity: UsageGranularity) -> f64 {
    let candidates = [(model, crate::pricing::MatchKind::Exact)];
    price_entry(
        EntryPricing {
            usage: TokenUsageRaw {
                input_tokens: 300,
                output_tokens: 10,
                ..TokenUsageRaw::default()
            },
            granularity,
            reasoning_tokens: 0,
            reported_model: None,
            candidates: &candidates,
            stored_cost: None,
            timestamp: None,
        },
        PriceTable::Calculate(map),
    )
    .cost
}

#[test]
fn aggregated_usage_is_priced_at_the_base_tier_of_a_whole_request_model() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_models_dev_json_for_tests(
        r#"{"two-stage-test": {"cost": {
            "input": 1, "output": 10,
            "tiers": [{"input": 2, "output": 20, "tier": {"type": "context", "size": 100}}]
        }}}"#,
    );

    let request = price_with_granularity(&pricing, "two-stage-test", UsageGranularity::Request);
    let aggregate = price_with_granularity(&pricing, "two-stage-test", UsageGranularity::Aggregate);

    assert!((request - (300.0 * 2.0 + 10.0 * 20.0) / 1e6).abs() < 1e-12);
    assert!((aggregate - (300.0 * 1.0 + 10.0 * 10.0) / 1e6).abs() < 1e-12);
}

#[test]
fn aggregated_usage_is_priced_at_the_base_tier_of_a_marginal_model() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"marginal-test": {
            "input_cost_per_token": 1, "output_cost_per_token": 10,
            "input_cost_per_token_above_200k_tokens": 2
        }}"#,
    );
    let candidates = [("marginal-test", crate::pricing::MatchKind::Exact)];
    let price = |granularity| {
        price_entry(
            EntryPricing {
                usage: TokenUsageRaw {
                    input_tokens: 250_000,
                    ..TokenUsageRaw::default()
                },
                granularity,
                reasoning_tokens: 0,
                reported_model: None,
                candidates: &candidates,
                stored_cost: None,
                timestamp: None,
            },
            PriceTable::Calculate(&pricing),
        )
        .cost
    };

    assert_eq!(price(UsageGranularity::Request), 200_000.0 + 50_000.0 * 2.0);
    assert_eq!(price(UsageGranularity::Aggregate), 250_000.0);
}
