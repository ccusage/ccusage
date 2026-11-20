use std::collections::BTreeMap;

use ccusage_cli::PricingOverride;

use crate::config_schema::ConfigPricingOverride;

pub(super) fn merge_pricing_overrides(
    current: &mut BTreeMap<String, PricingOverride>,
    incoming: BTreeMap<String, ConfigPricingOverride>,
) {
    for (model, incoming_override) in incoming {
        let entry = current.entry(model).or_default();
        merge_override_fields(entry, incoming_override);
    }
}

fn merge_override_fields(target: &mut PricingOverride, source: ConfigPricingOverride) {
    if source.input_cost_per_token.is_some() {
        target.input_cost_per_token = source.input_cost_per_token;
    }
    if source.output_cost_per_token.is_some() {
        target.output_cost_per_token = source.output_cost_per_token;
    }
    if source.cache_creation_input_token_cost.is_some() {
        target.cache_creation_input_token_cost = source.cache_creation_input_token_cost;
    }
    if source.cache_creation_input_token_cost_above_1hr.is_some() {
        target.cache_creation_input_token_cost_above_1hr =
            source.cache_creation_input_token_cost_above_1hr;
    }
    if source.cache_read_input_token_cost.is_some() {
        target.cache_read_input_token_cost = source.cache_read_input_token_cost;
    }
    if source.input_cost_per_token_above_200k_tokens.is_some() {
        target.input_cost_per_token_above_200k_tokens =
            source.input_cost_per_token_above_200k_tokens;
    }
    if source.output_cost_per_token_above_200k_tokens.is_some() {
        target.output_cost_per_token_above_200k_tokens =
            source.output_cost_per_token_above_200k_tokens;
    }
    if source
        .cache_creation_input_token_cost_above_200k_tokens
        .is_some()
    {
        target.cache_creation_input_token_cost_above_200k_tokens =
            source.cache_creation_input_token_cost_above_200k_tokens;
    }
    if source
        .cache_read_input_token_cost_above_200k_tokens
        .is_some()
    {
        target.cache_read_input_token_cost_above_200k_tokens =
            source.cache_read_input_token_cost_above_200k_tokens;
    }
    if source.max_input_tokens.is_some() {
        target.max_input_tokens = source.max_input_tokens;
    }
    if source.fast_multiplier.is_some() {
        target.fast_multiplier = source.fast_multiplier;
    }
    if source.flex_multiplier.is_some() {
        target.flex_multiplier = source.flex_multiplier;
    }
}
