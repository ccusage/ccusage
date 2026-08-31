use crate::{
    cli::CostMode,
    pricing::{Pricing, PricingMap},
    types::{Speed, UsageEntry},
};

const CACHE_CREATE_1H_INPUT_MULTIPLIER: f64 = 2.0;

pub fn calculate_cost(data: &UsageEntry, mode: CostMode, pricing: Option<&PricingMap>) -> f64 {
    calculate_cost_for_usage_at(
        data.message.model.as_deref(),
        data.message.usage,
        data.cost_usd,
        crate::parse_ts_timestamp(&data.timestamp),
        mode,
        pricing,
    )
}

#[cfg(test)]
fn calculate_cost_for_usage(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    cost_usd: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    calculate_cost_for_usage_at(model, usage, cost_usd, None, mode, pricing)
}

pub fn calculate_cost_for_usage_at(
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

pub fn missing_pricing_model_for_usage(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    cost_usd: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if mode == CostMode::Display || (mode == CostMode::Auto && cost_usd.is_some()) {
        return None;
    }
    missing_pricing_model_for_token_total(model, crate::total_usage_tokens(usage), pricing)
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

pub fn missing_pricing_model_for_candidates(
    model: &str,
    candidates: impl IntoIterator<Item = String>,
    total_tokens: u64,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if total_tokens == 0 {
        return None;
    }
    let pricing = pricing?;
    candidates
        .into_iter()
        .all(|candidate| pricing.find(&candidate).is_none())
        .then(|| crate::model_aliases::resolve_model_name(model).into_owned())
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
    let multiplier = if matches!(usage.speed, Some(Speed::Fast)) {
        pricing.fast_multiplier
    } else {
        1.0
    };
    calculate_cost_from_pricing(usage, pricing) * multiplier
}

pub fn calculate_cost_from_pricing(usage: crate::TokenUsageRaw, pricing: Pricing) -> f64 {
    let (cache_create_5m_tokens, cache_create_1h_tokens) =
        if let Some(breakdown) = usage.cache_creation {
            (
                breakdown.ephemeral_5m_input_tokens,
                breakdown.ephemeral_1h_input_tokens,
            )
        } else {
            (usage.cache_creation_input_tokens, 0)
        };
    let cache_create_1h_cost = pricing.input * CACHE_CREATE_1H_INPUT_MULTIPLIER;
    let cache_create_1h_cost_above_200k = pricing
        .input_above_200k
        .map(|c| c * CACHE_CREATE_1H_INPUT_MULTIPLIER);

    // Two-stage pricing: a per-model `long_context_threshold` means the
    // request's input size selects the tier and every bucket is billed entirely
    // at that tier's rate. The whole request switches once input exceeds the
    // threshold, so this is not a marginal breakpoint. This mirrors the Codex
    // per-request tiering in `calculate_codex_model_cost`.
    //
    // `input_tokens` here is the uncached remainder - adapters normalize usage
    // into the Claude shape - but the vendor's tier is chosen by the request's
    // whole context, cached or not: a Grok turn re-reading 8M cached tokens
    // with 10K fresh ones is a long-context request, not a short one.
    if let Some(threshold) = pricing.long_context_threshold {
        // Saturating: the counters come from lenient JSONL parsing, and a
        // corrupt line must not wrap the sum below the threshold or abort under
        // overflow checks.
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
        return usage.input_tokens as f64 * rate(pricing.input, pricing.input_above_200k)
            + usage.output_tokens as f64 * rate(pricing.output, pricing.output_above_200k)
            + cache_create_5m_tokens as f64
                * rate(pricing.cache_create, pricing.cache_create_above_200k)
            + cache_create_1h_tokens as f64
                * rate(cache_create_1h_cost, cache_create_1h_cost_above_200k)
            + usage.cache_read_input_tokens as f64
                * rate(pricing.cache_read, pricing.cache_read_above_200k);
    }

    // LiteLLM `*_above_200k_tokens` data keeps its marginal above-threshold
    // semantics at the default 200K boundary.
    let threshold = crate::pricing::DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS;
    tiered_cost(
        usage.input_tokens,
        pricing.input,
        pricing.input_above_200k,
        threshold,
    ) + tiered_cost(
        usage.output_tokens,
        pricing.output,
        pricing.output_above_200k,
        threshold,
    ) + tiered_cost(
        cache_create_5m_tokens,
        pricing.cache_create,
        pricing.cache_create_above_200k,
        threshold,
    ) + tiered_cost(
        cache_create_1h_tokens,
        cache_create_1h_cost,
        cache_create_1h_cost_above_200k,
        threshold,
    ) + tiered_cost(
        usage.cache_read_input_tokens,
        pricing.cache_read,
        pricing.cache_read_above_200k,
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

#[cfg(test)]
mod tests {
    use crate::{
        cli::CostMode,
        pricing::PricingMap,
        types::{CacheCreationRaw, TokenUsageRaw, UsageEntry, UsageMessage},
    };

    use super::{calculate_cost, calculate_cost_for_usage, calculate_cost_for_usage_at};

    fn pricing() -> PricingMap {
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
    fn prices_two_stage_model_as_whole_request_at_long_context_rates() {
        let pricing = PricingMap::load_embedded();

        // gpt-5.6-sol has a 272K threshold with long-context rates of
        // $10/$45 per 1M input/output tokens and a $1 per 1M cache-read rate.
        let long = TokenUsageRaw {
            input_tokens: 300_000,
            output_tokens: 1_000,
            cache_read_input_tokens: 100,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("gpt-5.6-sol"),
            long,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        // The whole request switches to long rates once input exceeds 272K,
        // including the output and cache-read buckets that are individually
        // far below the threshold: 3.0 + 0.045 + 0.0001.
        assert!((cost - 3.0451).abs() < 1e-9, "long-context cost was {cost}");

        // Below the threshold every bucket stays on the short-context rates:
        // 0.5 + 0.03 + 0.00005.
        let short = TokenUsageRaw {
            input_tokens: 100_000,
            output_tokens: 1_000,
            cache_read_input_tokens: 100,
            ..TokenUsageRaw::default()
        };
        let cost = calculate_cost_for_usage(
            Some("gpt-5.6-sol"),
            short,
            None,
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!(
            (cost - 0.53005).abs() < 1e-9,
            "short-context cost was {cost}"
        );
    }

    #[test]
    fn cached_context_selects_the_long_context_tier() {
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
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                },
                "deepseek-v4-pro": {
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
        assert_eq!(
            calculate_cost_for_usage_at(
                Some("deepseek-v4-flash"),
                usage,
                Some(42.0),
                timestamp,
                CostMode::Calculate,
                Some(&pricing),
            ),
            0.44
        );
        assert_eq!(
            calculate_cost_for_usage_at(
                Some("deepseek-v4-flash"),
                usage,
                None,
                timestamp,
                CostMode::Auto,
                Some(&pricing),
            ),
            0.44
        );
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
        let mut entry = UsageEntry {
            session_id: None,
            timestamp: "2026-08-17T01:00:00Z".to_string(),
            version: None,
            message: UsageMessage {
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    ..TokenUsageRaw::default()
                },
                model: Some("deepseek-v4-flash".to_string()),
                id: None,
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };

        assert_eq!(
            calculate_cost(&entry, CostMode::Calculate, Some(&pricing)),
            0.44
        );

        entry.timestamp = "not-a-timestamp".to_string();
        assert_eq!(
            calculate_cost(&entry, CostMode::Calculate, Some(&pricing)),
            0.14
        );
    }
}
