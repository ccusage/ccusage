use crate::{
    cli::CostMode,
    pricing::PricingMap,
    types::{Speed, UsageEntry},
};

const CACHE_CREATE_1H_INPUT_MULTIPLIER: f64 = 2.0;

pub(crate) fn calculate_cost(
    data: &UsageEntry,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    calculate_cost_for_usage(
        data.message.model.as_deref(),
        data.message.usage,
        data.cost_usd,
        mode,
        pricing,
    )
}

/// Cost and missing-pricing flag for output paths, honoring the cost mode.
///
/// Cost and the missing-pricing flag are decided together so they stay
/// coherent: a row that surfaces a logged `costUSD` is never also flagged as
/// missing pricing.
///
/// `Display` surfaces the agent's logged `costUSD` verbatim (or `0.0` when
/// absent) and never reprices — "display" means show what the agent recorded.
///
/// `Auto` uses the logged `costUSD` when present; only falls back to token
/// computation when the entry has no logged cost. This keeps warm-run output
/// faithful to the agent's own accounting for mapped models, while still
/// computing cost for unmapped models that carry no logged spend.
///
/// `Calculate` always derives cost from tokens + current pricing, ignoring the
/// logged `costUSD` entirely.
pub(crate) fn cost_and_missing_for_output(
    data: &UsageEntry,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> (f64, Option<String>) {
    let model = data.message.model.as_deref();
    let usage = data.message.usage;
    let cost = calculate_cost_for_usage(model, usage, data.cost_usd, mode, pricing);
    let missing = missing_pricing_model_for_usage(model, usage, data.cost_usd, mode, pricing);
    (cost, missing)
}

pub(crate) fn calculate_cost_for_usage(
    model: Option<&str>,
    usage: crate::TokenUsageRaw,
    cost_usd: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    match mode {
        CostMode::Display => cost_usd.unwrap_or(0.0),
        CostMode::Auto => {
            cost_usd.unwrap_or_else(|| calculate_cost_from_tokens(model, usage, pricing))
        }
        CostMode::Calculate => calculate_cost_from_tokens(model, usage, pricing),
    }
}

pub(crate) fn missing_pricing_model_for_usage(
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

pub(crate) fn missing_pricing_model_for_token_total(
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

pub(crate) fn missing_pricing_model_for_candidates(
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
    pricing: Option<&PricingMap>,
) -> f64 {
    let Some(model) = model else {
        return 0.0;
    };
    let Some(pricing) = pricing.and_then(|pricing| pricing.find(model)) else {
        return 0.0;
    };
    let multiplier = if matches!(usage.speed, Some(Speed::Fast)) {
        pricing.fast_multiplier
    } else {
        1.0
    };
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
    (tiered_cost(usage.input_tokens, pricing.input, pricing.input_above_200k)
        + tiered_cost(
            usage.output_tokens,
            pricing.output,
            pricing.output_above_200k,
        )
        + tiered_cost(
            cache_create_5m_tokens,
            pricing.cache_create,
            pricing.cache_create_above_200k,
        )
        + tiered_cost(
            cache_create_1h_tokens,
            cache_create_1h_cost,
            cache_create_1h_cost_above_200k,
        )
        + tiered_cost(
            usage.cache_read_input_tokens,
            pricing.cache_read,
            pricing.cache_read_above_200k,
        ))
        * multiplier
}

pub(crate) fn tiered_cost(tokens: u64, base: f64, above: Option<f64>) -> f64 {
    const THRESHOLD: u64 = 200_000;
    if tokens == 0 {
        return 0.0;
    }
    if let Some(above) = above
        && tokens > THRESHOLD
    {
        return (THRESHOLD as f64 * base) + ((tokens - THRESHOLD) as f64 * above);
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

    use super::{calculate_cost_for_usage, cost_and_missing_for_output};

    fn entry(model: &str, cost_usd: Option<f64>) -> UsageEntry {
        UsageEntry {
            session_id: Some("session".to_string()),
            timestamp: "2026-01-02T00:00:00.000Z".to_string(),
            version: None,
            message: UsageMessage {
                usage: TokenUsageRaw {
                    input_tokens: 100,
                    output_tokens: 200,
                    ..TokenUsageRaw::default()
                },
                model: Some(model.to_string()),
                id: None,
                provider: None,
            },
            cost_usd,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        }
    }

    #[test]
    fn auto_falls_back_to_logged_cost_for_unmapped_model() {
        // Model absent from the pricing map: Auto must surface the logged
        // costUSD rather than zeroing it, and must not flag missing pricing.
        let data = entry("model-not-in-map", Some(0.42));
        let (cost, missing) = cost_and_missing_for_output(&data, CostMode::Auto, Some(&pricing()));
        assert_eq!(cost, 0.42);
        assert_eq!(missing, None);
    }

    #[test]
    fn calculate_stays_strict_zero_for_unmapped_model() {
        // Calculate with pricing available keeps its always-recompute contract:
        // an unmapped model is $0.00 and is flagged as missing pricing.
        let data = entry("model-not-in-map", Some(0.42));
        let (cost, missing) =
            cost_and_missing_for_output(&data, CostMode::Calculate, Some(&pricing()));
        assert_eq!(cost, 0.0);
        assert_eq!(missing.as_deref(), Some("model-not-in-map"));
    }

    #[test]
    fn auto_uses_logged_cost_for_mapped_model() {
        // Mapped model in Auto with logged costUSD: the logged value is used
        // verbatim (999.0), not recomputed from tokens. No missing flag.
        let data = entry("test-model", Some(999.0));
        let (cost, missing) = cost_and_missing_for_output(&data, CostMode::Auto, Some(&pricing()));
        assert!((cost - 999.0).abs() < f64::EPSILON);
        assert_eq!(missing, None);
    }

    #[test]
    fn auto_falls_back_to_compute_when_no_logged_cost() {
        // Auto with no logged costUSD and a priceable model: computes from tokens.
        let data = entry("test-model", None);
        let (cost, missing) = cost_and_missing_for_output(&data, CostMode::Auto, Some(&pricing()));
        assert!((cost - 2100.0).abs() < f64::EPSILON);
        assert_eq!(missing, None);
    }

    #[test]
    fn auto_flags_missing_when_no_logged_cost_and_unmapped() {
        // Auto with no logged costUSD and an unmapped model: flags missing pricing.
        let data = entry("model-not-in-map", None);
        let (cost, missing) = cost_and_missing_for_output(&data, CostMode::Auto, Some(&pricing()));
        assert_eq!(cost, 0.0);
        assert_eq!(missing.as_deref(), Some("model-not-in-map"));
    }

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
}
