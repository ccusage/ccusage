use serde_json::{Map, Value};

const RATE_FIELDS: [&str; 10] = [
    "input_cost_per_token",
    "output_cost_per_token",
    "cache_creation_input_token_cost",
    "cache_creation_input_token_cost_above_1hr",
    "cache_read_input_token_cost",
    "input_cost_per_token_above_200k_tokens",
    "output_cost_per_token_above_200k_tokens",
    "cache_creation_input_token_cost_above_200k_tokens",
    "cache_read_input_token_cost_above_200k_tokens",
    "max_input_tokens",
];

pub(crate) fn filter_pricing_json(json: &str) -> Option<String> {
    let Value::Object(raw) = serde_json::from_str::<Value>(json).ok()? else {
        return None;
    };
    let mut filtered = Map::new();
    for (model, pricing) in raw {
        if !is_embedded_model(&model) {
            continue;
        }
        let Value::Object(pricing) = pricing else {
            continue;
        };
        let mut fields = Map::new();
        for field in RATE_FIELDS {
            if let Some(value) = pricing.get(field).filter(|value| !value.is_null()) {
                fields.insert(field.to_string(), value.clone());
            }
        }
        let tiers: Map<String, Value> = pricing
            .get("provider_specific_entry")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|entry| ["fast", "flex"].map(|tier| (tier, entry.get(tier))))
            .filter_map(|(tier, multiplier)| {
                let multiplier = multiplier.filter(|value| !value.is_null())?;
                Some((tier.to_string(), multiplier.clone()))
            })
            .collect();
        if !tiers.is_empty() {
            fields.insert("provider_specific_entry".to_string(), Value::Object(tiers));
        }
        if fields.contains_key("input_cost_per_token")
            && fields.contains_key("output_cost_per_token")
        {
            filtered.insert(model, Value::Object(fields));
        }
    }
    serde_json::to_string(&Value::Object(filtered)).ok()
}

fn is_embedded_model(model: &str) -> bool {
    model.starts_with("claude-")
        || model.starts_with("anthropic.")
        || model.starts_with("anthropic/")
        || model.starts_with("us.anthropic.")
        || model.starts_with("eu.anthropic.")
        || model.starts_with("global.anthropic.")
        || model.starts_with("jp.anthropic.")
        || model.starts_with("au.anthropic.")
        || model.starts_with("gpt-")
        || model == "o3"
        || model.starts_with("o3-")
        || model == "o4-mini"
        || model.starts_with("o4-mini-")
        || model.starts_with("openai/")
        || model.starts_with("azure/")
        || model.starts_with("zai/")
        || model.starts_with("openrouter/openai/")
}

#[cfg(test)]
mod tests {
    use super::filter_pricing_json;
    use ccusage_core::PricingMap;

    #[test]
    fn retains_published_one_hour_cache_rates() {
        let filtered = filter_pricing_json(r#"{"claude-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002,"cache_creation_input_token_cost_above_1hr":0.000007}}"#).unwrap();
        let pricing: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        assert_eq!(
            pricing["claude-test"]["cache_creation_input_token_cost_above_1hr"],
            7e-6
        );
    }

    #[test]
    fn keeps_service_tier_multipliers_in_embedded_pricing() {
        let filtered = filter_pricing_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2, "flex": 0.5 }
                },
                "gpt-flex-only": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": null, "flex": 0.25 }
                }
            }"#,
        )
        .unwrap();
        let mut pricing = PricingMap::default();
        pricing.load_json(&filtered);

        let both = pricing.find_exact("gpt-test").unwrap();
        assert_eq!(both.fast_multiplier, 2.0);
        assert_eq!(both.flex_multiplier, 0.5);
        let flex_only = pricing.find_exact("gpt-flex-only").unwrap();
        assert_eq!(flex_only.fast_multiplier, 1.0);
        assert_eq!(flex_only.flex_multiplier, 0.25);
    }

    #[test]
    fn drops_other_families_and_fields_the_loader_does_not_read() {
        let filtered = filter_pricing_json(
            r#"{
                "claude-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "litellm_provider": "anthropic",
                    "supports_vision": true
                },
                "vertex_ai/gemini-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002
                },
                "gpt-no-output": { "input_cost_per_token": 0.000001 }
            }"#,
        )
        .unwrap();

        let filtered: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        assert_eq!(
            filtered,
            serde_json::json!({
                "claude-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002
                }
            })
        );
    }
}
