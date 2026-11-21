use crate::{
    cli::CostMode,
    pricing::{BillingRoute, Pricing, PricingHandle, PricingMap},
    types::Speed,
};

/// Whether an entry's usage is one model request or a total over several.
///
/// The long-context tier is chosen per request, so it applies only to
/// `Request` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageGranularity {
    Request,
    Aggregate,
}

pub struct EntryPricing<'a> {
    pub usage: crate::TokenUsageRaw,
    pub granularity: UsageGranularity,
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
    price_entry_with(entry, pricing, |map, entry| {
        map.resolve_at(entry.reported_model, entry.candidates, entry.timestamp)
    })
}

pub fn price_entry_on_route<'a>(
    entry: EntryPricing<'a>,
    route: BillingRoute,
    pricing: PriceTable<'_>,
) -> EntryPrice<'a> {
    price_entry_with(entry, pricing, |map, entry| {
        map.resolve_on_route_at(
            entry.reported_model,
            entry.candidates,
            route,
            entry.timestamp,
        )
    })
}

fn price_entry_with<'a>(
    entry: EntryPricing<'a>,
    pricing: PriceTable<'_>,
    resolve: impl FnOnce(&PricingMap, &EntryPricing<'a>) -> Option<(&'a str, Pricing)>,
) -> EntryPrice<'a> {
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
    match resolve(map, &entry) {
        Some((model, rates)) => EntryPrice {
            cost: cost_at_speed(usage, &rates_for(entry.granularity, rates)),
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

// NOTE(2026-11-20, #2365): Vendors choose the long-context tier per request. An aggregate entry records no per-request split, so it is priced at the base tier as the closest estimate.
fn rates_for(granularity: UsageGranularity, mut rates: Pricing) -> Pricing {
    if granularity == UsageGranularity::Aggregate {
        rates.long_context = None;
    }
    rates
}

fn cost_at_speed(usage: crate::TokenUsageRaw, rates: &Pricing) -> f64 {
    let fast = matches!(usage.speed, Some(Speed::Fast));
    let multiplier = if fast { rates.fast_multiplier } else { 1.0 };
    calculate_cost_from_pricing(usage, &rates.at_speed(fast)) * multiplier
}

// NOTE(2026-11-21, #2199): Kilo, Hermes, and OpenCode write a cost of 0 both for free usage and when the provider returned no cost. A non-positive cost is treated as not recorded: the tokens are priced instead, which still comes to $0 for a model whose price is zero.
pub fn recorded_cost(cost: Option<f64>) -> Option<f64> {
    cost.filter(|cost| *cost > 0.0)
}

pub fn stored_cost_for_mode(cost_usd: Option<f64>, mode: CostMode) -> Option<f64> {
    cost_usd.filter(|_| mode == CostMode::Auto)
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

/// Sums from `+0.0` so an empty total renders as `$0.00` and `0.0` in JSON.
pub fn sum_costs(costs: impl IntoIterator<Item = f64>) -> f64 {
    costs.into_iter().fold(0.0, |total, cost| total + cost)
}

#[cfg(test)]
mod tests;
