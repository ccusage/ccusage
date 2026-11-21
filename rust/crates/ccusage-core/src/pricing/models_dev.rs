use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::fast::FxHashMap;

use super::{
    LongContext, LongContextMode, LongContextRates, Pricing, PricingMap, Row, RowPricing, Scope,
    Source, embedded::embedded_pricing_table, model_without_date_suffix, pricing_alias,
    provider_cache_defaults, row::whole_request_threshold, speed::PublishedSpeed,
};

mod catalog;
use catalog::{
    MODELS_DEV_TRUST_OWNER, ModelsDevCatalogRules, models_dev_catalog_rules,
    normalized_models_dev_model_id,
};

pub(super) fn is_vendor_segment(segment: &str) -> bool {
    let dashed = segment.to_ascii_lowercase().replace('_', "-");
    let direct = super::embedded::litellm_sources().is_some_and(|sources| {
        sources
            .direct_providers
            .iter()
            .any(|id| *id == dashed || *id == segment)
    });
    let rules = models_dev_catalog_rules();
    direct
        || super::direct_billing_vendor(&dashed).is_some()
        || rules.is_owner(&dashed)
        || rules.is_owner(segment)
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    id: Option<String>,
    models: FxHashMap<String, ModelsDevModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ModelsDevClaim {
    trust: u8,
    has_long_context_tier: bool,
    has_cache_read: bool,
    has_cache_write: bool,
    has_context_limit: bool,
}

#[derive(Debug, Clone)]
struct ModelsDevClaimSlot {
    claim: ModelsDevClaim,
    stored_id: String,
    provider_id: String,
    source_key: String,
}

#[derive(Debug)]
enum ModelsDevJson {
    Providers(FxHashMap<String, ModelsDevProvider>),
    Models(FxHashMap<String, ModelsDevModel>),
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    id: Option<String>,
    provider: Option<String>,
    cost: Option<ModelsDevCost>,
    limit: Option<ModelsDevLimit>,
    modalities: Option<ModelsDevModalities>,
    #[serde(rename = "exactOnly")]
    exact_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    input: Option<Vec<String>>,
    output: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    tiers: Option<Vec<ModelsDevCostTier>>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevCostTier {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    tier: Option<ModelsDevTierBound>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevTierBound {
    #[serde(rename = "type")]
    kind: Option<String>,
    size: Option<u64>,
}

impl ModelsDevCost {
    fn long_context_tier(&self) -> Option<ModelsDevLongContextRates> {
        self.tiers
            .as_deref()?
            .iter()
            .filter_map(|tier| {
                let bound = tier.tier.as_ref()?;
                if bound.kind.as_deref() != Some("context") {
                    return None;
                }
                let threshold = bound.size.filter(|size| *size > 0)?;
                Some(ModelsDevLongContextRates {
                    threshold,
                    input: tier.input.map(per_token),
                    output: tier.output.map(per_token),
                    cache_create: tier.cache_write.map(per_token),
                    cache_read: tier.cache_read.map(per_token),
                })
            })
            .min_by_key(|rates| rates.threshold)
    }
}

// NOTE(2026-08-15, #1541): models.dev publishes prices per million tokens at https://models.dev/api.json. The resolver requires per-token rates.
fn per_token(per_million: f64) -> f64 {
    per_million / 1_000_000.0
}

fn models_dev_usable_cost<'a>(
    rules: &ModelsDevCatalogRules,
    source_model_id: &str,
    model: &'a ModelsDevModel,
) -> Option<&'a ModelsDevCost> {
    if !rules.is_token_priced(source_model_id, model.modalities.as_ref()) {
        return None;
    }
    let cost = model.cost.as_ref()?;
    let input = cost.input?;
    let output = cost.output?;
    (input != 0.0 || output != 0.0).then_some(cost)
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    context: Option<u64>,
}

impl PricingMap {
    pub(super) fn load_models_dev_json_missing(&mut self, json: &str) -> Option<usize> {
        let raw = parse_models_dev_json(json)?;
        let loaded = match raw {
            ModelsDevJson::Providers(providers) => {
                let rules = models_dev_catalog_rules();
                let mut ranked: Vec<_> = providers.into_iter().collect();
                let provider_id = |key: &String, provider: &ModelsDevProvider| {
                    provider.id.clone().unwrap_or_else(|| key.clone())
                };
                crate::sort::stable_by(&mut ranked, |(left_key, left), (right_key, right)| {
                    let left_id = provider_id(left_key, left);
                    let right_id = provider_id(right_key, right);
                    rules
                        .rank(&right_id)
                        .cmp(&rules.rank(&left_id))
                        .then_with(|| left_id.cmp(&right_id))
                        .then_with(|| left_key.cmp(right_key))
                });
                let mut claims: FxHashMap<String, ModelsDevClaimSlot> = FxHashMap::default();
                ranked
                    .into_iter()
                    .map(|(provider_key, provider)| {
                        let provider_id = provider.id.unwrap_or(provider_key);
                        let trust = rules.rank(&provider_id);
                        self.load_models_dev_models(
                            provider.models,
                            &provider_id,
                            trust,
                            true,
                            &mut claims,
                        )
                    })
                    .sum()
            }
            ModelsDevJson::Models(models) => {
                let mut claims = FxHashMap::default();
                self.load_models_dev_models(models, "", MODELS_DEV_TRUST_OWNER, false, &mut claims)
            }
        };
        self.curate_long_context();
        Some(loaded)
    }

    fn load_models_dev_models(
        &mut self,
        models: FxHashMap<String, ModelsDevModel>,
        provider_id: &str,
        trust: u8,
        derive_exact_only: bool,
        claims: &mut FxHashMap<String, ModelsDevClaimSlot>,
    ) -> usize {
        let rules = models_dev_catalog_rules();
        let mut loaded_count = 0;
        let mut sources: Vec<_> = models.into_iter().collect();
        crate::sort::stable_by(&mut sources, |(left, _), (right, _)| left.cmp(right));
        for (model_key, model) in sources {
            let Some(cost) = models_dev_usable_cost(rules, &model_key, &model) else {
                continue;
            };
            let declared_id = model.id.as_deref().filter(|id| !id.is_empty());
            let exact_only = model.exact_only.unwrap_or_else(|| {
                derive_exact_only
                    && rules.is_exact_only(&model_key, declared_id.unwrap_or(&model_key))
            });
            let source_key = model_key;
            let model_id = match model.id.as_ref().filter(|id| !id.is_empty()) {
                Some(id) => id.clone(),
                None => source_key.clone(),
            };
            let normalized_id = normalized_models_dev_model_id(&model_id).into_owned();
            let claimed = claims.get(&normalized_id).cloned();
            if claimed.is_none() && self.rows.entries.contains_key(&model_id) {
                continue;
            }
            let Some(input) = cost.input else {
                continue;
            };
            let Some(output) = cost.output else {
                continue;
            };
            let context_limit = model.limit.as_ref().and_then(|limit| limit.context);
            let long_context = cost.long_context_tier();
            let claim = ModelsDevClaim {
                trust,
                has_long_context_tier: long_context.is_some(),
                has_cache_read: cost.cache_read.is_some(),
                has_cache_write: cost.cache_write.is_some(),
                has_context_limit: context_limit.is_some(),
            };
            let replaces_equal_claim = |previous: &ModelsDevClaimSlot| {
                previous.claim == claim
                    && previous.provider_id == provider_id
                    && source_key < previous.source_key
            };
            if claimed.as_ref().is_some_and(|claimed| {
                claimed.claim > claim || (claimed.claim == claim && !replaces_equal_claim(claimed))
            }) {
                continue;
            }
            if let Some(previous) = claimed
                .as_ref()
                .filter(|previous| previous.stored_id != model_id)
            {
                self.rows_mut().entries.remove(&previous.stored_id);
            }
            self.rows_mut().entries.insert(
                model_id.clone(),
                Row {
                    pricing: RowPricing::by_key(
                        Pricing {
                            input: per_token(input),
                            output: per_token(output),
                            cache_create: cost.cache_write.map(per_token),
                            cache_read: cost.cache_read.map(per_token),
                            long_context: long_context.map(|rates| LongContext {
                                threshold: rates.threshold,
                                mode: LongContextMode::WholeRequest,
                                rates: LongContextRates {
                                    input: rates.input,
                                    output: rates.output,
                                    cache_create: rates.cache_create,
                                    cache_read: rates.cache_read,
                                    cache_create_1h: None,
                                },
                            }),
                            ..Pricing::empty()
                        },
                        PublishedSpeed::default(),
                        provider_cache_defaults(None),
                    ),
                    exact_only,
                    whole_request_tiers: whole_request_threshold(if provider_id.is_empty() {
                        model.provider.as_deref().unwrap_or_default()
                    } else {
                        provider_id
                    })
                    .is_some(),
                    context_limit,
                    key: Arc::default(),
                    source: Source::ModelsDev,
                    scope: Scope::Direct,
                    vendor_start: None,
                    host: None,
                    schedule: None,
                },
            );
            if claims
                .insert(
                    normalized_id,
                    ModelsDevClaimSlot {
                        claim,
                        stored_id: model_id,
                        provider_id: provider_id.to_string(),
                        source_key,
                    },
                )
                .is_none()
            {
                loaded_count += 1;
            }
        }
        self.clear_find_cache();
        loaded_count
    }

    pub(super) fn fill_long_context_rates_from_models_dev(&mut self, tiers: &Self) {
        for row in &mut self.rows_mut().entries.rows {
            let model = &row.key;
            let pricing = &mut row.pricing;
            let previous = pricing
                .long_context
                .as_deref()
                .map(|context| context.rates)
                .unwrap_or_default();
            if [
                previous.input,
                previous.output,
                previous.cache_create,
                previous.cache_read,
            ]
            .iter()
            .any(Option::is_some)
            {
                continue;
            }
            let Some(mut context) = tiers.long_context_pricing(model) else {
                continue;
            };
            context.rates.cache_create_1h = previous.cache_create_1h;
            pricing.long_context = Some(Arc::new(context));
        }
        self.clear_find_cache();
    }

    fn long_context_pricing(&self, model: &str) -> Option<LongContext> {
        let base = model_without_date_suffix(model);
        let resolved = pricing_alias(base).unwrap_or(base);
        std::iter::once(resolved)
            .chain((base != resolved).then_some(base))
            .find_map(|key| {
                self.rows
                    .entries
                    .get(key)
                    .or_else(|| {
                        let exact_only = self.rows.exact_only_id(key)?;
                        self.rows.entries.get(exact_only)
                    })
                    .map(|row| row.pricing.long_context.as_deref().copied())
            })
            .flatten()
    }
}

fn parse_models_dev_json(json: &str) -> Option<ModelsDevJson> {
    let value = serde_json::from_str::<Value>(json).ok()?;
    let Value::Object(entries) = &value else {
        return None;
    };
    if entries.values().any(models_dev_entry_has_models_field) {
        if !entries.values().all(models_dev_entry_has_models_field) {
            return None;
        }
        return serde_json::from_value::<FxHashMap<String, ModelsDevProvider>>(value)
            .ok()
            .map(ModelsDevJson::Providers);
    }
    if !entries.values().all(models_dev_entry_has_required_cost) {
        return None;
    }
    serde_json::from_value::<FxHashMap<String, ModelsDevModel>>(value)
        .ok()
        .map(ModelsDevJson::Models)
}

fn models_dev_entry_has_models_field(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|entry| entry.get("models").is_some_and(Value::is_object))
}

fn models_dev_entry_has_required_cost(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|entry| entry.get("cost"))
        .and_then(Value::as_object)
        .is_some_and(|cost| {
            cost.get("input").is_some_and(Value::is_number)
                && cost.get("output").is_some_and(Value::is_number)
        })
}

#[derive(Clone, Copy)]
struct ModelsDevLongContextRates {
    threshold: u64,
    input: Option<f64>,
    output: Option<f64>,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
}

pub(super) fn embedded_models_dev_pricing() -> &'static PricingMap {
    &embedded_pricing_table().models_dev
}
