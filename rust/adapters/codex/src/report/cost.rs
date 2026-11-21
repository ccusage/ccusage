use crate::{CodexServiceTier, CodexTimestampedUsage, CodexUsageBucket, speed::CodexSpeedPolicy};

pub(super) fn calculate_codex_usage_cost(
    usage: CodexTimestampedUsage,
    pricing: &crate::Pricing,
    speed: CodexSpeedPolicy,
) -> f64 {
    let CodexTimestampedUsage {
        usage: total_usage,
        tier_usage,
    } = usage;
    let standard_cost = calculate_codex_bucket_cost(&total_usage, pricing);
    let empty = CodexUsageBucket::default();
    let (flex_usage, fast_usage, ultrafast_usage) = match speed {
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard) => {
            return standard_cost;
        }
        CodexSpeedPolicy::Forced(CodexServiceTier::Flex) => (total_usage, empty, empty),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast) => (empty, total_usage, empty),
        CodexSpeedPolicy::Forced(CodexServiceTier::Ultrafast) => (empty, empty, total_usage),
        CodexSpeedPolicy::Auto => (
            tier_usage[CodexServiceTier::Flex],
            tier_usage[CodexServiceTier::Fast],
            tier_usage[CodexServiceTier::Ultrafast],
        ),
    };
    standard_cost
        + calculate_codex_bucket_cost(&flex_usage, pricing) * (pricing.flex_multiplier - 1.0)
        + calculate_codex_bucket_cost(&fast_usage, pricing) * (pricing.fast_multiplier - 1.0)
        + calculate_codex_bucket_cost(&ultrafast_usage, pricing)
            * (pricing.ultrafast_multiplier - 1.0)
}

fn calculate_codex_bucket_cost(usage: &CodexUsageBucket, pricing: &crate::Pricing) -> f64 {
    let cache_read = pricing.cache_read_rate_at(pricing.input);
    let cache_creation = pricing.cache_creation_rate();
    let rates = pricing
        .long_context
        .map(|context| context.rates)
        .unwrap_or_default();
    let long_input_rate = rates.input.unwrap_or(pricing.input);
    let long_output_rate = rates.output.unwrap_or(pricing.output);
    let long_cache_read = rates
        .cache_read
        .unwrap_or_else(|| pricing.cache_read_rate_at(long_input_rate));
    let long_cache_creation_rate = rates.cache_create.unwrap_or(cache_creation);
    let long_input = usage.long_context_input_tokens.min(usage.input_tokens);
    let long_cached = usage
        .long_context_cached_input_tokens
        .min(usage.cached_input_tokens)
        .min(long_input);
    let long_cache_creation = usage
        .long_context_cache_creation_tokens
        .min(usage.cache_creation_tokens)
        .min(long_input.saturating_sub(long_cached));
    let long_output = usage.long_context_output_tokens.min(usage.output_tokens);
    let short_cached = usage.cached_input_tokens.saturating_sub(long_cached);
    let short_cache_creation = usage
        .cache_creation_tokens
        .saturating_sub(long_cache_creation);
    let short_non_cached = usage
        .input_tokens
        .saturating_sub(long_input)
        .saturating_sub(short_cached.saturating_add(short_cache_creation));
    let long_non_cached =
        long_input.saturating_sub(long_cached.saturating_add(long_cache_creation));
    short_non_cached as f64 * pricing.input
        + short_cached as f64 * cache_read
        + short_cache_creation as f64 * cache_creation
        + (usage.output_tokens - long_output) as f64 * pricing.output
        + long_non_cached as f64 * long_input_rate
        + long_cached as f64 * long_cache_read
        + long_cache_creation as f64 * long_cache_creation_rate
        + long_output as f64 * long_output_rate
}
