use std::{
    borrow::Cow,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use ccusage_cli::PricingOverride;

use serde::Deserialize;
use serde_json::Value;

use crate::fast::FxHashMap;

const BUILD_TIME_PRICING_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/litellm-pricing.json"));
const BUILD_TIME_MODELS_DEV_JSON: &str = include_str!("models-dev-pricing.json");
const FAST_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("fast-multiplier-overrides.json");
const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const PRICING_FETCH_TIMEOUT_SECONDS: u64 = 10;
const PRICING_FETCH_MAX_BYTES: u64 = 64 * 1024 * 1024;
const MODELS_DEV_FAILURE_RETRY_AFTER: Duration = Duration::from_secs(60);
// Anthropic date-suffixed model aliases use YYYYMMDD, while other numeric
// suffixes are treated as distinct model versions.
const MODEL_DATE_SUFFIX_DIGITS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pricing {
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_create: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_read_explicit: bool,
    pub(crate) input_above_200k: Option<f64>,
    pub(crate) output_above_200k: Option<f64>,
    pub(crate) cache_create_above_200k: Option<f64>,
    pub(crate) cache_read_above_200k: Option<f64>,
    // Token count above which the `*_above_200k` rates apply. The field names
    // keep the LiteLLM `_above_200k_tokens` suffix for JSON compatibility, but
    // some providers switch tiers at a different point (OpenAI long-context
    // pricing starts above 272K input tokens), so the threshold is per model.
    pub(crate) long_context_threshold: Option<u64>,
    pub(crate) fast_multiplier: f64,
}

/// Default tier boundary for LiteLLM `*_above_200k_tokens` pricing fields.
pub(crate) const DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 200_000;

/// OpenAI long-context pricing boundary: requests with more than 272K input
/// tokens (GPT-5's maximum short-context input size) are billed at
/// long-context rates.
pub(crate) const OPENAI_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 272_000;

impl Pricing {
    pub(crate) const fn empty() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_create: 0.0,
            cache_read: 0.0,
            cache_read_explicit: false,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PricingMap {
    entries: FxHashMap<String, Pricing>,
    context_limits: FxHashMap<String, u64>,
    enable_models_dev_fallback: bool,
    enable_embedded_models_dev_fallback: bool,
    find_cache: OnceLock<Mutex<FxHashMap<String, Option<Pricing>>>>,
}

impl Default for PricingMap {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
            context_limits: FxHashMap::default(),
            enable_models_dev_fallback: false,
            enable_embedded_models_dev_fallback: false,
            find_cache: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LiteLlmPricing {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    input_cost_per_token_above_200k_tokens: Option<f64>,
    output_cost_per_token_above_200k_tokens: Option<f64>,
    cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    max_input_tokens: Option<u64>,
    provider_specific_entry: Option<ProviderSpecificEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderSpecificEntry {
    fast: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CompactLiteLlmPricing {
    i: f64,
    o: f64,
    cc: Option<f64>,
    cr: Option<f64>,
    ia: Option<f64>,
    oa: Option<f64>,
    cca: Option<f64>,
    cra: Option<f64>,
    ctx: Option<u64>,
    fast: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    models: FxHashMap<String, ModelsDevModel>,
}

#[derive(Debug)]
enum ModelsDevJson {
    Providers(FxHashMap<String, ModelsDevProvider>),
    Models(FxHashMap<String, ModelsDevModel>),
}

struct ModelsDevPricingCache {
    pricing: OnceLock<PricingMap>,
    last_failure: Mutex<Option<Instant>>,
    failure_retry_after: Duration,
}

impl ModelsDevPricingCache {
    const fn new(failure_retry_after: Duration) -> Self {
        Self {
            pricing: OnceLock::new(),
            last_failure: Mutex::new(None),
            failure_retry_after,
        }
    }

    fn get_or_try_load<F>(&self, fetch_json: F) -> Option<&PricingMap>
    where
        F: FnOnce() -> std::io::Result<String>,
    {
        if let Some(pricing) = self.pricing.get() {
            return Some(pricing);
        }
        if self.last_failure.lock().is_ok_and(|last_failure| {
            last_failure.is_some_and(|failed_at| failed_at.elapsed() < self.failure_retry_after)
        }) {
            return None;
        }

        let Some(map) = load_models_dev_pricing(fetch_json) else {
            if let Ok(mut last_failure) = self.last_failure.lock() {
                *last_failure = Some(Instant::now());
            }
            return None;
        };
        let _ = self.pricing.set(map);
        if let Ok(mut last_failure) = self.last_failure.lock() {
            *last_failure = None;
        }
        self.pricing.get()
    }
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    id: Option<String>,
    cost: Option<ModelsDevCost>,
    limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    context: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct FastMultiplierOverrides {
    exact: FxHashMap<String, f64>,
    normalized_prefix: FxHashMap<String, f64>,
}

impl FastMultiplierOverrides {
    fn load() -> Self {
        serde_json::from_str(FAST_MULTIPLIER_OVERRIDES_JSON)
            .expect("parse embedded fast-multiplier-overrides.json")
    }

    fn multiplier_for(&self, model: &str) -> Option<f64> {
        if let Some(multiplier) = self.exact.get(model) {
            return Some(*multiplier);
        }
        let normalized = model.replace(['.', '@'], "-");
        normalized.split(['/', ':']).find_map(|part| {
            self.normalized_prefix
                .iter()
                .find_map(|(base, multiplier)| {
                    matches_model_suffix(part, base).then_some(*multiplier)
                })
        })
    }
}

impl PricingMap {
    pub(crate) fn load_embedded() -> Self {
        let mut map = Self::default();
        let fast_multiplier_overrides = FastMultiplierOverrides::load();
        map.load_json_with_overrides(BUILD_TIME_PRICING_JSON, &fast_multiplier_overrides);
        map.put_builtin_pricing(&fast_multiplier_overrides);
        map.apply_builtin_long_context_rates();
        // Resolve models that LiteLLM and the built-in table miss from the
        // embedded models.dev snapshot. This works offline, unlike the network
        // source gated by `enable_models_dev_fallback`.
        map.enable_embedded_models_dev_fallback = true;
        map
    }

    pub(crate) fn load_with_overrides<'a, I>(offline: bool, log: bool, overrides: I) -> Self
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        let mut map = Self::load_embedded();
        if !offline {
            let fetch_result = crate::progress::track_status(
                log && crate::progress::usage_load_output_is_tty(),
                "Refreshing model pricing from LiteLLM...",
                fetch_pricing_json,
            );

            match fetch_result {
                Ok(json) => {
                    let loaded_count = map.load_json(&json);
                    if loaded_count == 0 && should_log_pricing_refresh_details() {
                        eprintln!("WARN  Failed to parse LiteLLM pricing; using embedded pricing.");
                    }
                }
                Err(error) => {
                    if should_log_pricing_refresh_details() {
                        eprintln!(
                            "WARN  Failed to fetch LiteLLM pricing ({error}); using embedded pricing."
                        );
                    }
                }
            }
        }

        // A live LiteLLM refresh replaces whole entries, so re-apply the
        // built-in long-context rates it does not publish before user
        // overrides get the final word.
        map.apply_builtin_long_context_rates();
        map.enable_models_dev_fallback = !offline;
        map.apply_overrides(overrides);
        map
    }

    pub(crate) fn load_json(&mut self, json: &str) -> usize {
        let fast_multiplier_overrides = FastMultiplierOverrides::load();
        self.load_json_with_overrides(json, &fast_multiplier_overrides)
    }

    fn load_json_with_overrides(
        &mut self,
        json: &str,
        fast_multiplier_overrides: &FastMultiplierOverrides,
    ) -> usize {
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
            let context_limit = pricing.max_input_tokens;
            let cache_read_explicit = pricing.cache_read_input_token_cost.is_some();
            let fast_multiplier = pricing
                .provider_specific_entry
                .and_then(|entry| entry.fast)
                .or_else(|| fast_multiplier_overrides.multiplier_for(&model))
                .unwrap_or(1.0);
            self.entries.insert(
                model.clone(),
                Pricing {
                    input,
                    output,
                    cache_create: pricing
                        .cache_creation_input_token_cost
                        .unwrap_or(input * 1.25),
                    cache_read: pricing.cache_read_input_token_cost.unwrap_or(input * 0.1),
                    cache_read_explicit,
                    input_above_200k: pricing.input_cost_per_token_above_200k_tokens,
                    output_above_200k: pricing.output_cost_per_token_above_200k_tokens,
                    cache_create_above_200k: pricing
                        .cache_creation_input_token_cost_above_200k_tokens,
                    cache_read_above_200k: pricing.cache_read_input_token_cost_above_200k_tokens,
                    long_context_threshold: None,
                    fast_multiplier,
                },
            );
            if let Some(context_limit) = context_limit {
                self.context_limits.insert(model, context_limit);
            }
            loaded_count += 1;
        }
        self.clear_find_cache();
        loaded_count
    }

    fn load_models_dev_json_missing(&mut self, json: &str) -> Option<usize> {
        let raw = parse_models_dev_json(json)?;
        Some(match raw {
            ModelsDevJson::Providers(providers) => providers
                .into_values()
                .map(|provider| self.load_models_dev_models(provider.models))
                .sum(),
            ModelsDevJson::Models(models) => self.load_models_dev_models(models),
        })
    }

    fn load_models_dev_models(&mut self, models: FxHashMap<String, ModelsDevModel>) -> usize {
        let mut loaded_count = 0;
        for (model_key, model) in models {
            let model_id = model.id.unwrap_or(model_key);
            if self.entries.contains_key(&model_id) {
                continue;
            }
            let Some(cost) = model.cost else {
                continue;
            };
            let Some(input) = cost.input else {
                continue;
            };
            let Some(output) = cost.output else {
                continue;
            };
            let input = input / 1_000_000.0;
            let output = output / 1_000_000.0;
            let cache_read_explicit = cost.cache_read.is_some();
            self.entries.insert(
                model_id.clone(),
                Pricing {
                    input,
                    output,
                    cache_create: cost
                        .cache_write
                        .map(|value| value / 1_000_000.0)
                        .unwrap_or(input * 1.25),
                    cache_read: cost
                        .cache_read
                        .map(|value| value / 1_000_000.0)
                        .unwrap_or(input * 0.1),
                    cache_read_explicit,
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                },
            );
            if let Some(context_limit) = model.limit.and_then(|limit| limit.context) {
                self.context_limits.insert(model_id, context_limit);
            }
            loaded_count += 1;
        }
        self.clear_find_cache();
        loaded_count
    }

    pub(crate) fn find(&self, model: &str) -> Option<Pricing> {
        // Fast path: check the model-level cache first. When the same model
        // name is looked up repeatedly (e.g. across thousands of entries with
        // only a few dozen unique models), the cache avoids re-running the
        // expensive fuzzy fallback on every call.
        {
            let cache = self
                .find_cache
                .get_or_init(|| Mutex::new(FxHashMap::default()));
            let guard = cache.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(&cached) = guard.get(model) {
                return cached;
            }
        }
        // Full lookup (dropped the lock above so concurrent callers are not
        // serialized on the expensive fuzzy path).
        let alias = crate::model_aliases::resolve_model_name(model);
        let resolved_alias = alias.as_ref();
        let result = self
            .find_entry_or_alias(model)
            .or_else(|| {
                (resolved_alias != model)
                    .then(|| self.find_entry_or_alias(resolved_alias))
                    .flatten()
            })
            .or_else(|| {
                self.enable_models_dev_fallback
                    .then(|| {
                        models_dev_pricing()
                            .and_then(|pricing| pricing.find_entry_or_alias(resolved_alias))
                    })
                    .flatten()
            })
            // The embedded models.dev snapshot is a separate map, so it only
            // resolves models the primary table misses and never perturbs its
            // fuzzy alias matching. It works offline, unlike the network source.
            .or_else(|| {
                self.enable_embedded_models_dev_fallback
                    .then(|| embedded_models_dev_pricing().find_entry_or_alias(resolved_alias))
                    .flatten()
            });
        // Store the result (including None for misses) so repeated lookups
        // for the same model that fails to match any pricing entry are also
        // short-circuited.
        let cache = self
            .find_cache
            .get_or_init(|| Mutex::new(FxHashMap::default()));
        let mut guard = cache.lock().unwrap_or_else(|error| error.into_inner());
        guard.insert(model.to_string(), result);
        result
    }

    pub(crate) fn find_exact(&self, model: &str) -> Option<Pricing> {
        self.entries.get(model).copied()
    }

    fn find_entry_or_alias(&self, model: &str) -> Option<Pricing> {
        self.find_entry(model)
            .or_else(|| pricing_alias(model).and_then(|alias| self.find_entry(alias)))
    }

    fn find_entry(&self, model: &str) -> Option<Pricing> {
        self.entries.get(model).copied().or_else(|| {
            let normalized_model = normalized_pricing_key(model);
            self.entries
                .iter()
                .filter(|(candidate, _)| {
                    pricing_key_matches(candidate, model, normalized_model.as_ref())
                })
                .max_by(|(left, _), (right, _)| {
                    left.len().cmp(&right.len()).then_with(|| right.cmp(left))
                })
                .map(|(_, pricing)| *pricing)
        })
    }

    pub(crate) fn context_limit(&self, model: &str) -> Option<u64> {
        let alias = crate::model_aliases::resolve_model_name(model);
        let resolved_alias = alias.as_ref();
        self.context_limit_entry_or_alias(model)
            .or_else(|| {
                (resolved_alias != model)
                    .then(|| self.context_limit_entry_or_alias(resolved_alias))
                    .flatten()
            })
            .or_else(|| {
                self.enable_models_dev_fallback
                    .then(|| {
                        models_dev_pricing().and_then(|pricing| {
                            pricing.context_limit_entry_or_alias(resolved_alias)
                        })
                    })
                    .flatten()
            })
            .or_else(|| {
                self.enable_embedded_models_dev_fallback
                    .then(|| {
                        embedded_models_dev_pricing().context_limit_entry_or_alias(resolved_alias)
                    })
                    .flatten()
            })
    }

    fn context_limit_entry_or_alias(&self, model: &str) -> Option<u64> {
        self.context_limit_entry(model)
            .or_else(|| pricing_alias(model).and_then(|alias| self.context_limit_entry(alias)))
    }

    fn context_limit_entry(&self, model: &str) -> Option<u64> {
        self.context_limits.get(model).copied().or_else(|| {
            let normalized_model = normalized_pricing_key(model);
            self.context_limits
                .iter()
                .filter(|(candidate, _)| {
                    pricing_key_matches(candidate, model, normalized_model.as_ref())
                })
                .max_by(|(left, _), (right, _)| {
                    left.len().cmp(&right.len()).then_with(|| right.cmp(left))
                })
                .map(|(_, context_limit)| *context_limit)
        })
    }

    pub(crate) fn apply_overrides<'a, I>(&mut self, overrides: I)
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        for (model, override_value) in overrides {
            self.apply_override(model, override_value);
        }
        self.clear_find_cache();
    }

    fn apply_override(&mut self, model: &str, override_value: &PricingOverride) {
        let base = self
            .entries
            .get(model)
            .copied()
            .unwrap_or_else(Pricing::empty);

        let new_input = override_value.input_cost_per_token.unwrap_or(base.input);

        // When input cost is overridden but cache fields are not explicitly provided,
        // and the base cache values were derived from input (indicated by
        // !cache_read_explicit), scale cache costs proportionally by
        // new_input / old_input. When cache_read_explicit is true, the base cache
        // values were independently set (from LiteLLM data or a prior override),
        // so preserve them unchanged.
        let should_scale = override_value.input_cost_per_token.is_some()
            && base.input > 0.0
            && !base.cache_read_explicit;
        let scale = if should_scale {
            new_input / base.input
        } else {
            1.0
        };

        let cache_create = if let Some(value) = override_value.cache_creation_input_token_cost {
            value
        } else if should_scale && base.cache_create > 0.0 {
            base.cache_create * scale
        } else {
            base.cache_create
        };

        let cache_read = if let Some(value) = override_value.cache_read_input_token_cost {
            value
        } else if should_scale && base.cache_read > 0.0 {
            base.cache_read * scale
        } else {
            base.cache_read
        };

        let cache_create_above_200k = if override_value
            .cache_creation_input_token_cost_above_200k_tokens
            .is_some()
        {
            override_value.cache_creation_input_token_cost_above_200k_tokens
        } else if should_scale {
            base.cache_create_above_200k.map(|v| v * scale)
        } else {
            base.cache_create_above_200k
        };

        let cache_read_above_200k = if override_value
            .cache_read_input_token_cost_above_200k_tokens
            .is_some()
        {
            override_value.cache_read_input_token_cost_above_200k_tokens
        } else if should_scale {
            base.cache_read_above_200k.map(|v| v * scale)
        } else {
            base.cache_read_above_200k
        };

        let pricing = Pricing {
            input: new_input,
            output: override_value.output_cost_per_token.unwrap_or(base.output),
            cache_create,
            cache_read,
            cache_read_explicit: override_value.cache_read_input_token_cost.is_some()
                || base.cache_read_explicit,
            input_above_200k: override_value
                .input_cost_per_token_above_200k_tokens
                .or(base.input_above_200k),
            output_above_200k: override_value
                .output_cost_per_token_above_200k_tokens
                .or(base.output_above_200k),
            cache_create_above_200k,
            cache_read_above_200k,
            long_context_threshold: base.long_context_threshold,
            fast_multiplier: override_value
                .fast_multiplier
                .unwrap_or(base.fast_multiplier),
        };

        self.entries.insert(model.to_string(), pricing);
        if let Some(limit) = override_value.max_input_tokens {
            self.context_limits.insert(model.to_string(), limit);
        }
    }

    fn clear_find_cache(&self) {
        if let Some(cache) = self.find_cache.get() {
            let mut guard = cache.lock().unwrap_or_else(|error| error.into_inner());
            guard.clear();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn models_dev_fallback_enabled(&self) -> bool {
        self.enable_models_dev_fallback
    }

    /// Fills in long-context tier rates for models whose upstream pricing
    /// entries only carry the flat rates. Runs after every pricing load
    /// (embedded and live) because LiteLLM refreshes replace whole entries.
    /// Entries that already carry any tier rate are left untouched so
    /// upstream data wins once it exists. The check is deliberately
    /// all-or-nothing rather than per field: upstream `*_above_200k_tokens`
    /// values assume the 200K boundary, so mixing them with built-in rates
    /// that assume the OpenAI 272K boundary would price both tiers wrong.
    fn apply_builtin_long_context_rates(&mut self) {
        for (model, pricing) in &mut self.entries {
            if pricing.input_above_200k.is_some()
                || pricing.output_above_200k.is_some()
                || pricing.cache_create_above_200k.is_some()
                || pricing.cache_read_above_200k.is_some()
            {
                continue;
            }
            let Some(rates) = builtin_long_context_rates(model_without_date_suffix(model)) else {
                continue;
            };
            pricing.input_above_200k = rates.input;
            pricing.output_above_200k = rates.output;
            pricing.cache_create_above_200k = rates.cache_create;
            pricing.cache_read_above_200k = rates.cache_read;
            pricing.long_context_threshold = Some(rates.threshold);
        }
        self.clear_find_cache();
    }

    fn put_builtin_pricing(&mut self, fast_multiplier_overrides: &FastMultiplierOverrides) {
        self.entries.insert(
            "claude-opus-4-5".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-opus-4-6".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("claude-opus-4-6")
                    .unwrap_or(1.0),
            },
        );
        self.entries.insert(
            "claude-opus-4-7".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("claude-opus-4-7")
                    .unwrap_or(1.0),
            },
        );
        self.entries.insert(
            "claude-opus-4-8".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("claude-opus-4-8")
                    .unwrap_or(1.0),
            },
        );
        self.entries.insert(
            "claude-haiku-4-5".to_string(),
            Pricing {
                input: 1e-6,
                output: 5e-6,
                cache_create: 1.25e-6,
                cache_read: 0.1e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-sonnet-4-6".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-sonnet-4".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                input_above_200k: Some(6e-6),
                output_above_200k: Some(22.5e-6),
                cache_create_above_200k: Some(7.5e-6),
                cache_read_above_200k: Some(0.6e-6),
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        let claude_3_5_haiku = Pricing {
            input: 0.8e-6,
            output: 4e-6,
            cache_create: 1.0e-6,
            cache_read: 0.08e-6,
            cache_read_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.entries
            .insert("claude-3-5-haiku".to_string(), claude_3_5_haiku);
        self.entries
            .insert("claude-3-5-haiku-20241022".to_string(), claude_3_5_haiku);
        self.entries.insert(
            "claude-3-opus".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-3-sonnet".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "claude-3-haiku".to_string(),
            Pricing {
                input: 0.25e-6,
                output: 1.25e-6,
                cache_create: 0.3e-6,
                cache_read: 0.03e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "gpt-5".to_string(),
            Pricing {
                input: 1.25e-6,
                output: 10e-6,
                cache_create: 1.25e-6,
                cache_read: 0.125e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "gpt-5.5".to_string(),
            Pricing {
                input: 5e-6,
                output: 30e-6,
                cache_create: 5e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("gpt-5.5")
                    .unwrap_or(1.0),
            },
        );
        self.entries.insert(
            "grok-4.3".to_string(),
            Pricing {
                input: 1.25e-6,
                output: 2.5e-6,
                cache_create: 1.25e-6,
                cache_read: 0.125e-6,
                cache_read_explicit: false,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.kimi.ai/docs/pricing/chat-k25
        self.entries.insert(
            "moonshot/kimi-k2.5".to_string(),
            Pricing {
                input: 0.6e-6,
                output: 3e-6,
                cache_create: 0.75e-6,
                cache_read: 0.1e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.kimi.ai/docs/pricing/chat-k26
        self.entries.insert(
            "moonshot/kimi-k2.6".to_string(),
            Pricing {
                input: 0.95e-6,
                output: 4e-6,
                cache_create: 1.1875e-6,
                cache_read: 0.16e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        let gpt_5_1_pricing = Pricing {
            input: 1.25e-6,
            output: 10e-6,
            cache_create: 1.25e-6,
            cache_read: 0.125e-6,
            cache_read_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.entries.insert("gpt-5.1".to_string(), gpt_5_1_pricing);
        self.entries
            .insert("gpt-5.1-codex".to_string(), gpt_5_1_pricing);
        let gpt_5_codex_pricing = Pricing {
            input: 1.75e-6,
            output: 14e-6,
            cache_create: 1.75e-6,
            cache_read: 0.175e-6,
            cache_read_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.entries
            .insert("gpt-5.2-codex".to_string(), gpt_5_codex_pricing);
        self.entries.insert(
            "gpt-5.3-codex".to_string(),
            Pricing {
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("gpt-5.3-codex")
                    .unwrap_or(1.0),
                ..gpt_5_codex_pricing
            },
        );
        self.entries
            .insert("gpt-5.2".to_string(), gpt_5_codex_pricing);
        self.entries.insert(
            "gpt-5.4".to_string(),
            Pricing {
                input: 2.5e-6,
                output: 15e-6,
                cache_create: 2.5e-6,
                cache_read: 0.25e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: fast_multiplier_overrides
                    .multiplier_for("gpt-5.4")
                    .unwrap_or(1.0),
            },
        );
        self.entries.insert(
            "gpt-5.4-mini".to_string(),
            Pricing {
                input: 0.75e-6,
                output: 4.5e-6,
                cache_create: 0.75e-6,
                cache_read: 0.075e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.entries.insert(
            "gpt-5.4-nano".to_string(),
            Pricing {
                input: 0.2e-6,
                output: 1.25e-6,
                cache_create: 0.2e-6,
                cache_read: 0.02e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.openai.com/docs/pricing (Standard tier,
        // short context). The long-context tier rates live in
        // `builtin_long_context_rates`, which runs after every pricing load.
        for (model, input, output, cache_create, cache_read) in [
            ("gpt-5.6-sol", 5e-6, 30e-6, 6.25e-6, 0.5e-6),
            ("gpt-5.6-terra", 2.5e-6, 15e-6, 3.125e-6, 0.25e-6),
            ("gpt-5.6-luna", 1e-6, 6e-6, 1.25e-6, 0.1e-6),
        ] {
            self.entries.insert(
                model.to_string(),
                Pricing {
                    input,
                    output,
                    cache_create,
                    cache_read,
                    cache_read_explicit: true,
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: fast_multiplier_overrides
                        .multiplier_for(model)
                        .unwrap_or(1.0),
                },
            );
        }
        // Source: https://docs.z.ai/guides/overview/pricing
        let glm_pricing = |input: f64, output: f64, cache_read: f64| Pricing {
            input,
            output,
            cache_create: 0.0,
            cache_read,
            cache_read_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        let glm_base = glm_pricing(0.6e-6, 2.2e-6, 0.11e-6);
        self.entries.insert("glm-4.5".to_string(), glm_base);
        self.entries.insert("zai/glm-4.5".to_string(), glm_base);
        self.entries.insert(
            "zai/glm-4.5-x".to_string(),
            glm_pricing(2.2e-6, 8.9e-6, 0.45e-6),
        );
        self.entries.insert(
            "zai/glm-4.5-air".to_string(),
            glm_pricing(0.2e-6, 1.1e-6, 0.03e-6),
        );
        self.entries.insert(
            "zai/glm-4.5-airx".to_string(),
            glm_pricing(1.1e-6, 4.5e-6, 0.22e-6),
        );
        self.entries.insert(
            "zai/glm-4.5v".to_string(),
            glm_pricing(0.6e-6, 1.8e-6, 0.11e-6),
        );
        self.entries.insert(
            "zai/glm-4-32b-0414-128k".to_string(),
            glm_pricing(0.1e-6, 0.1e-6, 0.0),
        );
        self.entries
            .insert("zai/glm-4.5-flash".to_string(), glm_pricing(0.0, 0.0, 0.0));
        self.entries.insert("glm-4.6".to_string(), glm_base);
        self.entries.insert("glm-4.7".to_string(), glm_base);
        self.entries.insert(
            "glm-5".to_string(),
            Pricing {
                input: 1.0e-6,
                output: 3.2e-6,
                cache_read: 0.2e-6,
                ..glm_base
            },
        );
        self.entries.insert(
            "glm-5-turbo".to_string(),
            Pricing {
                input: 1.2e-6,
                output: 4.0e-6,
                cache_read: 0.24e-6,
                ..glm_base
            },
        );
        self.entries.insert(
            "glm-5.1".to_string(),
            Pricing {
                input: 1.4e-6,
                output: 4.4e-6,
                cache_read: 0.26e-6,
                ..glm_base
            },
        );
        self.context_limits.insert("gpt-5.5".to_string(), 1_050_000);
        self.context_limits
            .insert("grok-4.3".to_string(), 1_000_000);
        self.context_limits.insert("gpt-5.4".to_string(), 1_050_000);
        // The gpt-5.6 family shares the 1,050,000-token window of the other
        // long-context GPT-5 flagship models until upstream data lands.
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            self.context_limits.insert(model.to_string(), 1_050_000);
        }
        for model in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
        ] {
            self.context_limits.insert(model.to_string(), 1_000_000);
        }
        self.context_limits
            .insert("moonshot/kimi-k2.5".to_string(), 262_144);
        self.context_limits
            .insert("moonshot/kimi-k2.6".to_string(), 262_144);

        for model in [
            "claude-opus-4-5",
            "claude-haiku-4-5",
            "claude-opus-4",
            "claude-sonnet-4",
            "claude-3-5-haiku",
            "claude-3-5-haiku-20241022",
            "claude-3-opus",
            "claude-3-sonnet",
            "claude-3-haiku",
        ] {
            self.context_limits.insert(model.to_string(), 200_000);
        }
    }
}

fn parse_litellm_pricing(value: Value) -> Option<LiteLlmPricing> {
    if value
        .as_object()
        .is_some_and(|entry| entry.contains_key("i") && entry.contains_key("o"))
        && let Ok(compact) = serde_json::from_value::<CompactLiteLlmPricing>(value.clone())
    {
        return Some(LiteLlmPricing {
            input_cost_per_token: Some(compact.i),
            output_cost_per_token: Some(compact.o),
            cache_creation_input_token_cost: compact.cc,
            cache_read_input_token_cost: compact.cr,
            input_cost_per_token_above_200k_tokens: compact.ia,
            output_cost_per_token_above_200k_tokens: compact.oa,
            cache_creation_input_token_cost_above_200k_tokens: compact.cca,
            cache_read_input_token_cost_above_200k_tokens: compact.cra,
            max_input_tokens: compact.ctx,
            provider_specific_entry: compact
                .fast
                .map(|fast| ProviderSpecificEntry { fast: Some(fast) }),
        });
    }
    let pricing = serde_json::from_value::<LiteLlmPricing>(value).ok()?;
    pricing
        .input_cost_per_token
        .zip(pricing.output_cost_per_token)
        .map(|_| pricing)
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

/// Matches pricing keys across provider/model aliases while preserving version boundaries.
fn pricing_key_matches(candidate: &str, model: &str, normalized_model: &str) -> bool {
    if contains_pricing_key(model, candidate) || contains_pricing_key(candidate, model) {
        return true;
    }
    let normalized_candidate = normalized_pricing_key(candidate);
    contains_pricing_key(normalized_model, normalized_candidate.as_ref())
        || contains_pricing_key(normalized_candidate.as_ref(), normalized_model)
}

/// Finds a key only when the surrounding bytes are non-alphanumeric boundaries.
fn contains_pricing_key(value: &str, key: &str) -> bool {
    value.match_indices(key).any(|(index, _)| {
        let before = index
            .checked_sub(1)
            .and_then(|before| value.as_bytes().get(before))
            .copied();
        let suffix = &value[index + key.len()..];
        before.is_none_or(is_pricing_key_boundary) && suffix_allows_pricing_key_match(key, suffix)
    })
}

/// Treats punctuation separators as boundaries, but not adjacent version digits.
fn is_pricing_key_boundary(byte: u8) -> bool {
    !byte.is_ascii_alphanumeric()
}

fn suffix_allows_pricing_key_match(key: &str, suffix: &str) -> bool {
    let Some(separator) = suffix.as_bytes().first().copied() else {
        return true;
    };
    if !is_pricing_key_boundary(separator) {
        return false;
    }
    !suffix_starts_with_numeric_model_version(key, suffix)
}

fn suffix_starts_with_numeric_model_version(key: &str, suffix: &str) -> bool {
    if !key.as_bytes().last().is_some_and(u8::is_ascii_digit) {
        return false;
    }
    if !matches!(suffix.as_bytes().first(), Some(b'-' | b'.')) {
        return false;
    }

    let rest = &suffix[1..];
    let digit_len = rest
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return false;
    }
    let after_digits = rest.as_bytes().get(digit_len).copied();
    !(digit_len == MODEL_DATE_SUFFIX_DIGITS && after_digits.is_none_or(is_pricing_key_boundary))
}

/// Normalizes known model separator variants without allocating for canonical keys.
fn normalized_pricing_key(value: &str) -> Cow<'_, str> {
    if value.contains(['.', '@']) {
        Cow::Owned(value.replace(['.', '@'], "-"))
    } else {
        Cow::Borrowed(value)
    }
}

/// Long-context tier rates (per token) for a base model, applied on top of
/// loaded pricing entries by `apply_builtin_long_context_rates`.
#[derive(Clone, Copy)]
struct LongContextRates {
    threshold: u64,
    input: Option<f64>,
    output: Option<f64>,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
}

/// Built-in two-stage rates that upstream pricing sources do not publish.
/// Source: https://platform.openai.com/docs/pricing (Standard tier); OpenAI
/// bills requests with more than 272K input tokens at long-context rates.
fn builtin_long_context_rates(base_model: &str) -> Option<LongContextRates> {
    let openai = |input: f64, output: f64, cache_create: Option<f64>, cache_read: Option<f64>| {
        LongContextRates {
            threshold: OPENAI_LONG_CONTEXT_THRESHOLD_TOKENS,
            input: Some(input),
            output: Some(output),
            cache_create,
            cache_read,
        }
    };
    match base_model {
        "gpt-5.6-sol" => Some(openai(10e-6, 45e-6, Some(12.5e-6), Some(1e-6))),
        "gpt-5.6-terra" => Some(openai(5e-6, 22.5e-6, Some(6.25e-6), Some(0.5e-6))),
        "gpt-5.6-luna" => Some(openai(2e-6, 9e-6, Some(2.5e-6), Some(0.2e-6))),
        // gpt-5.5 and gpt-5.4 have no separate cache-write price, so cache
        // writes are billed as regular input in both tiers.
        "gpt-5.5" => Some(openai(10e-6, 45e-6, Some(10e-6), Some(1e-6))),
        "gpt-5.4" => Some(openai(5e-6, 22.5e-6, Some(5e-6), Some(0.5e-6))),
        // The pro models have no prompt-caching prices, so only the input and
        // output rates change in the long-context tier.
        "gpt-5.5-pro" | "gpt-5.4-pro" => Some(openai(60e-6, 270e-6, None, None)),
        _ => None,
    }
}

/// Input-token boundary above which a request is billed at a model's
/// long-context tier. Codex aggregates per-model token sums before pricing is
/// applied, so each request's tier must be decided during aggregation from the
/// model's own threshold rather than a single global constant. This returns the
/// same value `apply_builtin_long_context_rates` stamps onto
/// `Pricing::long_context_threshold`, and falls back to the default 200K
/// boundary used for LiteLLM `*_above_200k_tokens` data.
pub(crate) fn long_context_split_threshold(model: &str) -> u64 {
    builtin_long_context_rates(model_without_date_suffix(model))
        .map(|rates| rates.threshold)
        .unwrap_or(DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS)
}

/// Strips a trailing release-date suffix (`-YYYY-MM-DD` or `-YYYYMMDD`) so
/// date-pinned pricing keys share their base model's long-context rates.
fn model_without_date_suffix(model: &str) -> &str {
    let bytes = model.as_bytes();
    // -YYYY-MM-DD (OpenAI style, e.g. gpt-5.5-2026-04-23)
    if model.len() > 11 {
        let suffix = &bytes[model.len() - 11..];
        if suffix[0] == b'-'
            && suffix[1..5].iter().all(u8::is_ascii_digit)
            && suffix[5] == b'-'
            && suffix[6..8].iter().all(u8::is_ascii_digit)
            && suffix[8] == b'-'
            && suffix[9..].iter().all(u8::is_ascii_digit)
        {
            return &model[..model.len() - 11];
        }
    }
    // -YYYYMMDD (Anthropic style, e.g. claude-3-5-haiku-20241022)
    if model.len() > 9 {
        let suffix = &bytes[model.len() - 9..];
        if suffix[0] == b'-' && suffix[1..].iter().all(u8::is_ascii_digit) {
            return &model[..model.len() - 9];
        }
    }
    model
}

/// Maps Codex log labels that upstream pricing sources do not publish to
/// canonical pricing keys.
fn pricing_alias(model: &str) -> Option<&'static str> {
    match model {
        "gpt-5.3-spark" => Some("gpt-5.3-codex-spark"),
        _ => None,
    }
}

fn matches_model_suffix(part: &str, base: &str) -> bool {
    let Some(index) = part.rfind(base) else {
        return false;
    };
    let suffix = &part[index..];
    suffix == base || suffix.as_bytes().get(base.len()) == Some(&b'-')
}

fn should_log_pricing_refresh_details() -> bool {
    crate::log_level().is_some_and(|level| level >= 4)
}

fn models_dev_pricing() -> Option<&'static PricingMap> {
    static MODELS_DEV_PRICING: ModelsDevPricingCache =
        ModelsDevPricingCache::new(MODELS_DEV_FAILURE_RETRY_AFTER);
    MODELS_DEV_PRICING.get_or_try_load(fetch_models_dev_json)
}

/// Pricing built from the models.dev snapshot embedded at build time. Unlike the
/// network source this is always available, so it lets offline runs price models
/// that LiteLLM and the built-in table do not cover (for example newly released
/// Anthropic models). It is kept separate from the primary table so it never
/// participates in that table's fuzzy alias matching.
fn embedded_models_dev_pricing() -> &'static PricingMap {
    static EMBEDDED_MODELS_DEV_PRICING: OnceLock<PricingMap> = OnceLock::new();
    EMBEDDED_MODELS_DEV_PRICING.get_or_init(|| {
        let mut map = PricingMap::default();
        map.load_models_dev_json_missing(BUILD_TIME_MODELS_DEV_JSON)
            .expect("embedded models-dev-pricing.json must parse");
        map
    })
}

fn load_models_dev_pricing<F>(fetch_json: F) -> Option<PricingMap>
where
    F: FnOnce() -> std::io::Result<String>,
{
    let json = match fetch_json() {
        Ok(json) => json,
        Err(error) => {
            if should_log_pricing_refresh_details() {
                eprintln!(
                    "WARN  Failed to fetch models.dev pricing ({error}); using LiteLLM pricing."
                );
            }
            return None;
        }
    };
    let mut map = PricingMap::default();
    if map.load_models_dev_json_missing(&json).is_none() {
        if should_log_pricing_refresh_details() {
            eprintln!("WARN  Failed to parse models.dev pricing; using LiteLLM pricing.");
        }
        return None;
    }
    Some(map)
}

fn fetch_pricing_json() -> std::io::Result<String> {
    fetch_json_url(LITELLM_PRICING_URL)
}

fn fetch_models_dev_json() -> std::io::Result<String> {
    fetch_json_url(MODELS_DEV_API_URL)
}

fn fetch_json_url(url: &str) -> std::io::Result<String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(PRICING_FETCH_TIMEOUT_SECONDS)))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if response.status().as_u16() != 200 {
        return Err(std::io::Error::other(format!(
            "HTTP {}",
            response.status().as_u16()
        )));
    }
    response
        .body_mut()
        .with_config()
        .limit(PRICING_FETCH_MAX_BYTES)
        .read_to_string()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        BUILD_TIME_MODELS_DEV_JSON, BUILD_TIME_PRICING_JSON, Pricing, PricingMap,
        embedded_models_dev_pricing, long_context_split_threshold, model_without_date_suffix,
    };
    use ccusage_test_support::fs_fixture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn loads_embedded_claude_pricing() {
        let pricing = PricingMap::load_embedded();
        assert!(pricing.len() > 0);
        assert!(pricing.find("claude-sonnet-4-20250514").is_some());
    }

    #[test]
    fn reads_embedded_model_context_limits() {
        let pricing = PricingMap::load_embedded();

        let _ = pricing.context_limit("anthropic.claude-3-5-sonnet-20240620-v1:0");
    }

    #[test]
    fn embedded_pricing_includes_hermes_frontier_models() {
        let pricing = PricingMap::load_embedded();

        assert!(pricing.find("gpt-5.5").is_some());
        assert!(pricing.find("grok-4.3").is_some());
        assert_eq!(pricing.context_limit("grok-4.3"), Some(1_000_000));
    }

    #[test]
    fn embedded_pricing_includes_moonshot_kimi_for_offline_reports() {
        let pricing = PricingMap::load_embedded();
        let kimi_k25 = pricing.find("moonshot/kimi-k2.5").unwrap();
        let kimi_k26 = pricing.find("moonshot/kimi-k2.6").unwrap();

        assert_eq!(kimi_k25.input, 0.6e-6);
        assert_eq!(kimi_k25.output, 3e-6);
        assert_eq!(kimi_k25.cache_read, 0.1e-6);
        assert!(kimi_k25.cache_read_explicit);
        assert_eq!(kimi_k26.input, 0.95e-6);
        assert_eq!(kimi_k26.output, 4e-6);
        assert_eq!(kimi_k26.cache_read, 0.16e-6);
        assert!(kimi_k26.cache_read_explicit);
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.5"), Some(262_144));
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.6"), Some(262_144));
    }

    #[test]
    fn offline_prices_kimi_k3_from_embedded_models_dev() {
        // LiteLLM may lag new Moonshot releases; offline pricing should still
        // resolve from the embedded models.dev snapshot (see #1462).
        let pricing = PricingMap::load_embedded();
        let kimi_k3 = pricing.find("moonshot/kimi-k3").unwrap_or_else(|| {
            pricing
                .find("kimi-k3")
                .expect("embedded models.dev should include kimi-k3 pricing")
        });

        assert_eq!(kimi_k3.input, 3e-6);
        assert_eq!(kimi_k3.output, 15e-6);
        assert_eq!(kimi_k3.cache_read, 0.3e-6);
        assert!(kimi_k3.cache_read_explicit);
        assert!(
            pricing
                .context_limit("moonshot/kimi-k3")
                .or_else(|| pricing.context_limit("kimi-k3"))
                == Some(1_048_576)
        );
    }

    #[test]
    fn embedded_pricing_includes_z_ai_glm_models_for_offline_reports() {
        let pricing = PricingMap::load_embedded();

        let glm_51 = pricing.find("glm-5.1").unwrap();
        assert_eq!(glm_51.input, 1.4e-6);
        assert_eq!(glm_51.output, 4.4e-6);
        assert_eq!(glm_51.cache_create, 0.0);
        assert_eq!(glm_51.cache_read, 0.26e-6);
        assert!(glm_51.cache_read_explicit);

        let glm_5 = pricing.find("glm-5").unwrap();
        assert_eq!(glm_5.input, 1.0e-6);
        assert_eq!(glm_5.output, 3.2e-6);
        assert_eq!(glm_5.cache_create, 0.0);
        assert_eq!(glm_5.cache_read, 0.2e-6);
        assert_eq!(pricing.context_limit("zai/glm-5"), Some(200_000));

        let glm_5_turbo = pricing.find("glm-5-turbo").unwrap();
        assert_eq!(glm_5_turbo.input, 1.2e-6);
        assert_eq!(glm_5_turbo.output, 4.0e-6);
        assert_eq!(glm_5_turbo.cache_create, 0.0);
        assert_eq!(glm_5_turbo.cache_read, 0.24e-6);

        let glm_47 = pricing.find("glm-4.7").unwrap();
        assert_eq!(glm_47.input, 0.6e-6);
        assert_eq!(glm_47.output, 2.2e-6);
        assert_eq!(glm_47.cache_create, 0.0);
        assert_eq!(glm_47.cache_read, 0.11e-6);

        let glm_46 = pricing.find("glm-4.6").unwrap();
        assert_eq!(glm_46.input, 0.6e-6);
        assert_eq!(glm_46.output, 2.2e-6);
        assert_eq!(glm_46.cache_create, 0.0);
        assert_eq!(glm_46.cache_read, 0.11e-6);

        let glm_45 = pricing.find("glm-4.5").unwrap();
        assert_eq!(glm_45.input, 0.6e-6);
        assert_eq!(glm_45.output, 2.2e-6);
        assert_eq!(glm_45.cache_create, 0.0);
        assert_eq!(glm_45.cache_read, 0.11e-6);

        let zai_glm_45 = pricing.find("zai/glm-4.5").unwrap();
        assert_eq!(zai_glm_45.input, 0.6e-6);
        assert_eq!(zai_glm_45.output, 2.2e-6);
        assert_eq!(zai_glm_45.cache_create, 0.0);
        assert_eq!(zai_glm_45.cache_read, 0.11e-6);
        assert_eq!(pricing.context_limit("zai/glm-4.5"), Some(128_000));
    }

    #[test]
    fn embedded_pricing_patches_z_ai_glm_entries_without_litellm_cache_rates() {
        let pricing = PricingMap::load_embedded();

        let glm_45_air = pricing.find("zai/glm-4.5-air").unwrap();
        assert_eq!(glm_45_air.input, 0.2e-6);
        assert_eq!(glm_45_air.output, 1.1e-6);
        assert_eq!(glm_45_air.cache_create, 0.0);
        assert_eq!(glm_45_air.cache_read, 0.03e-6);

        let glm_45_x = pricing.find("zai/glm-4.5-x").unwrap();
        assert_eq!(glm_45_x.input, 2.2e-6);
        assert_eq!(glm_45_x.output, 8.9e-6);
        assert_eq!(glm_45_x.cache_create, 0.0);
        assert_eq!(glm_45_x.cache_read, 0.45e-6);

        let glm_45v = pricing.find("zai/glm-4.5v").unwrap();
        assert_eq!(glm_45v.input, 0.6e-6);
        assert_eq!(glm_45v.output, 1.8e-6);
        assert_eq!(glm_45v.cache_create, 0.0);
        assert_eq!(glm_45v.cache_read, 0.11e-6);
    }

    #[test]
    fn records_whether_cache_read_rate_came_from_litellm_pricing() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-with-cache": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                },
                "gpt-without-cache": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );

        assert!(pricing.find("gpt-with-cache").unwrap().cache_read_explicit);
        assert!(
            !pricing
                .find("gpt-without-cache")
                .unwrap()
                .cache_read_explicit
        );
    }

    #[test]
    fn skips_invalid_litellm_entries_without_discarding_valid_pricing() {
        let mut pricing = PricingMap::default();
        let loaded = pricing.load_json(
            r#"{
                "sample_spec": {
                    "max_input_tokens": "max input tokens, if the provider specifies it"
                },
                "gpt-valid": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "max_input_tokens": 123
                }
            }"#,
        );

        assert_eq!(loaded, 1);
        assert!(pricing.find("gpt-valid").is_some());
        assert_eq!(pricing.context_limit("gpt-valid"), Some(123));
    }

    #[test]
    fn loads_compact_litellm_pricing_json() {
        let mut pricing = PricingMap::default();
        let loaded = pricing.load_json(
            r#"{
                "gpt-compact": {
                    "i": 0.000001,
                    "o": 0.000010,
                    "cc": 0.00000125,
                    "cr": 0.0000001,
                    "ia": 0.000002,
                    "oa": 0.000020,
                    "cca": 0.0000025,
                    "cra": 0.0000002,
                    "ctx": 123456,
                    "fast": 1.5
                }
            }"#,
        );

        assert_eq!(loaded, 1);
        let compact = pricing.find("gpt-compact").unwrap();
        assert_eq!(compact.input, 1e-6);
        assert_eq!(compact.output, 10e-6);
        assert_eq!(compact.cache_create, 1.25e-6);
        assert_eq!(compact.cache_read, 0.1e-6);
        assert!(compact.cache_read_explicit);
        assert_eq!(compact.input_above_200k, Some(2e-6));
        assert_eq!(compact.output_above_200k, Some(20e-6));
        assert_eq!(compact.cache_create_above_200k, Some(2.5e-6));
        assert_eq!(compact.cache_read_above_200k, Some(0.2e-6));
        assert_eq!(compact.fast_multiplier, 1.5);
        assert_eq!(pricing.context_limit("gpt-compact"), Some(123456));
    }

    #[test]
    fn falls_back_to_full_litellm_pricing_when_compact_shape_is_incomplete() {
        let mut pricing = PricingMap::default();
        let loaded = pricing.load_json(
            r#"{
                "gpt-full-with-extra-i": {
                    "i": "provider metadata",
                    "o": "provider metadata",
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );

        assert_eq!(loaded, 1);
        let full = pricing.find("gpt-full-with-extra-i").unwrap();
        assert_eq!(full.input, 1e-6);
        assert_eq!(full.output, 10e-6);
    }

    #[test]
    fn keeps_models_dev_fallback_disabled_for_embedded_and_offline_pricing() {
        use ccusage_cli::PricingOverride;
        assert!(!PricingMap::load_embedded().models_dev_fallback_enabled());
        assert!(
            !PricingMap::load_with_overrides(
                true,
                false,
                std::iter::empty::<(&String, &PricingOverride)>(),
            )
            .models_dev_fallback_enabled()
        );
    }

    #[test]
    fn retries_models_dev_pricing_after_fetch_failure() {
        let cache = super::ModelsDevPricingCache::new(std::time::Duration::ZERO);

        let failed = cache.get_or_try_load(|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "temporary failure",
            ))
        });
        assert!(failed.is_none());

        let pricing = cache
            .get_or_try_load(|| {
                Ok(r#"{
                    "openai": {
                        "id": "openai",
                        "name": "OpenAI",
                        "models": {
                            "gpt-retry": {
                                "id": "gpt-retry",
                                "name": "GPT Retry",
                                "cost": {
                                    "input": 1.0,
                                    "output": 2.0
                                },
                                "limit": {
                                    "context": 42
                                }
                            }
                        }
                    }
                }"#
                .to_string())
            })
            .expect("models.dev retry should cache successful pricing");

        let gpt_retry = pricing
            .find_entry("gpt-retry")
            .expect("successful retry should load pricing");
        assert_eq!(gpt_retry.input, 0.000001);
        assert_eq!(gpt_retry.output, 0.000002);
        assert_eq!(pricing.context_limit_entry("gpt-retry"), Some(42));
    }

    #[test]
    fn backs_off_models_dev_pricing_after_fetch_failure() {
        let cache = super::ModelsDevPricingCache::new(std::time::Duration::from_secs(60));
        let attempts = AtomicUsize::new(0);

        let failed = cache.get_or_try_load(|| {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "temporary failure",
            ))
        });
        assert!(failed.is_none());

        let skipped = cache.get_or_try_load(|| {
            attempts.fetch_add(1, Ordering::Relaxed);
            Ok(r#"{
                "openai": {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-skipped": {
                            "id": "gpt-skipped",
                            "name": "GPT Skipped",
                            "cost": {
                                "input": 1.0,
                                "output": 2.0
                            }
                        }
                    }
                }
            }"#
            .to_string())
        });
        assert!(skipped.is_none());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn loads_missing_models_dev_pricing_without_overriding_litellm() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-primary": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001,
                    "max_input_tokens": 123
                },
                "openrouter/gpt-alias": {
                    "input_cost_per_token": 0.000003,
                    "output_cost_per_token": 0.000030,
                    "max_input_tokens": 321
                }
            }"#,
        );

        let models_dev_json = r#"{
                "openai": {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-primary": {
                            "id": "gpt-primary",
                            "name": "GPT Primary",
                            "cost": {
                                "input": 9.0,
                                "output": 90.0,
                                "cache_read": 0.9,
                                "cache_write": 11.25
                            },
                            "limit": {
                                "context": 999
                            }
                        },
                        "gpt-fallback": {
                            "id": "gpt-fallback",
                            "name": "GPT Fallback",
                            "cost": {
                                "input": 2.0,
                                "output": 8.0,
                                "cache_read": 0.2,
                                "cache_write": 2.5
                            },
                            "limit": {
                                "context": 456
                            }
                        },
                        "gpt-alias": {
                            "id": "gpt-alias",
                            "name": "GPT Alias",
                            "cost": {
                                "input": 4.0,
                                "output": 16.0
                            },
                            "limit": {
                                "context": 654
                            }
                        }
                    }
                }
            }"#;

        assert_eq!(
            pricing.load_models_dev_json_missing(models_dev_json),
            Some(2)
        );

        let primary = pricing.find("gpt-primary").unwrap();
        let fallback = pricing.find("gpt-fallback").unwrap();
        let alias = pricing.entries.get("gpt-alias").unwrap();

        assert_eq!(primary.input, 1e-6);
        assert_eq!(primary.output, 10e-6);
        assert_eq!(primary.cache_read, 0.1e-6);
        assert_eq!(pricing.context_limit("gpt-primary"), Some(123));
        assert!((fallback.input - 2e-6).abs() < f64::EPSILON);
        assert!((fallback.output - 8e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_create - 2.5e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_read - 0.2e-6).abs() < f64::EPSILON);
        assert!(fallback.cache_read_explicit);
        assert_eq!(fallback.input_above_200k, None);
        assert_eq!(fallback.output_above_200k, None);
        assert_eq!(fallback.fast_multiplier, 1.0);
        assert_eq!(pricing.context_limit("gpt-fallback"), Some(456));
        assert!((alias.input - 4e-6).abs() < f64::EPSILON);
        assert_eq!(pricing.context_limits.get("gpt-alias"), Some(&654));
    }

    #[test]
    fn rejects_malformed_models_dev_provider_payload() {
        let fixture = fs_fixture!({
            "models-dev.json": r#"{
                "openai": {
                    "models": {
                        "gpt-fallback": {
                            "cost": {
                                "input": 2.0,
                                "output": 8.0
                            }
                        }
                    }
                },
                "broken-provider": {
                    "name": "Broken Provider"
                }
            }"#,
        });
        let json = std::fs::read_to_string(fixture.path("models-dev.json")).unwrap();
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(&json), None);
        assert_eq!(pricing.len(), 0);
    }

    #[test]
    fn loads_flat_models_dev_pricing_snapshot() {
        let mut pricing = PricingMap::default();
        let models_dev_json = r#"{
                "claude-fallback": {
                    "cost": {
                        "input": 3.0,
                        "output": 15.0,
                        "cache_read": 0.3,
                        "cache_write": 3.75
                    },
                    "limit": {
                        "context": 200000
                    }
                }
            }"#;

        assert_eq!(
            pricing.load_models_dev_json_missing(models_dev_json),
            Some(1)
        );

        let fallback = pricing.find("claude-fallback").unwrap();
        assert!((fallback.input - 3e-6).abs() < f64::EPSILON);
        assert!((fallback.output - 15e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_create - 3.75e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_read - 0.3e-6).abs() < f64::EPSILON);
        assert_eq!(pricing.context_limit("claude-fallback"), Some(200000));
    }

    #[test]
    fn embedded_models_dev_snapshot_is_parseable() {
        let mut map = PricingMap::default();
        assert!(
            map.load_models_dev_json_missing(BUILD_TIME_MODELS_DEV_JSON)
                .is_some()
        );
    }

    #[test]
    fn offline_resolves_models_only_in_embedded_models_dev() {
        use ccusage_cli::PricingOverride;
        let offline = PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &PricingOverride)>(),
        );
        // Pick an embedded model the primary table (LiteLLM + built-ins) cannot
        // resolve on its own. `find_entry` never consults the fallback.
        let Some(model) = embedded_models_dev_pricing()
            .entries
            .keys()
            .find(|model| offline.find_entry(model).is_none())
        else {
            return;
        };
        // The primary table alone misses it, but the offline embedded fallback
        // resolves it; a bare map without the fallback flag must not.
        assert!(offline.find_entry(model).is_none());
        assert!(offline.find(model).is_some());
        assert!(PricingMap::default().find(model).is_none());
    }

    #[test]
    fn offline_prices_new_anthropic_model_from_embedded_models_dev() {
        use ccusage_cli::PricingOverride;
        assert!(
            embedded_models_dev_pricing()
                .find_entry("claude-fable-5")
                .is_some(),
            "embedded models.dev snapshot should include claude-fable-5"
        );
        let offline = PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &PricingOverride)>(),
        );
        assert!(offline.find("claude-fable-5").is_some());
    }

    #[test]
    fn embedded_pricing_resolves_overlapping_model_keys_exactly() {
        let pricing = PricingMap::load_embedded();
        let sonnet_4 = pricing.find("claude-sonnet-4-20250514").unwrap();
        let sonnet_45 = pricing.find("claude-sonnet-4-5-20250929").unwrap();

        assert_eq!(
            pricing.find("claude-sonnet-4-20250514").unwrap().input,
            sonnet_4.input
        );
        assert_eq!(
            pricing.find("claude-sonnet-4-5-20250929").unwrap().input,
            sonnet_45.input,
        );
        assert_eq!(
            pricing
                .find("anthropic.claude-sonnet-4-20250514-v1:0")
                .unwrap()
                .input,
            sonnet_4.input,
        );
        assert_eq!(
            pricing.find("claude-3-5-haiku-20241022").unwrap().input,
            0.8e-6,
        );
    }

    #[test]
    fn embedded_pricing_includes_gpt_5_5_for_offline_codex_reports() {
        let pricing = PricingMap::load_embedded();
        let gpt_55 = pricing.find("gpt-5.5").unwrap();

        assert_eq!(gpt_55.input, 5e-6);
        assert_eq!(gpt_55.output, 30e-6);
        assert_eq!(gpt_55.cache_read, 0.5e-6);
        assert!(gpt_55.cache_read_explicit);
        assert_eq!(gpt_55.fast_multiplier, 2.5);
        assert_eq!(pricing.context_limit("gpt-5.5"), Some(1_050_000));
    }

    #[test]
    fn embedded_pricing_includes_gpt_5_6_family_with_long_context_rates() {
        let pricing = PricingMap::load_embedded();

        let sol = pricing.find("gpt-5.6-sol").unwrap();
        assert_eq!(sol.input, 5e-6);
        assert_eq!(sol.output, 30e-6);
        assert_eq!(sol.cache_create, 6.25e-6);
        assert_eq!(sol.cache_read, 0.5e-6);
        assert!(sol.cache_read_explicit);
        assert_eq!(sol.input_above_200k, Some(10e-6));
        assert_eq!(sol.output_above_200k, Some(45e-6));
        assert_eq!(sol.cache_create_above_200k, Some(12.5e-6));
        assert_eq!(sol.cache_read_above_200k, Some(1e-6));
        assert_eq!(sol.long_context_threshold, Some(272_000));
        assert_eq!(pricing.context_limit("gpt-5.6-sol"), Some(1_050_000));

        let terra = pricing.find("gpt-5.6-terra").unwrap();
        assert_eq!(terra.input, 2.5e-6);
        assert_eq!(terra.cache_create, 3.125e-6);
        assert_eq!(terra.input_above_200k, Some(5e-6));
        assert_eq!(terra.output_above_200k, Some(22.5e-6));

        let luna = pricing.find("gpt-5.6-luna").unwrap();
        assert_eq!(luna.input, 1e-6);
        assert_eq!(luna.output, 6e-6);
        assert_eq!(luna.input_above_200k, Some(2e-6));
        assert_eq!(luna.output_above_200k, Some(9e-6));
    }

    #[test]
    fn embedded_pricing_fills_gpt_long_context_tier_rates() {
        let pricing = PricingMap::load_embedded();

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(gpt_55.input_above_200k, Some(10e-6));
        assert_eq!(gpt_55.output_above_200k, Some(45e-6));
        assert_eq!(gpt_55.cache_read_above_200k, Some(1e-6));
        assert_eq!(gpt_55.long_context_threshold, Some(272_000));

        let gpt_54 = pricing.find("gpt-5.4").unwrap();
        assert_eq!(gpt_54.input_above_200k, Some(5e-6));
        assert_eq!(gpt_54.output_above_200k, Some(22.5e-6));
        assert_eq!(gpt_54.long_context_threshold, Some(272_000));

        // Models the pricing page lists without a long-context tier stay flat.
        let mini = pricing.find("gpt-5.4-mini").unwrap();
        assert_eq!(mini.input_above_200k, None);
        assert_eq!(mini.long_context_threshold, None);
    }

    #[test]
    fn long_context_overlay_survives_litellm_refresh_and_defers_to_upstream() {
        let mut pricing = PricingMap::load_embedded();
        // A live LiteLLM refresh replaces whole entries with flat rates and
        // may add date-pinned keys.
        pricing.load_json(
            r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031
                },
                "gpt-5.5-2026-04-23": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031
                }
            }"#,
        );
        pricing.apply_builtin_long_context_rates();

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(gpt_55.input, 6e-6);
        assert_eq!(gpt_55.input_above_200k, Some(10e-6));
        assert_eq!(gpt_55.long_context_threshold, Some(272_000));
        // Date-pinned keys share the base model's long-context rates.
        let dated = pricing.find_exact("gpt-5.5-2026-04-23").unwrap();
        assert_eq!(dated.input_above_200k, Some(10e-6));

        // Tier rates published upstream win over the built-in overlay.
        pricing.load_json(
            r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000006,
                    "output_cost_per_token": 0.000031,
                    "input_cost_per_token_above_200k_tokens": 0.000012
                }
            }"#,
        );
        pricing.apply_builtin_long_context_rates();

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(gpt_55.input_above_200k, Some(12e-6));
        assert_eq!(gpt_55.long_context_threshold, None);
    }

    #[test]
    fn long_context_split_threshold_is_per_model() {
        // OpenAI two-stage models switch tiers above 272K input tokens.
        assert_eq!(long_context_split_threshold("gpt-5.6-sol"), 272_000);
        assert_eq!(long_context_split_threshold("gpt-5.5"), 272_000);
        assert_eq!(long_context_split_threshold("gpt-5.5-pro"), 272_000);
        // Date-pinned keys share their base model's boundary.
        assert_eq!(long_context_split_threshold("gpt-5.5-2026-04-23"), 272_000);
        // Models without a built-in tier fall back to the 200K default used for
        // LiteLLM `*_above_200k_tokens` data.
        assert_eq!(long_context_split_threshold("gpt-5"), 200_000);
        assert_eq!(long_context_split_threshold("gpt-5.4-mini"), 200_000);
    }

    #[test]
    fn strips_model_date_suffixes() {
        assert_eq!(model_without_date_suffix("gpt-5.5-2026-04-23"), "gpt-5.5");
        assert_eq!(
            model_without_date_suffix("gpt-5.5-pro-2026-04-23"),
            "gpt-5.5-pro"
        );
        assert_eq!(
            model_without_date_suffix("claude-3-5-haiku-20241022"),
            "claude-3-5-haiku"
        );
        assert_eq!(model_without_date_suffix("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(model_without_date_suffix("gpt-4-0613"), "gpt-4-0613");
    }

    #[test]
    fn pricing_lookup_resolves_model_aliases() {
        let _aliases =
            crate::model_aliases::set_model_aliases_for_tests([("private-gpt-55", "gpt-5.5")]);
        let pricing = PricingMap::load_embedded();

        assert_eq!(
            pricing.find("private-gpt-55").unwrap().input,
            pricing.find("gpt-5.5").unwrap().input
        );
        assert_eq!(pricing.context_limit("private-gpt-55"), Some(1_050_000));
    }

    #[test]
    fn pricing_lookup_prefers_known_original_model_before_alias() {
        let _aliases =
            crate::model_aliases::set_model_aliases_for_tests([("claude-opus-4-8", "mythos-5")]);
        let pricing = PricingMap::load_embedded();

        let original = pricing.find_entry("claude-opus-4-8").unwrap();
        let resolved = pricing.find("claude-opus-4-8").unwrap();

        assert_eq!(resolved.input, original.input);
        assert_eq!(
            pricing.context_limit("claude-opus-4-8"),
            pricing.context_limit_entry("claude-opus-4-8")
        );
    }

    #[test]
    fn embedded_pricing_includes_codex_priority_multiplier() {
        let pricing = PricingMap::load_embedded();

        assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
        assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
    }

    #[test]
    fn embedded_pricing_does_not_resolve_undated_codex_auto_review_model() {
        let pricing = PricingMap::load_embedded();

        assert!(pricing.find("codex-auto-review").is_none());
        assert!(pricing.context_limit("codex-auto-review").is_none());
    }

    #[test]
    fn embedded_pricing_resolves_codex_spark_short_model_alias() {
        let pricing = PricingMap::load_embedded();
        let short_spark = pricing
            .find("gpt-5.3-spark")
            .expect("gpt-5.3-spark should resolve via model alias");
        let codex_spark = pricing
            .find("gpt-5.3-codex-spark")
            .expect("canonical Codex Spark pricing should exist");

        assert_eq!(short_spark.input, codex_spark.input);
        assert_eq!(short_spark.output, codex_spark.output);
        assert_eq!(short_spark.cache_read, codex_spark.cache_read);
        assert_eq!(short_spark.fast_multiplier, codex_spark.fast_multiplier);
    }

    #[test]
    fn embedded_pricing_includes_claude_fast_multiplier_for_provider_models() {
        let pricing = PricingMap::load_embedded();

        assert_eq!(
            pricing
                .find("anthropic.claude-opus-4-6-v1")
                .unwrap()
                .fast_multiplier,
            6.0
        );
        assert_eq!(
            pricing
                .find("anthropic.claude-opus-4-7")
                .unwrap()
                .fast_multiplier,
            6.0
        );
        assert_eq!(
            pricing
                .find("anthropic.claude-opus-4-8")
                .unwrap()
                .fast_multiplier,
            2.0
        );
    }

    #[test]
    fn embedded_pricing_resolves_opus_47_dot_model_names() {
        let pricing = PricingMap::load_embedded();

        assert_eq!(
            pricing.find("claude-opus-4.7-20260416").unwrap().input,
            5e-6
        );
        assert_eq!(pricing.context_limit("claude-opus-4.7"), Some(1_000_000));
        assert_eq!(
            pricing
                .find("openrouter/anthropic/claude-opus-4.7")
                .unwrap()
                .input,
            5e-6
        );
    }

    #[test]
    fn embedded_pricing_resolves_opus_48_dot_model_names() {
        let pricing = PricingMap::load_embedded();

        let opus_48 = pricing.find("claude-opus-4.8-20260528").unwrap();
        assert_eq!(opus_48.input, 5e-6);
        assert_eq!(opus_48.output, 25e-6);
        assert_eq!(opus_48.cache_create, 6.25e-6);
        assert_eq!(opus_48.cache_read, 0.5e-6);
        assert_eq!(pricing.context_limit("claude-opus-4.8"), Some(1_000_000));
    }

    #[test]
    fn embedded_pricing_resolves_separator_aliases_for_other_claude_models() {
        let pricing = PricingMap::load_embedded();
        let sonnet_46 = pricing.find("claude-sonnet-4-6").unwrap();
        let haiku_45 = pricing.find("claude-haiku-4-5").unwrap();

        assert_eq!(
            pricing.find("claude-sonnet-4.6-20260416").unwrap().input,
            sonnet_46.input
        );
        assert_eq!(
            pricing.find("claude-haiku-4.5").unwrap().input,
            haiku_45.input
        );
        assert_eq!(
            pricing.context_limit("claude-sonnet-4.6"),
            pricing.context_limit("claude-sonnet-4-6")
        );
        assert_eq!(
            pricing.context_limit("claude-haiku-4.5"),
            pricing.context_limit("claude-haiku-4-5")
        );
    }

    #[test]
    fn fuzzy_match_requires_model_key_boundaries() {
        let mut pricing = PricingMap::default();
        pricing.entries.insert(
            "claude-opus-4-7".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        pricing.entries.insert(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );

        assert!(pricing.find("claude-opus-4.70").is_none());
    }

    #[test]
    fn fuzzy_match_does_not_fall_back_across_numeric_model_versions() {
        let mut pricing = PricingMap::default();
        pricing.entries.insert(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );

        assert!(pricing.find("claude-opus-4.8-20260528").is_none());
        assert!(pricing.find("claude-opus-4-9").is_none());
        assert!(pricing.find("claude-opus-5").is_none());
        assert!(pricing.find("claude-opus-4.70").is_none());
        assert!(pricing.find("claude-opus-4-20250514").is_some());
    }

    #[test]
    fn fuzzy_match_allows_date_like_suffixes_for_known_numeric_model_versions() {
        let pricing = PricingMap::load_embedded();

        assert!(pricing.find("claude-opus-4-8-20270898").is_some());
        assert!(pricing.find("claude-opus-4-9").is_none());
        assert!(pricing.find("claude-opus-5").is_none());
    }

    #[test]
    fn fills_codex_fast_multiplier_when_litellm_pricing_omits_it() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000030,
                    "cache_read_input_token_cost": 0.0000005
                },
                "gpt-5.4": {
                    "input_cost_per_token": 0.0000025,
                    "output_cost_per_token": 0.000015,
                    "cache_read_input_token_cost": 0.00000025
                },
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                },
                "gpt-5.2-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                }
            }"#,
        );

        assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
        assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.2-codex").unwrap().fast_multiplier, 1.0);
    }

    #[test]
    fn fills_claude_fast_multiplier_when_litellm_pricing_omits_it() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "vertex_ai/claude-opus-4-7@default": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "openrouter/anthropic/claude-opus-4.7": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4.7-20260416": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4.8-20260528": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4-70": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                }
            }"#,
        );

        assert_eq!(
            pricing
                .find("vertex_ai/claude-opus-4-7@default")
                .unwrap()
                .fast_multiplier,
            6.0
        );
        assert_eq!(
            pricing
                .find("openrouter/anthropic/claude-opus-4.7")
                .unwrap()
                .fast_multiplier,
            6.0
        );
        assert_eq!(
            pricing
                .find("claude-opus-4.7-20260416")
                .unwrap()
                .fast_multiplier,
            6.0
        );
        assert_eq!(
            pricing
                .find("claude-opus-4.8-20260528")
                .unwrap()
                .fast_multiplier,
            2.0
        );
        assert_eq!(
            pricing.find("claude-opus-4-70").unwrap().fast_multiplier,
            1.0
        );
    }

    #[test]
    fn embedded_build_time_pricing_is_compact() {
        assert!(BUILD_TIME_PRICING_JSON.len() < 200_000);
        assert!(!BUILD_TIME_PRICING_JSON.contains("\"source\""));
        assert!(!BUILD_TIME_PRICING_JSON.contains("vertex_ai/"));
        assert!(BUILD_TIME_PRICING_JSON.contains("claude-opus-4-6"));
    }

    #[test]
    fn fuzzy_match_prefers_longest_model_key() {
        let mut pricing = PricingMap::default();
        pricing.entries.insert(
            "claude-sonnet-4".to_string(),
            Pricing {
                input: 1.0,
                output: 0.0,
                cache_create: 0.0,
                cache_read: 0.0,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        pricing.entries.insert(
            "claude-sonnet-4-20250514".to_string(),
            Pricing {
                input: 2.0,
                output: 0.0,
                cache_create: 0.0,
                cache_read: 0.0,
                cache_read_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );

        let matched = pricing
            .find("claude-sonnet-4-20250514-via-bedrock")
            .unwrap();

        assert_eq!(matched.input, 2.0);
    }

    mod overrides {
        use super::super::{Pricing, PricingMap};
        use ccusage_cli::PricingOverride;
        use std::collections::BTreeMap;

        fn build_overrides<F: FnOnce(&mut PricingOverride)>(
            model: &str,
            init: F,
        ) -> BTreeMap<String, PricingOverride> {
            let mut override_value = PricingOverride::default();
            init(&mut override_value);
            let mut map = BTreeMap::new();
            map.insert(model.to_string(), override_value);
            map
        }

        #[test]
        fn full_override_creates_new_model() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("custom-model", |o| {
                o.input_cost_per_token = Some(1e-6);
                o.output_cost_per_token = Some(2e-6);
                o.cache_creation_input_token_cost = Some(3e-6);
                o.cache_read_input_token_cost = Some(4e-7);
                o.fast_multiplier = Some(2.0);
                o.max_input_tokens = Some(123_456);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("custom-model").unwrap();
            assert_eq!(entry.input, 1e-6);
            assert_eq!(entry.output, 2e-6);
            assert_eq!(entry.cache_create, 3e-6);
            assert_eq!(entry.cache_read, 4e-7);
            assert!(entry.cache_read_explicit);
            assert_eq!(entry.fast_multiplier, 2.0);
            assert_eq!(pricing.context_limit("custom-model"), Some(123_456));
        }

        #[test]
        fn partial_override_preserves_existing_fields() {
            let mut pricing = PricingMap::default();
            pricing.entries.insert(
                "existing".to_string(),
                Pricing {
                    input: 10e-6,
                    output: 20e-6,
                    cache_create: 30e-6,
                    cache_read: 40e-6,
                    cache_read_explicit: true,
                    input_above_200k: Some(15e-6),
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.5,
                },
            );

            let overrides = build_overrides("existing", |o| {
                o.input_cost_per_token = Some(99e-6);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("existing").unwrap();
            assert_eq!(entry.input, 99e-6);
            assert_eq!(entry.output, 20e-6);
            assert_eq!(entry.cache_create, 30e-6);
            assert_eq!(entry.cache_read, 40e-6);
            assert!(entry.cache_read_explicit);
            assert_eq!(entry.input_above_200k, Some(15e-6));
            assert_eq!(entry.fast_multiplier, 1.5);
        }

        #[test]
        fn override_without_cache_read_does_not_set_explicit() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("new-model", |o| {
                o.input_cost_per_token = Some(1e-6);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("new-model").unwrap();
            assert!(!entry.cache_read_explicit);
            assert_eq!(entry.cache_read, 0.0);
        }

        #[test]
        fn override_with_cache_read_sets_explicit() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("new-model", |o| {
                o.cache_read_input_token_cost = Some(0.0);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("new-model").unwrap();
            assert!(entry.cache_read_explicit);
        }

        #[test]
        fn max_input_tokens_writes_context_limits() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("with-limit", |o| {
                o.max_input_tokens = Some(2_000_000);
            });
            pricing.apply_overrides(overrides.iter());
            assert_eq!(pricing.context_limit("with-limit"), Some(2_000_000));
        }

        #[test]
        fn missing_max_input_tokens_does_not_clobber_existing_limit() {
            let mut pricing = PricingMap::default();
            pricing.context_limits.insert("model".to_string(), 500_000);
            let overrides = build_overrides("model", |o| {
                o.input_cost_per_token = Some(1e-6);
            });
            pricing.apply_overrides(overrides.iter());
            assert_eq!(pricing.context_limit("model"), Some(500_000));
        }

        #[test]
        fn input_override_scales_cache_proportionally() {
            let mut pricing = PricingMap::default();
            // Base: input=3e-6, cache_read=3e-7 (0.1x), cache_create=3.75e-6 (1.25x)
            // cache_read_explicit=false means these were derived from input by LiteLLM
            pricing.entries.insert(
                "claude-model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    cache_create: 3.75e-6,
                    cache_read: 3e-7,
                    cache_read_explicit: false,
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: Some(4.6875e-6),
                    cache_read_above_200k: Some(3.75e-7),
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                },
            );

            // Override input to 2e-6 (2/3 of original), don't touch cache
            let overrides = build_overrides("claude-model", |o| {
                o.input_cost_per_token = Some(2e-6);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("claude-model").unwrap();
            assert_eq!(entry.input, 2e-6);
            assert_eq!(entry.output, 15e-6); // unchanged
            // cache_create: 3.75e-6 * (2/3) = 2.5e-6
            assert!((entry.cache_create - 2.5e-6).abs() < 1e-15);
            // cache_read: 3e-7 * (2/3) = 2e-7
            assert!((entry.cache_read - 2e-7).abs() < 1e-15);
            // above_200k variants also scaled
            assert!((entry.cache_create_above_200k.unwrap() - 3.125e-6).abs() < 1e-15);
            assert!((entry.cache_read_above_200k.unwrap() - 2.5e-7).abs() < 1e-15);
        }

        #[test]
        fn input_override_does_not_scale_zero_cache() {
            let mut pricing = PricingMap::default();
            // Base has zero cache values
            pricing.entries.insert(
                "no-cache-model".to_string(),
                Pricing {
                    input: 5e-6,
                    output: 10e-6,
                    cache_create: 0.0,
                    cache_read: 0.0,
                    cache_read_explicit: false,
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                },
            );

            let overrides = build_overrides("no-cache-model", |o| {
                o.input_cost_per_token = Some(2e-6);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("no-cache-model").unwrap();
            assert_eq!(entry.cache_create, 0.0);
            assert_eq!(entry.cache_read, 0.0);
        }

        #[test]
        fn explicit_cache_override_takes_precedence_over_scaling() {
            let mut pricing = PricingMap::default();
            // cache_read_explicit=false so scaling would normally apply
            pricing.entries.insert(
                "model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    cache_create: 3.75e-6,
                    cache_read: 3e-7,
                    cache_read_explicit: false,
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                },
            );

            // User overrides both input AND cache_read explicitly
            let overrides = build_overrides("model", |o| {
                o.input_cost_per_token = Some(2e-6);
                o.cache_read_input_token_cost = Some(5e-7); // explicit, not scaled
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("model").unwrap();
            assert_eq!(entry.input, 2e-6);
            assert_eq!(entry.cache_read, 5e-7); // explicit value, not 2e-7
            // cache_create still scaled since not explicitly provided
            assert!((entry.cache_create - 2.5e-6).abs() < 1e-15);
        }
    }
}
