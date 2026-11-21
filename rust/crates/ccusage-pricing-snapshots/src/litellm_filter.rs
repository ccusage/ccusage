use serde_json::{Map, Value};

const LOADED_FIELDS: [&str; 12] = [
    "input_cost_per_token",
    "output_cost_per_token",
    "cache_creation_input_token_cost",
    "cache_creation_input_token_cost_above_1hr",
    "cache_creation_input_token_cost_above_1hr_above_200k_tokens",
    "cache_read_input_token_cost",
    "input_cost_per_token_above_200k_tokens",
    "output_cost_per_token_above_200k_tokens",
    "cache_creation_input_token_cost_above_200k_tokens",
    "cache_read_input_token_cost_above_200k_tokens",
    "max_input_tokens",
    "litellm_provider",
];

// NOTE(2026-11-19, #2187): Provider IDs for author-operated APIs are published by LiteLLM at https://docs.litellm.ai/docs/providers/anthropic, https://docs.litellm.ai/docs/providers/openai, https://docs.litellm.ai/docs/providers/openai_text, https://docs.litellm.ai/docs/providers/deepseek, https://docs.litellm.ai/docs/providers/zai, https://docs.litellm.ai/docs/providers/moonshot, https://docs.litellm.ai/docs/providers/xai and https://docs.litellm.ai/docs/providers/dashscope. Use those IDs for direct-provider pricing.
// NOTE(2026-11-21, #2226): A 0/0 token row from Google's own `gemini` provider is never a token price: Gemma, LearnLM and the experimental ids are "Free of charge" on Google's rate-limited free tier only, with the paid tier "Not available", and Lyria is priced per song (`output_cost_per_image`) at https://ai.google.dev/gemini-api/docs/pricing. `zeroTokenRateUnpricedProviders` leaves every such row out of the catalogue, so a paid listing or the missing-pricing marker answers instead of $0.
// NOTE(2026-11-21, #2226): LiteLLM's `gemini` provider is Google's own Gemini Developer API (https://docs.litellm.ai/docs/providers/gemini), priced at https://ai.google.dev/gemini-api/docs/pricing. Its rows are Google's list price, so every Google-owned spelling such as `google/<id>` bills at them rather than at a models.dev reseller rate.
const SOURCES: &str = include_str!("litellm-sources.json");

