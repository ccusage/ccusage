use crate::{
    cli::CostMode,
    pricing::{Pricing, PricingHandle, PricingMap},
    types::Speed,
};

pub struct EntryPricing<'a> {
    pub usage: crate::TokenUsageRaw,
    pub reasoning_tokens: u64,
    pub residual_tokens: u64,
    pub reported_model: Option<&'a str>,
    pub candidates: &'a [(&'a str, crate::pricing::MatchKind)],
    pub stored_cost: Option<f64>,
    pub timestamp: Option<crate::TimestampMs>,
}

#[derive(Debug, Clone, Copy)]
pub struct EntryPrice<'a> {
    pub cost: f64,
    pub matched: Option<&'a str>,
    pub missing_pricing: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum PriceTable<'a> {
    Display,
    Auto(&'a PricingMap),
    Calculate(&'a PricingMap),
}

impl<'a> PriceTable<'a> {
    pub fn for_map(mode: CostMode, map: &'a PricingMap) -> Self {
        match mode {
            CostMode::Display => Self::Display,
            CostMode::Auto => Self::Auto(map),
            CostMode::Calculate => Self::Calculate(map),
        }
    }

    pub fn as_map(&self) -> Option<&PricingMap> {
        match self {
            Self::Display => None,
            Self::Auto(map) | Self::Calculate(map) => Some(map),
        }
    }
}

pub fn price_entry<'a>(entry: EntryPricing<'a>, pricing: PriceTable<'_>) -> EntryPrice<'a> {
    let Some(map) = pricing.as_map() else {
        return EntryPrice {
            cost: entry.stored_cost.unwrap_or(0.0),
            matched: None,
            missing_pricing: false,
        };
    };
    if matches!(pricing, PriceTable::Auto(_))
        && let Some(cost) = entry.stored_cost
    {
        return EntryPrice {
            cost,
            matched: None,
            missing_pricing: false,
        };
    }
    let usage = crate::TokenUsageRaw {
        output_tokens: entry
            .usage
            .output_tokens
            .saturating_add(entry.reasoning_tokens)
            .saturating_add(entry.residual_tokens),
        ..entry.usage
    };
    let resolved = map.resolve_at(entry.reported_model, entry.candidates, entry.timestamp);
    match resolved {
        Some((model, rates)) => EntryPrice {
            cost: cost_at_speed(usage, &rates),
            matched: Some(model),
            missing_pricing: false,
        },
        None => EntryPrice {
            cost: 0.0,
            matched: None,
            missing_pricing: !entry.candidates.is_empty() && crate::total_usage_tokens(usage) != 0,
        },
    }
}

pub fn price_resolved_usage(
    model: &str,
    usage: crate::TokenUsageRaw,
    timestamp: crate::TimestampMs,
    rates: Option<&PricingHandle>,
) -> (f64, Option<String>) {
    match rates {
        Some(handle) => (
            handle.with_rates_at(Some(timestamp), |rates| cost_at_speed(usage, rates)),
            None,
        ),
        None => {
            let missing = (crate::total_usage_tokens(usage) != 0)
                .then(|| crate::model_aliases::resolve_model_name(model).into_owned());
            (0.0, missing)
        }
    }
}

fn cost_at_speed(usage: crate::TokenUsageRaw, rates: &Pricing) -> f64 {
    let multiplier = if matches!(usage.speed, Some(Speed::Fast)) {
        rates.fast_multiplier
    } else {
        1.0
    };
    calculate_cost_from_pricing(usage, rates) * multiplier
}

pub fn stored_cost_for_mode(cost_usd: Option<f64>, mode: CostMode) -> Option<f64> {
    cost_usd.filter(|_| mode == CostMode::Auto)
}

pub fn missing_pricing_model_for_token_total(
    model: Option<&str>,
    total_tokens: u64,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if total_tokens == 0 {
        return None;
    }
    let model = model?;
    let pricing = pricing?;
    pricing
        .find(model)
        .is_none()
        .then(|| crate::model_aliases::resolve_model_name(model).into_owned())
}

fn calculate_cost_from_pricing(usage: crate::TokenUsageRaw, pricing: &Pricing) -> f64 {
    let (cache_create_5m_tokens, cache_create_1h_tokens) =
        if let Some(breakdown) = usage.cache_creation {
            (
                breakdown.ephemeral_5m_input_tokens,
                breakdown.ephemeral_1h_input_tokens,
            )
        } else {
            (usage.cache_creation_input_tokens, 0)
        };
    let cache_create_1h_cost = pricing.one_hour_cache_creation_rate();
    let cache_create = pricing.cache_creation_rate();
    let cache_read = pricing.cache_read_rate_at(pricing.input);
    let cache_create_1h_cost_above_200k = pricing.one_hour_cache_creation_rate_above_200k();

    let rates = pricing
        .long_context
        .map(|context| context.rates)
        .unwrap_or_default();
    if let Some(context) = pricing
        .long_context
        .filter(|context| context.mode == crate::pricing::LongContextMode::WholeRequest)
    {
        let threshold = context.threshold;
        let context_tokens = usage
            .input_tokens
            .saturating_add(usage.cache_read_input_tokens)
            .saturating_add(cache_create_5m_tokens)
            .saturating_add(cache_create_1h_tokens);
        let long_context = context_tokens > threshold;
        let rate = |base: f64, above: Option<f64>| {
            if long_context {
                above.unwrap_or(base)
            } else {
                base
            }
        };
        return usage.input_tokens as f64 * rate(pricing.input, rates.input)
            + usage.output_tokens as f64 * rate(pricing.output, rates.output)
            + cache_create_5m_tokens as f64 * rate(cache_create, rates.cache_create)
            + cache_create_1h_tokens as f64
                * rate(cache_create_1h_cost, cache_create_1h_cost_above_200k)
            + usage.cache_read_input_tokens as f64 * rate(cache_read, rates.cache_read);
    }

    let threshold = pricing.long_context_split_threshold();
    tiered_cost(usage.input_tokens, pricing.input, rates.input, threshold)
        + tiered_cost(usage.output_tokens, pricing.output, rates.output, threshold)
        + tiered_cost(
            cache_create_5m_tokens,
            cache_create,
            rates.cache_create,
            threshold,
        )
        + tiered_cost(
            cache_create_1h_tokens,
            cache_create_1h_cost,
            cache_create_1h_cost_above_200k,
            threshold,
        )
        + tiered_cost(
            usage.cache_read_input_tokens,
            cache_read,
            rates.cache_read,
            threshold,
        )
}

pub fn tiered_cost(tokens: u64, base: f64, above: Option<f64>, threshold: u64) -> f64 {
    if tokens == 0 {
        return 0.0;
    }
    if let Some(above) = above
        && tokens > threshold
    {
        return (threshold as f64 * base) + ((tokens - threshold) as f64 * above);
    }
    tokens as f64 * base
}

/// Adds up costs starting from `+0.0`. `Iterator::sum` for `f64` starts from
/// `-0.0`, so an empty selection would print as `$-0.00` and `-0.0` in JSON.
pub fn sum_costs(costs: impl IntoIterator<Item = f64>) -> f64 {
    costs.into_iter().fold(0.0, |total, cost| total + cost)
}

#[cfg(test)]
mod tests {
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
            CostMode::Auto => cost_usd
                .unwrap_or_else(|| calculate_cost_from_tokens(model, usage, timestamp, pricing)),
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
    fn candidate_entry_pricing_adds_only_the_supplied_extra_tokens() {
        crate::test_support::install_pricing_snapshots();
        let mut map = PricingMap::default();
        map.load_json(
            r#"{"priced": {
            "input_cost_per_token": 2, "output_cost_per_token": 3
        }}"#,
        );
        let candidates = [("priced", crate::pricing::MatchKind::Exact)];
        let priced = price_entry(
            EntryPricing {
                usage: crate::TokenUsageRaw {
                    input_tokens: 2,
                    output_tokens: 3,
                    ..Default::default()
                },
                reasoning_tokens: 4,
                residual_tokens: 5,
                reported_model: None,
                candidates: &candidates,
                stored_cost: Some(99.0),
                timestamp: None,
            },
            PriceTable::Calculate(&map),
        );
        assert_eq!(priced.cost, 40.0);
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
                    usage: crate::TokenUsageRaw {
                        input_tokens: 100,
                        ..Default::default()
                    },
                    reasoning_tokens: 0,
                    residual_tokens: 0,
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
        for (input_tokens, reasoning_tokens, residual_tokens, mode, stored_cost, missing) in [
            (0, 0, 0, CostMode::Calculate, None, false),
            (1, 0, 0, CostMode::Calculate, None, true),
            (0, 1, 0, CostMode::Calculate, None, true),
            (0, 0, 1, CostMode::Calculate, None, true),
            (1, 0, 0, CostMode::Display, None, false),
            (1, 0, 0, CostMode::Auto, Some(0.0), false),
        ] {
            let result = price_entry(
                EntryPricing {
                    usage: crate::TokenUsageRaw {
                        input_tokens,
                        ..Default::default()
                    },
                    reasoning_tokens,
                    residual_tokens,
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
                usage: crate::TokenUsageRaw {
                    input_tokens: 1_000_000,
                    speed: Some(Speed::Fast),
                    ..Default::default()
                },
                reasoning_tokens: 0,
                residual_tokens: 0,
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
    fn published_cache_rates_survive_an_input_override_without_a_cache_read_rate() {
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
        assert_eq!(calculate_cost_from_pricing(usage, &entry), 1_440_012.0);
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
    fn zero_rate_pricing_is_not_missing_pricing() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{ "free-model": { "litellm_provider": "openai", "input_cost_per_token": 0, "output_cost_per_token": 0 } }"#,
        );

        assert_eq!(
            missing_pricing_model_for_token_total(Some("free-model"), 100, Some(&pricing)),
            None
        );
        assert_eq!(
            missing_pricing_model_for_token_total(Some("unknown-model"), 100, Some(&pricing)),
            Some("unknown-model".to_string())
        );
        assert_eq!(
            missing_pricing_model_for_token_total(Some("unknown-model"), 0, Some(&pricing)),
            None,
            "usage without tokens is not reported"
        );
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
                    usage,
                    reasoning_tokens: 0,
                    residual_tokens: 0,
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
    fn prices_summed_message_iteration_breakdowns() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_input_tokens": 30,
                "cache_read_input_tokens": 30,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 10,
                    "ephemeral_1h_input_tokens": 0
                },
                "iterations": [
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 10, "ephemeral_1h_input_tokens": 0}},
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 20}}
                ]
            }"#,
        )
        .unwrap();

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

        // Keep cache reads non-zero so tier selection covers the whole context
        // rather than only freshly processed input.
        let short = TokenUsageRaw {
            input_tokens: 59,
            output_tokens: 3,
            cache_read_input_tokens: 40,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("two-stage-test"),
            short,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!(
            (cost - 93e-6).abs() < 1e-12,
            "short-context cost was {cost}"
        );

        let boundary = TokenUsageRaw {
            input_tokens: 60,
            output_tokens: 3,
            cache_read_input_tokens: 40,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("two-stage-test"),
            boundary,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!((cost - 94e-6).abs() < 1e-12, "boundary cost was {cost}");

        let long = TokenUsageRaw {
            input_tokens: 61,
            output_tokens: 3,
            cache_read_input_tokens: 40,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("two-stage-test"),
            long,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!(
            (cost - 190e-6).abs() < 1e-12,
            "long-context cost was {cost}"
        );
    }

    #[test]
    fn cached_context_selects_the_long_context_tier() {
        crate::test_support::install_pricing_snapshots();
        // The tier is chosen by the whole context the request carried, and on
        // agents with aggressive prompt caching almost all of it is cache
        // reads: a Grok turn re-reading megabytes of cached context with a few
        // thousand fresh tokens is a long-context request. Judging by uncached
        // input alone billed such turns at the short-context rate.
        let pricing = PricingMap::load_embedded();

        // grok-4.5: base $2/$6, cache read $0.3; above 200K context $4/$12/$0.6.
        let cached_heavy = TokenUsageRaw {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_read_input_tokens: 500_000,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("grok-4.5"),
            cached_heavy,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        // 0.01M * 4 + 0.001M * 12 + 0.5M * 0.6
        assert!((cost - 0.352).abs() < 1e-9, "cached-heavy cost was {cost}");

        // The same shape below the boundary stays on the base rates.
        let short = TokenUsageRaw {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_read_input_tokens: 100_000,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("grok-4.5"),
            short,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        // 0.01M * 2 + 0.001M * 6 + 0.1M * 0.3
        assert!((cost - 0.056).abs() < 1e-9, "short-context cost was {cost}");
    }

    #[test]
    fn parses_cache_creation_breakdown_from_usage_json() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 1,
                "output_tokens": 2,
                "cache_creation_input_tokens": 300,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 100,
                    "ephemeral_1h_input_tokens": 200
                }
            }"#,
        )
        .unwrap();

        assert_eq!(usage.cache_creation_token_count(), 300);
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
}
