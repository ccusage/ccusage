use serde::Deserialize;
use serde_json::Value;

use crate::fast::FxHashMap;

use super::{
    LongContextMode, LongContextRates, Pricing, PricingMap, Row, RowPricing, Scope, Source, data,
    embedded::litellm_sources, provider_cache_defaults, row_scope, schedule_for,
    speed::PublishedSpeed, vendor_start,
};

#[derive(Debug, Deserialize)]
struct LiteLlmPricing {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
    cache_creation_input_token_cost_above_1hr: Option<f64>,
    cache_creation_input_token_cost_above_1hr_above_200k_tokens: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    input_cost_per_token_above_200k_tokens: Option<f64>,
    output_cost_per_token_above_200k_tokens: Option<f64>,
    cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    max_input_tokens: Option<u64>,
    litellm_provider: Option<String>,
    provider_specific_entry: Option<ProviderSpecificEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderSpecificEntry {
    fast: Option<f64>,
    flex: Option<f64>,
}

impl PricingMap {
    pub fn load_json(&mut self, json: &str) -> usize {
        let Some(sources) = litellm_sources() else {
            return 0;
        };
        let Ok(raw) = serde_json::from_str::<FxHashMap<String, serde_json::Value>>(json) else {
            return 0;
        };
        let mut loaded_count = 0;
        for (model, value) in raw {
            let Some(pricing) = parse_litellm_pricing(value) else {
                continue;
            };
            let Some(input) = pricing.input_cost_per_token else {
                continue;
            };
            let Some(output) = pricing.output_cost_per_token else {
                continue;
            };
            let provider = pricing.litellm_provider.as_deref();
            let (scope, vendor_id) = if let Some(provider) = provider {
                row_scope(&model, provider, &sources.direct_providers)
            } else {
                (Scope::Hosted, None)
            };
            let free = input == 0.0 && output == 0.0;
            if free && scope == Scope::Hosted && !model.contains('/') {
                continue;
            }
            if free
                && provider.is_some_and(|provider| {
                    sources
                        .zero_token_rate_unpriced_providers
                        .iter()
                        .any(|id| id == provider)
                })
            {
                continue;
            }

            let provider_specific = pricing
                .provider_specific_entry
                .as_ref()
                .map(|entry| PublishedSpeed {
                    fast: entry.fast,
                    ultrafast: None,
                    flex: entry.flex,
                })
                .unwrap_or_default();
            let cache_fallback = provider_cache_defaults(provider);
            let schedule = vendor_id.and_then(schedule_for);
            let vendor_start = vendor_id.and_then(|id| vendor_start(&model, id));
            let Some(row) = self.rows_mut().entries.get_or_insert(model, Row::default) else {
                continue;
            };
            row.source = Source::LiteLlm;
            row.scope = scope;
            row.schedule = schedule;
            row.vendor_start = vendor_start;
            row.context_limit = pricing.max_input_tokens.or(row.context_limit);
            row.pricing = RowPricing::by_key(
                Pricing {
                    input,
                    output,
                    cache_create: pricing.cache_creation_input_token_cost,
                    cache_read: pricing.cache_read_input_token_cost,
                    cache_create_1h: pricing.cache_creation_input_token_cost_above_1hr,
                    long_context: LongContextRates {
                        input: pricing.input_cost_per_token_above_200k_tokens,
                        output: pricing.output_cost_per_token_above_200k_tokens,
                        cache_create: pricing.cache_creation_input_token_cost_above_200k_tokens,
                        cache_read: pricing.cache_read_input_token_cost_above_200k_tokens,
                        cache_create_1h: pricing
                            .cache_creation_input_token_cost_above_1hr_above_200k_tokens,
                    }
                    .marginal()
                    .map(|mut context| {
                        if let Some((_, threshold)) = provider.and_then(|provider| {
                            data::WHOLE_REQUEST_LONG_CONTEXT_PROVIDERS
                                .iter()
                                .find(|(id, _)| *id == provider)
                        }) {
                            context.mode = LongContextMode::WholeRequest;
                            context.threshold = *threshold;
                        }
                        context
                    }),
                    ..Pricing::empty()
                },
                provider_specific,
                cache_fallback,
            );
            loaded_count += 1;
        }
        self.clear_find_cache();
        loaded_count
    }
}

fn parse_litellm_pricing(value: Value) -> Option<LiteLlmPricing> {
    let pricing = serde_json::from_value::<LiteLlmPricing>(value).ok()?;
    pricing
        .input_cost_per_token
        .zip(pricing.output_cost_per_token)
        .map(|_| pricing)
}