pub(crate) fn filter_pricing_json(json: &str) -> Option<String> {
    let Value::Object(raw) = serde_json::from_str::<Value>(json).ok()? else {
        return None;
    };
    let sources: Value = serde_json::from_str(SOURCES).ok()?;
    let admissions = sources.get("embedded")?.as_array()?;
    let mut filtered = Map::new();
    for (model, pricing) in raw {
        let Value::Object(pricing) = pricing else {
            continue;
        };
        let provider = pricing.get("litellm_provider").and_then(Value::as_str);
        let mode = pricing.get("mode").and_then(Value::as_str);
        if !admissions
            .iter()
            .any(|rule| is_admitted(rule, &model, provider, mode))
        {
            continue;
        }
        let mut fields = Map::new();
        for field in LOADED_FIELDS {
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

// NOTE(2026-11-21, #2228): A vendor's free row reached through a host bills an estimate only when that host is known to charge. A LiteLLM provider or models.dev platform with at least one positive token rate is such a host, while local runtimes such as `ollama` list only $0 rows, so they stay free.
pub(crate) fn paid_host_providers(
    litellm_json: &str,
    models_dev_json: &str,
) -> Option<Vec<String>> {
    let rows = |json: &str| match serde_json::from_str::<Value>(json).ok()? {
        Value::Object(rows) => Some(rows),
        _ => None,
    };
    let positive = |rates: &Map<String, Value>, fields: [&str; 2]| {
        fields
            .iter()
            .any(|field| rates.get(*field).and_then(Value::as_f64).unwrap_or(0.0) > 0.0)
    };
    let litellm = rows(litellm_json)?;
    let models_dev = rows(models_dev_json)?;
    let litellm_hosts = litellm
        .values()
        .filter_map(Value::as_object)
        .filter_map(|row| {
            positive(row, ["input_cost_per_token", "output_cost_per_token"])
                .then(|| row.get("litellm_provider")?.as_str())
                .flatten()
        });
    let models_dev_hosts = models_dev
        .values()
        .filter_map(Value::as_object)
        .filter_map(|row| {
            row.get("cost")
                .and_then(Value::as_object)
                .is_some_and(|cost| positive(cost, ["input", "output"]))
                .then(|| row.get("provider")?.as_str())
                .flatten()
        });
    let providers = litellm_hosts
        .chain(models_dev_hosts)
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    Some(providers.into_iter().collect())
}

// NOTE(2026-11-21, #2229): A rule admits a row only when every constraint it names holds. `unqualified` admits only a key with no `/` after its `keyPrefix`, so a host's default-region listing is embedded without its regional and gateway paths, and `modes` limits a rule to token-priced listings.
fn is_admitted(rule: &Value, key: &str, provider: Option<&str>, mode: Option<&str>) -> bool {
    let field = |name| rule.get(name).and_then(Value::as_str);
    let prefix = field("keyPrefix").unwrap_or("");
    field("key").is_none_or(|expected| expected == key)
        && key.starts_with(prefix)
        && field("provider").is_none_or(|expected| provider == Some(expected))
        && (rule.get("unqualified").and_then(Value::as_bool) != Some(true)
            || !key[prefix.len()..].contains('/'))
        && rule
            .get("modes")
            .and_then(Value::as_array)
            .is_none_or(|modes| modes.iter().any(|allowed| allowed.as_str() == mode))
}

#[cfg(test)]
mod tests {
    use super::{filter_pricing_json, paid_host_providers};
    use ccusage_core::{PricingMap, pricing::MatchKind};

    #[test]
    fn admits_free_direct_rows_and_excludes_same_prefix_hosted_rows() {
        let json = serde_json::json!({
            "deepseek-chat": {"litellm_provider":"deepseek","input_cost_per_token":0.00000028,"output_cost_per_token":0.00000042},
            "deepseek/deepseek-v3": {"litellm_provider":"deepseek","input_cost_per_token":0.00000027,"output_cost_per_token":0.0000011},
            "deepseek.v3.2": {"litellm_provider":"bedrock_converse","input_cost_per_token":0.00000062,"output_cost_per_token":0.00000185},
            "deepseek-v3-2-251201": {"litellm_provider":"volcengine","input_cost_per_token":0,"output_cost_per_token":0},
            "deepseek/deepseek-test": {"litellm_provider":"deepseek","input_cost_per_token":0,"output_cost_per_token":0},
            "deepseek-vendor-test": {"litellm_provider":"reseller","input_cost_per_token":0.000001,"output_cost_per_token":0.000002}
        });
        let filtered: serde_json::Value =
            serde_json::from_str(&filter_pricing_json(&json.to_string()).unwrap()).unwrap();
        assert_eq!(
            filtered
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "deepseek-chat",
                "deepseek/deepseek-test",
                "deepseek/deepseek-v3"
            ]
        );
    }

    #[test]
    fn lists_only_providers_with_a_positive_token_rate_as_paid_hosts() {
        let json = serde_json::json!({
            "kyma/glm-4.7-flash": {"litellm_provider":"kyma","input_cost_per_token":0.000000081,"output_cost_per_token":0.00000054},
            "kyma/free-model": {"litellm_provider":"kyma","input_cost_per_token":0,"output_cost_per_token":0},
            "ollama/codellama": {"litellm_provider":"ollama","input_cost_per_token":0,"output_cost_per_token":0},
            "openrouter/out-only": {"litellm_provider":"openrouter","output_cost_per_token":0.000001},
            "sample_spec": "not a row"
        });
        let models_dev = serde_json::json!({
            "zai-org/glm-4.7-flash": {"cost": {"input": 0.07, "output": 0.4}, "provider": "novita-ai"},
            "free/model": {"cost": {"input": 0, "output": 0}, "provider": "local-runtime"}
        });
        assert_eq!(
            paid_host_providers(&json.to_string(), &models_dev.to_string()).unwrap(),
            ["kyma", "novita-ai", "openrouter"]
        );
    }

    #[test]
    fn keeps_the_two_existing_exact_key_admissions() {
        let sources: serde_json::Value = serde_json::from_str(super::SOURCES).unwrap();
        let rules = sources["embedded"].as_array().unwrap();
        let keys: Vec<_> = rules
            .iter()
            .filter_map(|rule| rule["key"].as_str())
            .collect();
        assert_eq!(keys, ["o3", "o4-mini"]);
    }

    #[test]
    fn retains_published_one_hour_cache_rates() {
        let filtered = filter_pricing_json(r#"{"claude-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002,"cache_creation_input_token_cost_above_1hr":0.000007,"cache_creation_input_token_cost_above_1hr_above_200k_tokens":0.000014}}"#).unwrap();
        let pricing: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        assert_eq!(
            pricing["claude-test"]["cache_creation_input_token_cost_above_1hr"],
            7e-6
        );
        assert_eq!(
            pricing["claude-test"]["cache_creation_input_token_cost_above_1hr_above_200k_tokens"],
            14e-6
        );
    }

    #[test]
    fn keeps_service_tier_multipliers_in_embedded_pricing() {
        ccusage_core::pricing::set_embedded_snapshots(ccusage_core::pricing::EmbeddedSnapshots {
            litellm_sources: crate::LITELLM_SOURCES,
            models_dev_catalog_rules: crate::MODELS_DEV_CATALOG_RULES,
            pricing_table: crate::PRICING_TABLE,
        });
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

        let both = pricing
            .resolve_at(None, &[("gpt-test", MatchKind::StrictExact)], None)
            .unwrap()
            .1;
        assert_eq!(both.fast_multiplier, 2.0);
        assert_eq!(both.flex_multiplier, 0.5);
        let flex_only = pricing
            .resolve_at(None, &[("gpt-flex-only", MatchKind::StrictExact)], None)
            .unwrap()
            .1;
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
                    "output_cost_per_token": 0.000002,
                    "litellm_provider": "anthropic"
                }
            })
        );
    }
}
