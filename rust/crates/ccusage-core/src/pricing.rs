use std::{
    borrow::Cow,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use ccusage_cli::PricingOverride;

use serde::Deserialize;
use serde_json::Value;

use crate::{
    MILLIS_PER_DAY, MILLIS_PER_HOUR, TimestampMs,
    fast::{FxHashMap, FxHashSet},
};

// The embedded snapshots ship deflated - the models.dev one alone would
// otherwise add a quarter megabyte of JSON to the binary - and are inflated
// once, on first use, by the accessors below.
const BUILD_TIME_PRICING_DEFLATE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-pricing.json.deflate"));
const BUILD_TIME_MODELS_DEV_DEFLATE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models-dev-pricing.json.deflate"));
const MODELS_DEV_CATALOG_RULES_DEFLATE: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/models-dev-catalog-rules.json.deflate"
));
const FAST_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("fast-multiplier-overrides.json");

/// Inflate one of the deflated snapshots above. Infallible by construction:
/// build.rs produced the bytes from JSON it had just serialized.
fn inflate_snapshot(cell: &'static OnceLock<String>, deflated: &[u8]) -> &'static str {
    cell.get_or_init(|| {
        let bytes = miniz_oxide::inflate::decompress_to_vec(deflated)
            .expect("inflate embedded pricing snapshot");
        String::from_utf8(bytes).expect("embedded pricing snapshot is UTF-8")
    })
}

fn build_time_pricing_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, BUILD_TIME_PRICING_DEFLATE)
}

fn build_time_models_dev_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, BUILD_TIME_MODELS_DEV_DEFLATE)
}

fn models_dev_catalog_rules_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, MODELS_DEV_CATALOG_RULES_DEFLATE)
}
const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_FAILURE_RETRY_AFTER: Duration = Duration::from_secs(60);
// Anthropic date-suffixed model aliases use YYYYMMDD, while other numeric
// suffixes are treated as distinct model versions.
const MODEL_DATE_SUFFIX_DIGITS: usize = 8;

/// Identifies the upstream document whose schema a pricing response must use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricingEndpoint {
    LiteLlm,
    ModelsDev,
}

impl PricingEndpoint {
    /// Returns the pricing endpoint represented by a fetch URL.
    pub fn for_url(url: &str) -> Option<Self> {
        match url {
            LITELLM_PRICING_URL => Some(Self::LiteLlm),
            MODELS_DEV_API_URL => Some(Self::ModelsDev),
            _ => None,
        }
    }

    /// Returns whether a body contains at least one entry the endpoint loader can use.
    pub fn validates(self, json: &str) -> bool {
        match self {
            Self::LiteLlm => litellm_json_has_loader_usable_entry(json),
            Self::ModelsDev => models_dev_json_has_loader_usable_entry(json),
        }
    }

    /// Returns whether a body has a non-empty pricing shape for this endpoint.
    pub fn validates_shape(self, json: &str) -> bool {
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(json) else {
            return false;
        };

        match self {
            Self::LiteLlm => entries.values().any(litellm_entry_has_required_cost),
            Self::ModelsDev => models_dev_object_has_required_shape(&entries),
        }
    }
}

fn litellm_json_has_loader_usable_entry(json: &str) -> bool {
    let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    entries
        .values()
        .any(|value| parse_litellm_pricing(value.clone()).is_some())
}

fn models_dev_json_has_loader_usable_entry(json: &str) -> bool {
    let Some(raw) = parse_models_dev_json(json) else {
        return false;
    };
    let rules = models_dev_catalog_rules();
    match raw {
        ModelsDevJson::Providers(providers) => providers.values().any(|provider| {
            provider
                .models
                .iter()
                .any(|(model_key, model)| models_dev_usable_cost(rules, model_key, model).is_some())
        }),
        ModelsDevJson::Models(models) => models
            .iter()
            .any(|(model_key, model)| models_dev_usable_cost(rules, model_key, model).is_some()),
    }
}

fn litellm_entry_has_required_cost(value: &Value) -> bool {
    let Some(entry) = value.as_object() else {
        return false;
    };
    let full = entry
        .get("input_cost_per_token")
        .is_some_and(Value::is_number)
        && entry
            .get("output_cost_per_token")
            .is_some_and(Value::is_number);
    let compact = entry.get("i").is_some_and(Value::is_number)
        && entry.get("o").is_some_and(Value::is_number);
    full || compact
}

fn models_dev_object_has_required_shape(entries: &serde_json::Map<String, Value>) -> bool {
    if entries.values().any(models_dev_entry_has_models_field) {
        return entries.values().all(models_dev_provider_has_required_shape)
            && entries.values().any(models_dev_provider_has_required_cost);
    }
    entries.values().all(models_dev_entry_has_required_cost)
        && entries.values().any(models_dev_entry_has_nonzero_cost)
}

fn models_dev_provider_has_required_shape(value: &Value) -> bool {
    let Some(provider) = value.as_object() else {
        return false;
    };
    let Some(models) = provider.get("models").and_then(Value::as_object) else {
        return false;
    };
    models.values().all(Value::is_object)
}

fn models_dev_provider_has_required_cost(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
        .is_some_and(|models| models.values().any(models_dev_model_has_required_cost))
}

fn models_dev_model_has_required_cost(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|model| model.get("cost"))
        .and_then(Value::as_object)
        .is_some_and(models_dev_cost_has_required_rates)
}

fn models_dev_entry_has_nonzero_cost(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|entry| entry.get("cost"))
        .and_then(Value::as_object)
        .is_some_and(models_dev_cost_has_required_rates)
}

fn models_dev_cost_has_required_rates(cost: &serde_json::Map<String, Value>) -> bool {
    let Some(input) = cost.get("input").and_then(Value::as_f64) else {
        return false;
    };
    let Some(output) = cost.get("output").and_then(Value::as_f64) else {
        return false;
    };
    input != 0.0 || output != 0.0
}

#[derive(Debug, Clone, Copy)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub(crate) cache_create: f64,
    pub cache_read: f64,
    pub cache_read_explicit: bool,
    /// Whether `cache_create` came from published data rather than the derived
    /// `input * 1.25` default, so provider-fact patches know what they may fix.
    cache_create_explicit: bool,
    pub input_above_200k: Option<f64>,
    pub output_above_200k: Option<f64>,
    pub(crate) cache_create_above_200k: Option<f64>,
    pub cache_read_above_200k: Option<f64>,
    // Token count above which the `*_above_200k` rates apply. The field names
    // keep the LiteLLM `_above_200k_tokens` suffix for JSON compatibility, but
    // some providers switch tiers at a different point (OpenAI long-context
    // pricing starts above 272K input tokens), so the threshold is per model.
    pub(crate) long_context_threshold: Option<u64>,
    pub fast_multiplier: f64,
}

/// Default tier boundary for LiteLLM `*_above_200k_tokens` pricing fields.
pub(crate) const DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 200_000;

impl Pricing {
    /// Returns the per-token rate for cache creation input.
    pub fn cache_creation_input_token_cost(&self) -> f64 {
        self.cache_create
    }

    /// Returns the per-token rate for cache creation input above the long-context threshold.
    pub fn cache_creation_input_token_cost_above_200k_tokens(&self) -> Option<f64> {
        self.cache_create_above_200k
    }

    const fn empty() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_create: 0.0,
            cache_read: 0.0,
            cache_read_explicit: false,
            cache_create_explicit: false,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        }
    }
}

const DEEPSEEK_V4_PRICING_CUTOFF_MS: i64 = 1_786_896_000_000;

#[derive(Clone, Copy)]
struct DeepSeekV4Rates {
    input: f64,
    output: f64,
    cache_create: f64,
    cache_read: f64,
}

fn deepseek_v4_model_identity(model: &str) -> Option<&'static str> {
    let normalized = normalized_pricing_key(model);
    match normalized.as_ref() {
        "deepseek-v4-flash" => Some("deepseek-v4-flash"),
        "deepseek-v4-pro" => Some("deepseek-v4-pro"),
        _ => None,
    }
}

/// Reports whether a model's pricing can vary by event timestamp.
pub fn has_time_dependent_pricing(model: &str) -> bool {
    let resolved_model = crate::model_aliases::resolve_model_name(model);
    deepseek_v4_model_identity(model)
        .or_else(|| deepseek_v4_model_identity(resolved_model.as_ref()))
        .is_some()
}

fn deepseek_v4_rates(model: &str, timestamp: TimestampMs) -> Option<DeepSeekV4Rates> {
    let (old, off_peak, peak) = match model {
        "deepseek-v4-flash" => (
            DeepSeekV4Rates {
                input: 0.14e-6,
                output: 0.28e-6,
                cache_create: 0.14e-6,
                cache_read: 0.0028e-6,
            },
            DeepSeekV4Rates {
                input: 0.22e-6,
                output: 0.66e-6,
                cache_create: 0.22e-6,
                cache_read: 0.007e-6,
            },
            DeepSeekV4Rates {
                input: 0.44e-6,
                output: 1.32e-6,
                cache_create: 0.44e-6,
                cache_read: 0.014e-6,
            },
        ),
        "deepseek-v4-pro" => (
            DeepSeekV4Rates {
                input: 0.435e-6,
                output: 0.87e-6,
                cache_create: 0.435e-6,
                cache_read: 0.003625e-6,
            },
            DeepSeekV4Rates {
                input: 0.66e-6,
                output: 1.98e-6,
                cache_create: 0.66e-6,
                cache_read: 0.022e-6,
            },
            DeepSeekV4Rates {
                input: 1.32e-6,
                output: 3.96e-6,
                cache_create: 1.32e-6,
                cache_read: 0.044e-6,
            },
        ),
        _ => return None,
    };
    if timestamp.as_millis() < DEEPSEEK_V4_PRICING_CUTOFF_MS {
        return Some(old);
    }
    Some(if deepseek_v4_peak(timestamp) {
        peak
    } else {
        off_peak
    })
}

fn deepseek_v4_peak(timestamp: TimestampMs) -> bool {
    // DeepSeek publishes these windows in UTC, so use the epoch instant rather
    // than a report's display timezone when deriving the calendar buckets.
    let days_since_epoch = timestamp.as_millis().div_euclid(MILLIS_PER_DAY);
    // Unix epoch Thursday is weekday 4 when Sunday is zero; Euclidean modulo
    // keeps the mapping valid for timestamps before the epoch as well.
    let weekday_from_sunday = (days_since_epoch + 4).rem_euclid(7);
    // Saturdays and Sundays are always off-peak.
    if !(1..=5).contains(&weekday_from_sunday) {
        return false;
    }
    // The published windows are half-open, so their ending hours are excluded.
    let hour = timestamp.as_millis().rem_euclid(MILLIS_PER_DAY) / MILLIS_PER_HOUR;
    (1..4).contains(&hour) || (6..10).contains(&hour)
}

fn apply_deepseek_v4_schedule(
    model: &str,
    timestamp: TimestampMs,
    mut pricing: Pricing,
) -> Pricing {
    let Some(rates) = deepseek_v4_rates(model, timestamp) else {
        return pricing;
    };
    pricing.input = rates.input;
    pricing.output = rates.output;
    pricing.cache_create = rates.cache_create;
    pricing.cache_read = rates.cache_read;
    pricing.input_above_200k = Some(rates.input);
    pricing.output_above_200k = Some(rates.output);
    pricing.cache_create_above_200k = Some(rates.cache_create);
    pricing.cache_read_above_200k = Some(rates.cache_read);
    pricing.cache_create_explicit = true;
    pricing.cache_read_explicit = true;
    pricing
}

fn apply_explicit_pricing_override(pricing: &mut Pricing, override_value: &PricingOverride) {
    if let Some(value) = override_value.input_cost_per_token {
        pricing.input = value;
    }
    if let Some(value) = override_value.output_cost_per_token {
        pricing.output = value;
    }
    if let Some(value) = override_value.cache_creation_input_token_cost {
        pricing.cache_create = value;
        pricing.cache_create_explicit = true;
    }
    if let Some(value) = override_value.cache_read_input_token_cost {
        pricing.cache_read = value;
        pricing.cache_read_explicit = true;
    }
    if let Some(value) = override_value.input_cost_per_token_above_200k_tokens {
        pricing.input_above_200k = Some(value);
    }
    if let Some(value) = override_value.output_cost_per_token_above_200k_tokens {
        pricing.output_above_200k = Some(value);
    }
    if let Some(value) = override_value.cache_creation_input_token_cost_above_200k_tokens {
        pricing.cache_create_above_200k = Some(value);
    }
    if let Some(value) = override_value.cache_read_input_token_cost_above_200k_tokens {
        pricing.cache_read_above_200k = Some(value);
    }
    if let Some(value) = override_value.fast_multiplier {
        pricing.fast_multiplier = value;
    }
}

/// Whether a lookup may fall back to the fuzzy scan, or has to answer from
/// exact entries because a fuzzy match would shadow an exact-only id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fuzzy {
    Allowed,
    Denied,
}

#[derive(Debug)]
pub struct PricingMap {
    entries: FxHashMap<String, Pricing>,
    /// Direct model overrides reapply only the fields explicitly supplied by
    /// the user after any timestamp-based schedule has been selected.
    user_overrides: FxHashMap<String, PricingOverride>,
    /// Entries that only a request recording that exact id may use.
    ///
    /// A separately priced tier such as `kimi-k2.7-code-highspeed` is the right
    /// rate only for a request that names it. Left in the fuzzy scan it wins over
    /// the base model, because that scan prefers the longest matching key, so
    /// `kimi-k2-7-code` would be billed at the premium tier.
    exact_only: ExactOnlyKeys,
    context_limits: FxHashMap<String, u64>,
    enable_models_dev_fallback: bool,
    enable_embedded_models_dev_fallback: bool,
    find_cache: OnceLock<Mutex<FxHashMap<String, Option<Pricing>>>>,
}

/// The ids of [`PricingMap::exact_only`], indexed both as written and under the
/// separator-normalized spelling `pricing_key_matches` compares.
///
/// The gate that keeps a request naming an exact-only id off the fuzzy scan has
/// to recognize the same spellings that scan does, or `claude-opus-5@eu` written
/// as `claude-opus-5-eu` passes the gate and is billed at the base model's rate,
/// which is exactly what marking the id exact-only was meant to prevent. That
/// spelling names the tier, so it resolves to it rather than losing its rate.
#[derive(Debug, Default)]
struct ExactOnlyKeys {
    raw: FxHashSet<String>,
    /// Every id under its normalized spelling, mapped back to the id it names.
    /// An id spelled with dashes alone is indexed too, or a request writing it
    /// with `.` or `@` would answer neither membership question; `None` marks a
    /// spelling two exact-only ids share, which therefore names neither on its
    /// own.
    normalized: FxHashMap<String, Option<String>>,
}

impl ExactOnlyKeys {
    fn insert(&mut self, key: String) {
        self.normalized
            .entry(normalized_pricing_key(&key).into_owned())
            .and_modify(|named| {
                if named.as_deref() != Some(key.as_str()) {
                    *named = None;
                }
            })
            .or_insert_with(|| Some(key.clone()));
        self.raw.insert(key);
    }

    fn remove(&mut self, key: &str) {
        if !self.raw.remove(key) {
            return;
        }
        let normalized = normalized_pricing_key(key).into_owned();
        // Another id can share the normalized spelling, and dropping it while
        // that id is still exact-only would reopen the gate for both.
        let mut sharing = self
            .raw
            .iter()
            .filter(|other| normalized_pricing_key(other) == normalized);
        let only = sharing.next().cloned();
        let shared = sharing.next().is_some();
        match only {
            None => {
                self.normalized.remove(&normalized);
            }
            Some(only) => {
                self.normalized
                    .insert(normalized, (!shared).then_some(only));
            }
        }
    }

    /// Whether the id is exact-only as written, the question the fuzzy scan asks
    /// of its own candidate keys.
    fn contains(&self, key: &str) -> bool {
        self.raw.contains(key)
    }

    /// Whether the id is any spelling of an exact-only id, the question a
    /// requested model name has to answer before it may be fuzzy-matched.
    fn contains_any_spelling(&self, key: &str) -> bool {
        self.raw.contains(key)
            || self
                .normalized
                .contains_key(normalized_pricing_key(key).as_ref())
    }

    /// The exact-only id an alternate separator spelling names, if exactly one
    /// does. The fuzzy scan already treats those spellings as one id, so this
    /// only decides which entry answers, not whether they match.
    fn id_spelled_by(&self, key: &str) -> Option<&str> {
        self.normalized
            .get(normalized_pricing_key(key).as_ref())?
            .as_deref()
    }
}

impl Default for PricingMap {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
            user_overrides: FxHashMap::default(),
            exact_only: ExactOnlyKeys::default(),
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
    /// models.dev sets this to the catalog's directory name, which is also the
    /// key it is filed under. The generator reads `provider.id ?? providerId`, so
    /// prefer it here too rather than relying on the two agreeing.
    id: Option<String>,
    models: FxHashMap<String, ModelsDevModel>,
}

/// The selection rules for reading a live models.dev response, generated from
/// the pinned catalog by `just gen-models-dev-pricing`.
///
/// models.dev repeats every model once per catalog that serves it, and reseller
/// catalogs carry their own promotions, markups, and looser descriptions. The
/// live `api.json` records neither who authored a model nor the authored
/// modalities, so both have to be carried in from generation time; without them
/// the online path would make different decisions than the embedded snapshot.
///
/// The artifact also carries the tiers each author prices itself, which the
/// generator's own rules use; the loader has no decision left that distinguishes
/// them, so the field is simply not read here.
#[derive(Debug, Deserialize)]
struct ModelsDevCatalogRules {
    /// Catalogs of the providers that author models.
    owners: FxHashSet<String>,
    /// Cloud platforms that resell at list price plus a published regional
    /// premium, and the only source of platform-specific model ids.
    platforms: FxHashSet<String>,
    /// Every model the authored catalog lists. One that is absent from
    /// `asset_priced_model_ids` is authored as token-priced, which settles it
    /// whatever the catalog serving it claims.
    #[serde(rename = "authoredModelIds")]
    authored_model_ids: FxHashSet<String>,
    /// Models the authored catalog prices per asset - per second of audio, per
    /// generated image - rather than per token.
    #[serde(rename = "assetPricedModelIds")]
    asset_priced_model_ids: FxHashSet<String>,
    /// `authored_model_ids` normalized the way `normalizeModelId` normalizes them,
    /// for the prefix comparison the tier check makes. Derived after parsing
    /// rather than carried, because it is the same data spelled differently.
    #[serde(skip)]
    normalized_authored_model_ids: Vec<String>,
}

/// How strong one catalog's claim on a model id is.
///
/// Ordered exactly as `shouldReplaceModelsDevPricingCandidate` compares
/// candidates at generation time - trust first, then a long-context band, then
/// cache-read, cache-write and context-limit presence - so the online path
/// resolves the same catalog the committed snapshot did. `derive(PartialOrd)`
/// compares the fields in declaration order, which is that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ModelsDevClaim {
    trust: u8,
    has_long_context_tier: bool,
    has_cache_read: bool,
    has_cache_write: bool,
    has_context_limit: bool,
}

/// The spelling and strength under which a model id was claimed this pass.
#[derive(Debug, Clone)]
struct ModelsDevClaimSlot {
    claim: ModelsDevClaim,
    stored_id: String,
    /// The declared id of the catalog that claimed it, and the source key it
    /// claimed under: generation breaks exact-strength ties by
    /// `(sourceProviderId, sourceModelId)`, and two catalogs can declare the
    /// same provider id.
    provider_id: String,
    source_key: String,
}

/// Trust tiers, matching the generator's.
const MODELS_DEV_TRUST_OWNER: u8 = 3;
const MODELS_DEV_TRUST_PLATFORM: u8 = 2;
const MODELS_DEV_TRUST_RESELLER: u8 = 1;

impl ModelsDevCatalogRules {
    fn rank(&self, provider_id: &str) -> u8 {
        if self.owners.contains(provider_id) {
            return MODELS_DEV_TRUST_OWNER;
        }
        if self.platforms.contains(provider_id) {
            return MODELS_DEV_TRUST_PLATFORM;
        }
        MODELS_DEV_TRUST_RESELLER
    }

    /// Whether only a request naming this id exactly may use its rates, the
    /// verdict the generator records as `exactOnly` on the embedded snapshot.
    ///
    /// A live models.dev response carries no such field, so the online refresh
    /// has to reach the same verdict from the same rules or the fuzzy lookup
    /// would resolve a premium tier for the base model it shadows.
    ///
    /// The tier check reads the catalog's own key, as generation does, while the
    /// unversioned check reads the pricing key the entry resolves to, because
    /// that is the key the fuzzy scan would offer as a candidate.
    fn is_exact_only(&self, source_model_id: &str, pricing_key: &str) -> bool {
        self.is_tier_variant_of_authored_model(source_model_id)
            || is_unversioned_models_dev_model_id(pricing_key)
    }

    /// Whether an id names a separately priced tier of a model the catalog also
    /// carries under its base id, such as `kimi-k2.6-nitro`, `glm-5.2-flex` or
    /// `claude-opus-5-fast`, following `isTierVariantOfAuthoredModel` with its
    /// `includeAuthorPricedModes` option: the shadowing hazard does not care who
    /// set the rate.
    ///
    /// Only bare ids qualify: an id carrying a provider path is that gateway's
    /// addressing of a model rather than a distinct tier of it, and it has to
    /// stay fuzzy-reachable so the gateway's own tier spellings still resolve.
    fn is_tier_variant_of_authored_model(&self, source_model_id: &str) -> bool {
        if source_model_id.contains('/') {
            return false;
        }
        let normalized = normalized_models_dev_model_id(source_model_id);
        self.normalized_authored_model_ids.iter().any(|authored| {
            normalized
                .strip_prefix(authored.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
        })
    }

    /// Whether the embedded `input` and `output` rates mean per-token rates.
    ///
    /// The authored catalog decides both ways where it knows the model: a
    /// reseller describing an image model as text-only must not let a per-image
    /// rate through, and one describing a token-priced model as image-output
    /// must not drop a model the snapshot carries. Only models the authored
    /// catalog never listed fall back to the serving catalog's own modalities.
    ///
    /// `source_model_id` is the catalog's own key for the model, the same id
    /// `isTokenPricedModel` is given at generation time, because that is what
    /// `assetPricedModelIds` records - not the pricing key the entry's `id`
    /// field may resolve to.
    fn is_token_priced(
        &self,
        source_model_id: &str,
        modalities: Option<&ModelsDevModalities>,
    ) -> bool {
        // The artifact lists are normalized, so a catalog spelling the model
        // with different separators or case still gets the authored verdict.
        let normalized = normalized_models_dev_model_id(source_model_id);
        if self.asset_priced_model_ids.contains(normalized.as_ref()) {
            return false;
        }
        if self.authored_model_ids.contains(normalized.as_ref()) {
            return true;
        }
        let Some(modalities) = modalities else {
            return true;
        };
        // An absent list says nothing about the model, so it reads as plain text
        // rather than disqualifying the entry. An explicitly empty one is not
        // text, which is how `isTokenPricedModel` reads it too: it defaults only
        // a missing list to `['text']`.
        let text_only_output = match modalities.output.as_deref() {
            None => true,
            Some([single]) => single == "text",
            Some(_) => false,
        };
        let accepts_text = match modalities.input.as_deref() {
            None => true,
            Some(input) => input.iter().any(|modality| modality == "text"),
        };
        text_only_output && accepts_text
    }
}

/// Model ids are spelled with dots, dashes or an `@` regional suffix for the
/// same model, as `normalizeModelId` reads them. Missing `@` here left Vertex
/// ids such as `claude-opus-5@eu` unrecognized as tiers of the model they
/// shadow, which the snapshot marks exact-only.
fn normalized_models_dev_model_id(model_id: &str) -> Cow<'_, str> {
    if model_id
        .bytes()
        .any(|byte| byte == b'.' || byte == b'@' || byte.is_ascii_uppercase())
    {
        Cow::Owned(model_id.to_lowercase().replace(['.', '@'], "-"))
    } else {
        Cow::Borrowed(model_id)
    }
}

/// Whether an id names no particular model, as `isUnversionedModelId` reads it.
///
/// A model id nearly always carries a version, so an id with no digit at all is
/// a family or a routing label - models.dev publishes one called `auto` - and it
/// is short enough to be a substring of ids it has nothing to do with.
fn is_unversioned_models_dev_model_id(model_id: &str) -> bool {
    !model_id.bytes().any(|byte| byte.is_ascii_digit())
}

fn models_dev_catalog_rules() -> &'static ModelsDevCatalogRules {
    static RULES: OnceLock<ModelsDevCatalogRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules: ModelsDevCatalogRules =
            serde_json::from_str(models_dev_catalog_rules_json())
                .expect("parse embedded models-dev-catalog-rules.json");
        rules.normalized_authored_model_ids = rules
            .authored_model_ids
            .iter()
            .map(|model_id| normalized_models_dev_model_id(model_id).into_owned())
            .collect();
        rules.normalized_authored_model_ids.sort();
        rules
    })
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
    modalities: Option<ModelsDevModalities>,
    /// Set by the snapshot generator on separately priced tiers, which are only
    /// the right rate for a request that names them.
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
    /// Above-base rate bands. The embedded snapshot keeps the upstream shape, so
    /// this reads a live `api.json` response and the snapshot alike.
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
    /// The band a request crosses first, in per-token rates.
    ///
    /// `Pricing` holds one above-base band, so the lowest context threshold wins:
    /// a handful of models publish a second, higher one, and dropping the lower
    /// would price everything between the two thresholds at the base rate.
    /// Bands keyed by anything but context are skipped, because the runtime
    /// compares the threshold against an input-token count.
    fn long_context_tier(&self) -> Option<LongContextRates> {
        self.tiers
            .as_deref()?
            .iter()
            .filter_map(|tier| {
                let bound = tier.tier.as_ref()?;
                if bound.kind.as_deref() != Some("context") {
                    return None;
                }
                let threshold = bound.size.filter(|size| *size > 0)?;
                Some(LongContextRates {
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

/// models.dev publishes rates per million tokens; everything else here is per token.
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
        // A default family alias bills at the variant it points to, so it
        // shares that variant's Fast multiplier.
        if let Some(multiplier) = pricing_alias(model).and_then(|alias| self.exact.get(alias)) {
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
    pub fn load_embedded() -> Self {
        let mut map = Self::default();
        let fast_multiplier_overrides = FastMultiplierOverrides::load();
        map.load_json_with_overrides(build_time_pricing_json(), &fast_multiplier_overrides);
        map.put_builtin_pricing(&fast_multiplier_overrides);
        map.fill_long_context_rates_from_models_dev();
        // Resolve models that LiteLLM and the built-in table miss from the
        // embedded models.dev snapshot. This works offline, unlike the network
        // source gated by `enable_models_dev_fallback`.
        map.enable_embedded_models_dev_fallback = true;
        map
    }

    pub fn load_with_overrides<'a, I>(offline: bool, log: bool, overrides: I) -> Self
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
        map.fill_long_context_rates_from_models_dev();
        map.enable_models_dev_fallback = !offline;
        map.apply_overrides(overrides);
        map
    }

    pub fn load_json(&mut self, json: &str) -> usize {
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
            let cache_create_explicit = pricing.cache_creation_input_token_cost.is_some();
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
                    cache_create_explicit,
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
            ModelsDevJson::Providers(providers) => {
                let rules = models_dev_catalog_rules();
                let mut ranked: Vec<_> = providers.into_iter().collect();
                // The most trustworthy catalog has to be loaded first, because it
                // claims the model id. Sorting by id within a tier keeps the
                // result from depending on hash iteration order.
                let provider_id = |key: &String, provider: &ModelsDevProvider| {
                    provider.id.clone().unwrap_or_else(|| key.clone())
                };
                ranked.sort_by(|(left_key, left), (right_key, right)| {
                    let left_id = provider_id(left_key, left);
                    let right_id = provider_id(right_key, right);
                    rules
                        .rank(&right_id)
                        .cmp(&rules.rank(&left_id))
                        .then_with(|| left_id.cmp(&right_id))
                        // Two catalogs can declare the same id; the map key is
                        // unique, so it settles the order the way generation's
                        // provider-key walk does.
                        .then_with(|| left_key.cmp(right_key))
                });
                // Within one tier the generator prefers the entry carrying more
                // pricing detail, so track what claimed each id to make the same
                // choice here rather than keeping whichever came first.
                let mut claims: FxHashMap<String, ModelsDevClaimSlot> = FxHashMap::default();
                ranked
                    .into_iter()
                    .map(|(provider_key, provider)| {
                        let provider_id = provider.id.unwrap_or(provider_key);
                        let trust = rules.rank(&provider_id);
                        // A live catalog is the raw upstream shape, so none of
                        // generation's verdicts are recorded in it.
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
                // A flat map is the generated snapshot, which carries its
                // verdicts as fields rather than leaving them to be rederived.
                // The flat snapshot has one implicit source, so any constant id
                // works: ties inside it are settled by the source keys.
                self.load_models_dev_models(models, "", MODELS_DEV_TRUST_OWNER, false, &mut claims)
            }
        })
    }

    /// Load the entries of one provider catalog.
    ///
    /// `claims` records the claim strength of whichever catalog supplied each
    /// model id so far in this pass, so a better candidate can replace a weaker
    /// one. Ids absent from it belong to another pricing source and are left
    /// alone.
    ///
    /// `derive_exact_only` recomputes the generator's `exactOnly` verdict for
    /// payloads that do not carry it, which is every live models.dev response.
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
        // Two source keys in one catalog can resolve to the same model id, and the
        // generator walks each catalog in key order, so match that instead of
        // depending on hash iteration order.
        let mut sources: Vec<_> = models.into_iter().collect();
        sources.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (model_key, model) in sources {
            // Asked of the catalog's source key rather than the pricing key it
            // resolves to, because generation asks that way: an authored
            // asset-priced model served under a different `id` would otherwise
            // slip past and bill per-image or per-second rates as per-token ones.
            // It is generation's only remaining gate, so the online refresh
            // carries the same ids the snapshot does.
            let Some(cost) = models_dev_usable_cost(rules, &model_key, &model) else {
                continue;
            };
            // Same reason: the tier half of the verdict reads the source key,
            // before it is resolved away.
            let declared_id = model.id.as_deref().filter(|id| !id.is_empty());
            let exact_only = model.exact_only.unwrap_or_else(|| {
                derive_exact_only
                    && rules.is_exact_only(&model_key, declared_id.unwrap_or(&model_key))
            });
            let source_key = model_key;
            // An empty declared id falls back to the source key, exactly as the
            // generator's `selectModelsDevPricingKey` does: keeping "" would
            // store the model under a name no lookup ever asks for.
            let model_id = match model.id.as_ref().filter(|id| !id.is_empty()) {
                Some(id) => id.clone(),
                None => source_key.clone(),
            };
            // Dotted, dashed and case spellings name one model, so they contend
            // for one slot; kept apart, the fuzzy lookup ties between a tiered
            // spelling and a reseller's flat one and can pick either. Normalized
            // exactly as the generator normalizes, case folding included.
            let normalized_id = normalized_models_dev_model_id(&model_id).into_owned();
            let claimed = claims.get(&normalized_id).cloned();
            if claimed.is_none() && self.entries.contains_key(&model_id) {
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
                // Generation compares equal-strength candidates by
                // `(sourceProviderId, sourceModelId)`, smaller first. Providers
                // load here in ascending declared-id order, so a differing
                // declared id is already settled by arrival; only a collision
                // falls through to the source keys.
                previous.claim == claim
                    && previous.provider_id == provider_id
                    && source_key < previous.source_key
            };
            if claimed.as_ref().is_some_and(|claimed| {
                claimed.claim > claim || (claimed.claim == claim && !replaces_equal_claim(claimed))
            }) {
                continue;
            }
            // A replacement under a different spelling supersedes the losing
            // spelling's entry entirely, or both would stay findable and the
            // fuzzy tie would return.
            if let Some(previous) = claimed
                .as_ref()
                .filter(|previous| previous.stored_id != model_id)
            {
                self.entries.remove(&previous.stored_id);
                self.exact_only.remove(&previous.stored_id);
                self.context_limits.remove(&previous.stored_id);
            }
            let input = input / 1_000_000.0;
            let output = output / 1_000_000.0;
            let cache_read_explicit = cost.cache_read.is_some();
            let cache_create_explicit = cost.cache_write.is_some();
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
                    cache_create_explicit,
                    input_above_200k: long_context.and_then(|rates| rates.input),
                    output_above_200k: long_context.and_then(|rates| rates.output),
                    cache_create_above_200k: long_context.and_then(|rates| rates.cache_create),
                    cache_read_above_200k: long_context.and_then(|rates| rates.cache_read),
                    long_context_threshold: long_context.map(|rates| rates.threshold),
                    fast_multiplier: 1.0,
                },
            );
            if exact_only {
                self.exact_only.insert(model_id.clone());
            } else {
                self.exact_only.remove(&model_id);
            }
            match context_limit {
                Some(context_limit) => {
                    self.context_limits.insert(model_id.clone(), context_limit);
                }
                // A replacement that publishes no limit must not keep the limit of
                // the catalog it replaced.
                None if claimed.is_some() => {
                    self.context_limits.remove(&model_id);
                }
                None => {}
            }
            // Replacing a weaker claim is not a new model, so it must not count
            // twice: the count is how many models the payload resolved.
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

    pub fn find(&self, model: &str) -> Option<Pricing> {
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
        let fuzzy = self.allows_fuzzy_lookup(model, resolved_alias);
        let result = self
            .find_entry_or_alias(model, fuzzy)
            .or_else(|| {
                (resolved_alias != model)
                    .then(|| self.find_entry_or_alias(resolved_alias, Fuzzy::Allowed))
                    .flatten()
            })
            .or_else(|| {
                self.enable_models_dev_fallback
                    .then(|| {
                        models_dev_pricing().and_then(|pricing| {
                            pricing.find_entry_or_alias(resolved_alias, Fuzzy::Allowed)
                        })
                    })
                    .flatten()
            })
            // The embedded models.dev snapshot is a separate map, so it only
            // resolves models the primary table misses and never perturbs its
            // fuzzy alias matching. It works offline, unlike the network source.
            .or_else(|| {
                self.enable_embedded_models_dev_fallback
                    .then(|| {
                        embedded_models_dev_pricing()
                            .find_entry_or_alias(resolved_alias, Fuzzy::Allowed)
                    })
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

    /// Finds the pricing that applies to one usage event.
    ///
    /// The timestamp-aware DeepSeek schedule is applied to the two direct model
    /// ids and aliases that resolve to them. Provider and reseller names
    /// continue through static lookup.
    pub fn find_at(&self, model: &str, timestamp: TimestampMs) -> Option<Pricing> {
        let resolved_model = crate::model_aliases::resolve_model_name(model);
        let Some(scheduled_model) = deepseek_v4_model_identity(model)
            .or_else(|| deepseek_v4_model_identity(resolved_model.as_ref()))
        else {
            return self.find(model);
        };
        let mut pricing = apply_deepseek_v4_schedule(scheduled_model, timestamp, self.find(model)?);
        let override_value = self
            .user_overrides
            .get(model)
            .or_else(|| {
                (resolved_model.as_ref() != model)
                    .then(|| self.user_overrides.get(resolved_model.as_ref()))
                    .flatten()
            })
            .or_else(|| self.user_overrides.get(scheduled_model));
        if let Some(override_value) = override_value {
            apply_explicit_pricing_override(&mut pricing, override_value);
        }
        Some(pricing)
    }

    pub fn find_exact(&self, model: &str) -> Option<Pricing> {
        self.entries.get(model).copied()
    }

    /// Finds pricing by exact model id in the primary map or an enabled
    /// models.dev fallback.
    ///
    /// Unlike [`Self::find`], this lookup does not resolve aliases or use
    /// separator and fuzzy matching.
    pub fn find_exact_with_fallback(&self, model: &str) -> Option<Pricing> {
        self.find_exact_normalized(model)
            .or_else(|| {
                self.enable_models_dev_fallback
                    .then(|| {
                        models_dev_pricing()
                            .and_then(|pricing| pricing.find_exact_normalized(model))
                    })
                    .flatten()
            })
            .or_else(|| {
                self.enable_embedded_models_dev_fallback
                    .then(|| embedded_models_dev_pricing().find_exact_normalized(model))
                    .flatten()
            })
    }

    fn find_exact_normalized(&self, model: &str) -> Option<Pricing> {
        self.find_exact(model).or_else(|| {
            if self.exact_only.contains_any_spelling(model) {
                return self
                    .exact_only
                    .id_spelled_by(model)
                    .and_then(|id| self.entries.get(id).copied());
            }

            let normalized_model = normalized_pricing_key(model);
            let mut matches = self.entries.iter().filter(|(candidate, _)| {
                normalized_pricing_key(candidate).as_ref() == normalized_model.as_ref()
            });
            let (_, pricing) = matches.next()?;
            matches.next().is_none().then_some(*pricing)
        })
    }

    fn find_entry_or_alias(&self, model: &str, fuzzy: Fuzzy) -> Option<Pricing> {
        self.entries
            .get(model)
            .copied()
            .or_else(|| pricing_alias(model).and_then(|alias| self.find_entry(alias, fuzzy)))
            .or_else(|| self.find_entry(model, fuzzy))
    }

    fn find_entry(&self, model: &str, fuzzy: Fuzzy) -> Option<Pricing> {
        self.entries.get(model).copied().or_else(|| {
            // A separator spelling of an exact-only id names that entry, so it
            // is answered from it rather than gated into a miss below.
            if let Some(id) = self.exact_only.id_spelled_by(model) {
                return self.entries.get(id).copied();
            }
            if fuzzy == Fuzzy::Denied || self.is_exact_only_lookup(model) {
                return None;
            }
            let normalized_model = normalized_pricing_key(model);
            self.entries
                .iter()
                .filter(|(candidate, _)| !self.exact_only.contains(candidate.as_str()))
                .filter(|(candidate, _)| {
                    pricing_key_matches(candidate, model, normalized_model.as_ref())
                })
                .max_by(|(left, _), (right, _)| {
                    left.len().cmp(&right.len()).then_with(|| right.cmp(left))
                })
                .map(|(_, pricing)| *pricing)
        })
    }

    /// Whether `model` names an entry that only an exact request may use, which
    /// is the other half of the `exact_only` rule: such a key must not be
    /// answered by a fuzzy match on a different model either, or the tier it
    /// names is unreachable and the base model's rate is billed for it.
    ///
    /// The check has to reach past this map because the embedded models.dev
    /// snapshot is a separate map consulted only after the primary one misses:
    /// `claude-opus-5-fast` would otherwise fuzzy-match LiteLLM's
    /// `claude-opus-5` here and never reach the snapshot's tier entry. Only a
    /// carried entry gates the scan, so a key nothing prices exactly still falls
    /// back to fuzzy matching rather than losing its rate.
    ///
    /// The network catalog is deliberately not consulted: reading it would fetch
    /// models.dev before the primary lookup has even missed, and it is generated
    /// from the same rules as the snapshot the check already reads.
    /// Membership is asked of every spelling of the id, not just the one the
    /// catalog wrote: the fuzzy scan the gate protects matches `.`, `@` and `-`
    /// interchangeably, so `claude-opus-5-eu` reaches the base model's entry
    /// just as `claude-opus-5@eu` would and has to be gated with it.
    fn is_exact_only_lookup(&self, model: &str) -> bool {
        self.exact_only.contains_any_spelling(model)
            || (self.enable_embedded_models_dev_fallback
                && embedded_models_dev_pricing()
                    .exact_only
                    .contains_any_spelling(model))
    }

    /// Whether the spelling a lookup recorded may be fuzzy-matched at all.
    ///
    /// A `CCUSAGE_MODEL_ALIASES` entry pointing at an exact-only id is tried
    /// only after this map has answered for the recorded spelling, so a spelling
    /// that fuzzy-matches some other model - `claude-opus-5-turbo` aliased to
    /// `claude-opus-5-fast` matches LiteLLM's `claude-opus-5` - would be billed
    /// at that model's rate and never reach the tier the alias names. Exact
    /// entries for the recorded spelling still win, as they do without an alias.
    fn allows_fuzzy_lookup(&self, model: &str, resolved_alias: &str) -> Fuzzy {
        if resolved_alias != model && self.is_exact_only_lookup(resolved_alias) {
            return Fuzzy::Denied;
        }
        Fuzzy::Allowed
    }

    pub fn context_limit(&self, model: &str) -> Option<u64> {
        let alias = crate::model_aliases::resolve_model_name(model);
        let resolved_alias = alias.as_ref();
        let fuzzy = self.allows_fuzzy_lookup(model, resolved_alias);
        self.context_limit_entry_or_alias(model, fuzzy)
            .or_else(|| {
                (resolved_alias != model)
                    .then(|| self.context_limit_entry_or_alias(resolved_alias, Fuzzy::Allowed))
                    .flatten()
            })
            .or_else(|| {
                self.enable_models_dev_fallback
                    .then(|| {
                        models_dev_pricing().and_then(|pricing| {
                            pricing.context_limit_entry_or_alias(resolved_alias, Fuzzy::Allowed)
                        })
                    })
                    .flatten()
            })
            .or_else(|| {
                self.enable_embedded_models_dev_fallback
                    .then(|| {
                        embedded_models_dev_pricing()
                            .context_limit_entry_or_alias(resolved_alias, Fuzzy::Allowed)
                    })
                    .flatten()
            })
    }

    fn context_limit_entry_or_alias(&self, model: &str, fuzzy: Fuzzy) -> Option<u64> {
        self.context_limits
            .get(model)
            .copied()
            .or_else(|| {
                pricing_alias(model).and_then(|alias| self.context_limit_entry(alias, fuzzy))
            })
            .or_else(|| self.context_limit_entry(model, fuzzy))
    }

    fn context_limit_entry(&self, model: &str, fuzzy: Fuzzy) -> Option<u64> {
        self.context_limits.get(model).copied().or_else(|| {
            if let Some(id) = self.exact_only.id_spelled_by(model) {
                return self.context_limits.get(id).copied();
            }
            if fuzzy == Fuzzy::Denied || self.is_exact_only_lookup(model) {
                return None;
            }
            let normalized_model = normalized_pricing_key(model);
            self.context_limits
                .iter()
                .filter(|(candidate, _)| !self.exact_only.contains(candidate.as_str()))
                .filter(|(candidate, _)| {
                    pricing_key_matches(candidate, model, normalized_model.as_ref())
                })
                .max_by(|(left, _), (right, _)| {
                    left.len().cmp(&right.len()).then_with(|| right.cmp(left))
                })
                .map(|(_, context_limit)| *context_limit)
        })
    }

    fn apply_overrides<'a, I>(&mut self, overrides: I)
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        for (model, override_value) in overrides {
            self.apply_override(model, override_value);
        }
        self.clear_find_cache();
    }

    fn apply_override(&mut self, model: &str, override_value: &PricingOverride) {
        self.user_overrides
            .insert(model.to_string(), override_value.clone());
        let base = self
            .entries
            .get(model)
            .copied()
            .or_else(|| pricing_alias(model).and_then(|alias| self.entries.get(alias).copied()))
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
            cache_create_explicit: override_value.cache_creation_input_token_cost.is_some()
                || base.cache_create_explicit,
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
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn models_dev_fallback_enabled(&self) -> bool {
        self.enable_models_dev_fallback
    }

    /// Fills in long-context tier rates for models whose upstream pricing
    /// entries only carry the flat rates, from the embedded models.dev
    /// snapshot's `cost.tiers`. LiteLLM publishes no long-context rates for
    /// OpenAI or xAI at all, so without this every request above the boundary
    /// is billed at the base rate. Runs after every pricing load (embedded and
    /// live) because LiteLLM refreshes replace whole entries.
    ///
    /// Entries that already carry any tier rate are left untouched so upstream
    /// data wins once it exists. The check is deliberately all-or-nothing
    /// rather than per field: each source's rates assume that source's own
    /// boundary, so mixing fields across sources would price both tiers wrong.
    fn fill_long_context_rates_from_models_dev(&mut self) {
        let tiers = embedded_models_dev_pricing();
        for (model, pricing) in &mut self.entries {
            if pricing.input_above_200k.is_some()
                || pricing.output_above_200k.is_some()
                || pricing.cache_create_above_200k.is_some()
                || pricing.cache_read_above_200k.is_some()
            {
                continue;
            }
            // Only exact ids are consulted (after date-suffix and alias
            // resolution): a fuzzy match here could graft one model's tier onto
            // another's base rates.
            let base = model_without_date_suffix(model);
            let resolved = pricing_alias(base).unwrap_or(base);
            let Some(source) = tiers
                .entries
                .get(resolved)
                .or_else(|| tiers.entries.get(base))
            else {
                continue;
            };
            if source.long_context_threshold.is_none() {
                continue;
            }
            pricing.input_above_200k = source.input_above_200k;
            pricing.output_above_200k = source.output_above_200k;
            pricing.cache_create_above_200k = source.cache_create_above_200k;
            pricing.cache_read_above_200k = source.cache_read_above_200k;
            pricing.long_context_threshold = source.long_context_threshold;
        }
        self.clear_find_cache();
    }

    fn put_builtin_entry(&mut self, model: String, pricing: Pricing) {
        self.entries.entry(model).or_insert(pricing);
    }

    /// z.ai's catalog needs one provider fact LiteLLM does not publish: GLM
    /// bills no cache writes, and its cache reads have their own rate. Base
    /// rates stay whatever the loaded snapshot says - overwriting them froze
    /// stale prices before - and only the cache fields are patched, and only
    /// when the snapshot did not publish them itself.
    fn put_builtin_glm(&mut self, model: &str, pricing: Pricing) {
        match self.entries.entry(model.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut existing) => {
                let existing = existing.get_mut();
                if !existing.cache_read_explicit {
                    existing.cache_read = pricing.cache_read;
                    existing.cache_read_explicit = true;
                }
                if !existing.cache_create_explicit {
                    existing.cache_create = pricing.cache_create;
                    existing.cache_create_explicit = true;
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(pricing);
            }
        }
    }

    /// Last-resort rates for models ccusage must always price, used only when
    /// the loaded snapshots miss the key: upstream data is refreshed hourly,
    /// so overwriting it with these frozen numbers would reintroduce stale
    /// prices whenever a vendor changes theirs (OpenAI cut the gpt-5.6 rates
    /// after these were written).
    fn put_builtin_pricing(&mut self, fast_multiplier_overrides: &FastMultiplierOverrides) {
        self.put_builtin_entry(
            "claude-opus-4-5".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-opus-4-6".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
        self.put_builtin_entry(
            "claude-opus-4-7".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
        self.put_builtin_entry(
            "claude-opus-4-8".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: 6.25e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
        self.put_builtin_entry(
            "claude-haiku-4-5".to_string(),
            Pricing {
                input: 1e-6,
                output: 5e-6,
                cache_create: 1.25e-6,
                cache_read: 0.1e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-sonnet-4-6".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-sonnet-4".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
            cache_create_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.put_builtin_entry("claude-3-5-haiku".to_string(), claude_3_5_haiku);
        self.put_builtin_entry("claude-3-5-haiku-20241022".to_string(), claude_3_5_haiku);
        self.put_builtin_entry(
            "claude-3-opus".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: 18.75e-6,
                cache_read: 1.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-3-sonnet".to_string(),
            Pricing {
                input: 3e-6,
                output: 15e-6,
                cache_create: 3.75e-6,
                cache_read: 0.3e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "claude-3-haiku".to_string(),
            Pricing {
                input: 0.25e-6,
                output: 1.25e-6,
                cache_create: 0.3e-6,
                cache_read: 0.03e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "gpt-5".to_string(),
            Pricing {
                input: 1.25e-6,
                output: 10e-6,
                cache_create: 1.25e-6,
                cache_read: 0.125e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "gpt-5.5".to_string(),
            Pricing {
                input: 5e-6,
                output: 30e-6,
                cache_create: 5e-6,
                cache_read: 0.5e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
        self.put_builtin_entry(
            "grok-4.3".to_string(),
            Pricing {
                input: 1.25e-6,
                output: 2.5e-6,
                cache_create: 1.25e-6,
                cache_read: 0.125e-6,
                cache_read_explicit: false,
                cache_create_explicit: false,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.kimi.ai/docs/pricing/chat-k25
        self.put_builtin_entry(
            "moonshot/kimi-k2.5".to_string(),
            Pricing {
                input: 0.6e-6,
                output: 3e-6,
                cache_create: 0.75e-6,
                cache_read: 0.1e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.kimi.ai/docs/pricing/chat-k26
        self.put_builtin_entry(
            "moonshot/kimi-k2.6".to_string(),
            Pricing {
                input: 0.95e-6,
                output: 4e-6,
                cache_create: 1.1875e-6,
                cache_read: 0.16e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
            cache_create_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.put_builtin_entry("gpt-5.1".to_string(), gpt_5_1_pricing);
        self.entries
            .insert("gpt-5.1-codex".to_string(), gpt_5_1_pricing);
        let gpt_5_codex_pricing = Pricing {
            input: 1.75e-6,
            output: 14e-6,
            cache_create: 1.75e-6,
            cache_read: 0.175e-6,
            cache_read_explicit: true,
            cache_create_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        self.entries
            .insert("gpt-5.2-codex".to_string(), gpt_5_codex_pricing);
        self.put_builtin_entry(
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
        self.put_builtin_entry(
            "gpt-5.4".to_string(),
            Pricing {
                input: 2.5e-6,
                output: 15e-6,
                cache_create: 2.5e-6,
                cache_read: 0.25e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
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
        self.put_builtin_entry(
            "gpt-5.4-mini".to_string(),
            Pricing {
                input: 0.75e-6,
                output: 4.5e-6,
                cache_create: 0.75e-6,
                cache_read: 0.075e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        self.put_builtin_entry(
            "gpt-5.4-nano".to_string(),
            Pricing {
                input: 0.2e-6,
                output: 1.25e-6,
                cache_create: 0.2e-6,
                cache_read: 0.02e-6,
                cache_read_explicit: true,
                cache_create_explicit: true,
                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
            },
        );
        // Source: https://platform.openai.com/docs/pricing (Standard tier,
        // short context). The long-context tier rates come from the embedded
        // models.dev snapshot via `fill_long_context_rates_from_models_dev`,
        // which runs after every pricing load.
        for (model, input, output, cache_create, cache_read) in [
            ("gpt-5.6-sol", 5e-6, 30e-6, 6.25e-6, 0.5e-6),
            ("gpt-5.6-terra", 2.5e-6, 15e-6, 3.125e-6, 0.25e-6),
            ("gpt-5.6-luna", 1e-6, 6e-6, 1.25e-6, 0.1e-6),
        ] {
            self.put_builtin_entry(
                model.to_string(),
                Pricing {
                    input,
                    output,
                    cache_create,
                    cache_read,
                    cache_read_explicit: true,
                    cache_create_explicit: true,
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
            cache_create_explicit: true,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
        };
        let glm_base = glm_pricing(0.6e-6, 2.2e-6, 0.11e-6);
        self.put_builtin_glm("glm-4.5", glm_base);
        self.put_builtin_glm("zai/glm-4.5", glm_base);
        self.put_builtin_glm("zai/glm-4.5-x", glm_pricing(2.2e-6, 8.9e-6, 0.45e-6));
        self.put_builtin_glm("zai/glm-4.5-air", glm_pricing(0.2e-6, 1.1e-6, 0.03e-6));
        self.put_builtin_glm("zai/glm-4.5-airx", glm_pricing(1.1e-6, 4.5e-6, 0.22e-6));
        self.put_builtin_glm("zai/glm-4.5v", glm_pricing(0.6e-6, 1.8e-6, 0.11e-6));
        self.put_builtin_glm("zai/glm-4-32b-0414-128k", glm_pricing(0.1e-6, 0.1e-6, 0.0));
        self.put_builtin_glm("zai/glm-4.5-flash", glm_pricing(0.0, 0.0, 0.0));
        self.put_builtin_glm("glm-4.6", glm_base);
        self.put_builtin_glm("glm-4.7", glm_base);
        self.put_builtin_entry(
            "glm-5".to_string(),
            Pricing {
                input: 1.0e-6,
                output: 3.2e-6,
                cache_read: 0.2e-6,
                ..glm_base
            },
        );
        self.put_builtin_entry(
            "glm-5-turbo".to_string(),
            Pricing {
                input: 1.2e-6,
                output: 4.0e-6,
                cache_read: 0.24e-6,
                ..glm_base
            },
        );
        self.put_builtin_entry(
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

/// Long-context tier rates (per token) for a base model, parsed from a
/// models.dev `cost.tiers` band and applied on top of loaded pricing entries
/// by `fill_long_context_rates_from_models_dev`.
#[derive(Clone, Copy)]
struct LongContextRates {
    threshold: u64,
    input: Option<f64>,
    output: Option<f64>,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
}

/// Input-token boundary above which a request is billed at a model's
/// long-context tier. Codex aggregates per-model token sums before pricing is
/// applied, so each request's tier must be decided during aggregation from the
/// model's own threshold rather than a single global constant. The boundary
/// comes from the embedded models.dev snapshot's `cost.tiers` - the same data
/// `fill_long_context_rates_from_models_dev` stamps onto
/// `Pricing::long_context_threshold` - and falls back to the default 200K
/// boundary used for LiteLLM `*_above_200k_tokens` data.
pub fn long_context_split_threshold(model: &str) -> u64 {
    let tiers = embedded_models_dev_pricing();
    let base = model_without_date_suffix(model);
    let resolved = pricing_alias(base).unwrap_or(base);
    tiers
        .entries
        .get(resolved)
        .or_else(|| tiers.entries.get(base))
        .and_then(|pricing| pricing.long_context_threshold)
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

/// Maps model aliases to canonical pricing keys before fuzzy matching.
fn pricing_alias(model: &str) -> Option<&'static str> {
    match model {
        "gpt-5.6" => Some("gpt-5.6-sol"),
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
        map.load_models_dev_json_missing(build_time_models_dev_json())
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
    if !map
        .load_models_dev_json_missing(&json)
        .is_some_and(|loaded_count| loaded_count > 0)
    {
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

/// Fetches a JSON document over HTTP for the pricing refresh.
pub type JsonFetcher = fn(&str) -> std::io::Result<String>;

static JSON_FETCHER: OnceLock<JsonFetcher> = OnceLock::new();

/// Installs the HTTP client used to refresh pricing.
///
/// The client lives with the binary rather than here so that its TLS stack is not
/// a dependency of the crate every adapter builds against. Without one installed,
/// a refresh reports that it is unavailable and the embedded snapshots are used,
/// which is what `--offline` already does.
pub fn set_json_fetcher(fetcher: JsonFetcher) {
    let _ = JSON_FETCHER.set(fetcher);
}

fn fetch_json_url(url: &str) -> std::io::Result<String> {
    let Some(fetch) = JSON_FETCHER.get() else {
        return Err(std::io::Error::other(
            "no HTTP client installed for pricing refresh",
        ));
    };
    fetch(url)
}

#[cfg(test)]
mod tests {
    use super::{
        Fuzzy, Pricing, PricingEndpoint, PricingMap, build_time_models_dev_json,
        build_time_pricing_json, embedded_models_dev_pricing, long_context_split_threshold,
        model_without_date_suffix,
    };
    use ccusage_test_support::fs_fixture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn loads_embedded_claude_pricing() {
        let pricing = PricingMap::load_embedded();
        assert!(pricing.len() > 0);
        assert!(pricing.find("claude-sonnet-4-20250514").is_some());
    }

    fn direct_deepseek_pricing() -> PricingMap {
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
                },
                "openrouter/deepseek-v4-flash": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010,
                    "cache_creation_input_token_cost": 0.000007,
                    "cache_read_input_token_cost": 0.000008
                }
            }"#,
        );
        pricing
    }

    fn timestamp(value: &str) -> crate::TimestampMs {
        crate::parse_ts_timestamp(value).unwrap()
    }

    #[test]
    fn applies_deepseek_v4_rates_by_effective_period_for_both_models() {
        let pricing = direct_deepseek_pricing();
        let cases = [
            (
                "2026-08-16T15:59:59Z",
                [0.14, 0.28, 0.0028],
                [0.435, 0.87, 0.003625],
            ),
            (
                "2026-08-16T16:00:00Z",
                [0.22, 0.66, 0.007],
                [0.66, 1.98, 0.022],
            ),
            (
                "2026-08-17T01:00:00Z",
                [0.44, 1.32, 0.014],
                [1.32, 3.96, 0.044],
            ),
        ];

        for (timestamp_text, flash_expected, pro_expected) in cases {
            let flash = pricing
                .find_at("deepseek-v4-flash", timestamp(timestamp_text))
                .unwrap();
            for (actual, expected) in [flash.input, flash.output, flash.cache_read]
                .map(|rate| rate * 1e6)
                .into_iter()
                .zip(flash_expected)
            {
                assert!((actual - expected).abs() < 1e-12);
            }

            let pro = pricing
                .find_at("deepseek-v4-pro", timestamp(timestamp_text))
                .unwrap();
            for (actual, expected) in [pro.input, pro.output, pro.cache_read]
                .map(|rate| rate * 1e6)
                .into_iter()
                .zip(pro_expected)
            {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn deepseek_v4_peak_windows_are_utc_weekday_half_open_and_offset_aware() {
        let pricing = direct_deepseek_pricing();
        let cases = [
            ("2026-08-17T00:59:59Z", 0.22),
            ("2026-08-17T01:00:00Z", 0.44),
            ("2026-08-17T03:59:59Z", 0.44),
            ("2026-08-17T04:00:00Z", 0.22),
            ("2026-08-17T05:59:59Z", 0.22),
            ("2026-08-17T06:00:00Z", 0.44),
            ("2026-08-17T09:59:59Z", 0.44),
            ("2026-08-17T10:00:00Z", 0.22),
            ("2026-08-22T02:00:00Z", 0.22),
            ("2026-08-17T09:00:00+08:00", 0.44),
        ];

        for (timestamp_text, expected_input_per_million) in cases {
            let flash = pricing
                .find_at("deepseek-v4-flash", timestamp(timestamp_text))
                .unwrap();
            assert!((flash.input * 1e6 - expected_input_per_million).abs() < 1e-12);
        }
    }

    #[test]
    fn deepseek_v4_schedule_requires_exact_direct_model_names_and_normalizes_cache_creation() {
        let pricing = direct_deepseek_pricing();
        let direct = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();
        assert_eq!(direct.cache_creation_input_token_cost() * 1e6, 0.44);

        let reseller = pricing
            .find_at(
                "openrouter/deepseek-v4-flash",
                timestamp("2026-08-17T01:00:00Z"),
            )
            .unwrap();
        let dotted_reseller = pricing
            .find_at(
                "openrouter/deepseek.v4.flash",
                timestamp("2026-08-17T01:00:00Z"),
            )
            .unwrap();
        assert_eq!(reseller.input * 1e6, 9.0);
        assert_eq!(reseller.output * 1e6, 10.0);
        assert_eq!(reseller.cache_read * 1e6, 8.0);
        assert_eq!(reseller.cache_creation_input_token_cost() * 1e6, 7.0);
        assert_eq!(dotted_reseller.input * 1e6, 9.0);
        assert_eq!(dotted_reseller.output * 1e6, 10.0);
    }

    #[test]
    fn find_at_applies_deepseek_schedule_after_model_alias_resolution() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "deepseek-latest",
            "deepseek-v4-flash",
        )]);
        let pricing = direct_deepseek_pricing();

        let resolved = pricing
            .find_at("deepseek-latest", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();

        assert_eq!(resolved.input * 1e6, 0.44);
    }

    #[test]
    fn find_at_applies_deepseek_schedule_to_separator_spellings_of_direct_models() {
        let pricing = direct_deepseek_pricing();

        for model in ["deepseek.v4.flash", "deepseek@v4@flash"] {
            let resolved = pricing
                .find_at(model, timestamp("2026-08-17T01:00:00Z"))
                .unwrap();
            assert!((resolved.input * 1e6 - 0.44).abs() < 1e-12, "{model}");
        }
    }

    #[test]
    fn time_dependent_pricing_excludes_provider_qualified_models() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "deepseek-latest",
            "deepseek-v4-flash",
        )]);

        for model in [
            "deepseek-v4-flash",
            "deepseek.v4.flash",
            "deepseek@v4@flash",
            "deepseek-latest",
        ] {
            assert!(super::has_time_dependent_pricing(model));
        }
        for model in [
            "openrouter/deepseek-v4-flash",
            "openrouter/deepseek.v4.flash",
            "gpt-5",
        ] {
            assert!(!super::has_time_dependent_pricing(model));
        }
    }

    #[test]
    fn find_at_replaces_stale_deepseek_long_context_rates() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.00000014,
                    "cache_read_input_token_cost": 0.0000000028,
                    "input_cost_per_token_above_200k_tokens": 0.000009,
                    "output_cost_per_token_above_200k_tokens": 0.000010,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.000011,
                    "cache_read_input_token_cost_above_200k_tokens": 0.000012
                }
            }"#,
        );

        let peak = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();

        assert_eq!(peak.input_above_200k, Some(0.44e-6));
        assert_eq!(peak.output_above_200k, Some(1.32e-6));
        assert_eq!(peak.cache_create_above_200k, Some(0.44e-6));
        assert_eq!(peak.cache_read_above_200k, Some(0.014e-6));
    }

    #[test]
    fn offline_embedded_pricing_uses_the_deepseek_v4_schedule() {
        let pricing = PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &ccusage_cli::PricingOverride)>(),
        );
        let flash = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();

        assert!((flash.input * 1e6 - 0.44).abs() < 1e-12);
        assert!((flash.output * 1e6 - 1.32).abs() < 1e-12);
        assert!((flash.cache_read * 1e6 - 0.014).abs() < 1e-12);
    }

    #[test]
    fn user_deepseek_v4_overrides_remain_authoritative() {
        let model = "deepseek-v4-flash".to_string();
        let override_value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(9e-6),
            output_cost_per_token: Some(10e-6),
            cache_creation_input_token_cost: Some(7e-6),
            cache_read_input_token_cost: Some(8e-6),
            ..Default::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);
        let flash = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();

        assert_eq!(flash.input * 1e6, 9.0);
        assert_eq!(flash.output * 1e6, 10.0);
        assert_eq!(flash.cache_read * 1e6, 8.0);
        assert_eq!(flash.cache_creation_input_token_cost() * 1e6, 7.0);
    }

    #[test]
    fn partial_deepseek_v4_override_keeps_schedule_for_unspecified_fields() {
        let model = "deepseek-v4-flash".to_string();
        let override_value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(9e-6),
            ..Default::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);

        let peak = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();
        assert_eq!(peak.input * 1e6, 9.0);
        assert_eq!(peak.output * 1e6, 1.32);
        assert_eq!(peak.cache_read * 1e6, 0.014);
        assert_eq!(peak.cache_creation_input_token_cost() * 1e6, 0.44);

        let off_peak = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T04:00:00Z"))
            .unwrap();
        assert_eq!(off_peak.input * 1e6, 9.0);
        assert_eq!(off_peak.output * 1e6, 0.66);
        assert_eq!(off_peak.cache_read * 1e6, 0.007);
        assert_eq!(off_peak.cache_creation_input_token_cost() * 1e6, 0.22);
    }

    #[test]
    fn validates_pricing_documents_per_endpoint() {
        let litellm =
            r#"{"gpt-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
        let models_dev =
            r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;
        let zero_loaded_models_dev = r#"{"openai":{"models":{"unpriced":{"modalities":{"input":[],"output":["text"]},"cost":{"input":1.0,"output":2.0}}}}}"#;

        assert!(PricingEndpoint::LiteLlm.validates(litellm));
        assert!(PricingEndpoint::LiteLlm.validates(r#"{"gpt-test":{"i":1.0,"o":2.0}}"#));
        assert!(!PricingEndpoint::LiteLlm.validates(models_dev));
        assert!(PricingEndpoint::ModelsDev.validates_shape(models_dev));
        assert!(PricingEndpoint::ModelsDev.validates(models_dev));
        assert!(!PricingEndpoint::ModelsDev.validates(litellm));
        assert!(!PricingEndpoint::ModelsDev.validates("{}"));
        assert!(!PricingEndpoint::ModelsDev.validates_shape(
            r#"{"openai":{"models":{"free":{"cost":{"input":0.0,"output":0.0}}}}}"#
        ));
        assert!(PricingEndpoint::ModelsDev.validates_shape(zero_loaded_models_dev));
        assert!(!PricingEndpoint::ModelsDev.validates(zero_loaded_models_dev));
        let mut pricing = PricingMap::default();
        assert_eq!(
            pricing.load_models_dev_json_missing(zero_loaded_models_dev),
            Some(0)
        );
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
    fn offline_prices_models_outside_the_claude_and_moonshot_families() {
        // The snapshot used to be filtered by a model-name pattern, so families
        // nobody had added to it were unpriced offline even when models.dev
        // published them and LiteLLM did not.
        let pricing = PricingMap::load_embedded();
        let grok_build = pricing
            .find("grok-build-0.1")
            .expect("embedded models.dev should include xAI pricing");

        // Compared per million tokens, which is how models.dev publishes rates.
        assert!((grok_build.input * 1e6 - 1.0).abs() < 1e-9);
        assert!((grok_build.output * 1e6 - 2.0).abs() < 1e-9);
        assert_eq!(pricing.context_limit("grok-build-0.1"), Some(256_000));
    }

    #[test]
    fn embedded_models_dev_omits_models_priced_per_asset() {
        // These rates are per second of audio and per generated image. The
        // runtime divides by a million and multiplies by token counts, so
        // embedding them reports a wrong cost rather than no cost.
        let embedded = embedded_models_dev_pricing();

        for model in [
            "whisper-large-v3",
            "gemini-2.5-flash-image",
            "gemini-3-pro-image-preview",
        ] {
            assert!(
                embedded.find_exact(model).is_none(),
                "{model} prices assets rather than text tokens and must stay out of the snapshot"
            );
        }
        // The guard keys on modalities, not on the name, so text models that
        // merely accept audio or video input have to survive it.
        assert!(embedded.find_exact("kimi-k3").is_some());
        assert!(embedded.find_exact("gemini-3-flash-preview").is_some());
    }

    #[test]
    fn embedded_models_dev_prices_resold_models_from_their_author() {
        // models.dev lists kimi-k2.7-code once per catalog that serves it, and
        // reseller catalogs publish their own promotional rates. Selecting one of
        // those would undercharge every Kimi report.
        let pricing = PricingMap::load_embedded();
        let kimi_k27_code = pricing.find("kimi-k2.7-code").unwrap();

        // MoonshotAI's list rate. Reseller catalogs publish 0.73 and 0.75.
        assert!((kimi_k27_code.input * 1e6 - 0.95).abs() < 1e-9);
        assert!((kimi_k27_code.output * 1e6 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn embedded_pricing_prefers_an_exact_only_tier_over_a_fuzzy_litellm_match() {
        // The snapshot lives in its own map, consulted only after the primary
        // LiteLLM one misses, so marking `claude-opus-5-fast` exact-only there is
        // not enough on its own: the primary fuzzy scan would answer with the
        // base `claude-opus-5` entry it does carry and bill the tier at list
        // price. Exercised through `load_embedded` to cover that lookup order.
        let pricing = PricingMap::load_embedded();

        let base = pricing.find("claude-opus-5").unwrap();
        assert!((base.input * 1e6 - 5.0).abs() < 1e-9);

        let fast = pricing
            .find("claude-opus-5-fast")
            .expect("the embedded snapshot prices the Fast tier");
        assert!((fast.input * 1e6 - 12.0).abs() < 1e-9);
        assert!((fast.output * 1e6 - 60.0).abs() < 1e-9);
        assert_eq!(pricing.context_limit("claude-opus-5-fast"), Some(1_000_000));

        // A regional alias shadows the same base entry, and is exact-only for
        // the same reason: its premium is not the list rate.
        let eu = pricing
            .find("claude-opus-5@eu")
            .expect("the embedded snapshot prices the EU alias");
        assert!(eu.input > base.input);

        // Gating keys the snapshot carries must not cost keys nothing prices
        // exactly their fuzzy match.
        assert!(pricing.find("claude-opus-5-20260115").is_some());
    }

    #[test]
    fn a_separator_spelling_of_an_exact_only_id_is_priced_as_that_id() {
        // `pricing_key_matches` reads `@`, `.` and `-` as one separator, so
        // `claude-opus-5-eu` fuzzy-matches LiteLLM's base `claude-opus-5` exactly
        // as `claude-opus-5@eu` would. Gating only the spelling the catalog wrote
        // would bill the regional premium at list price, and gating the spelling
        // without answering from the id it names would lose the rate entirely.
        let pricing = PricingMap::load_embedded();

        let base = pricing.find("claude-opus-5").unwrap();
        let eu = pricing.find("claude-opus-5@eu").unwrap();
        let dashed = pricing
            .find("claude-opus-5-eu")
            .expect("the dashed spelling names the EU alias");
        assert!(dashed.input > base.input);
        assert!((dashed.input - eu.input).abs() < 1e-12);
        assert!((dashed.output - eu.output).abs() < 1e-12);
        assert_eq!(
            pricing.context_limit("claude-opus-5-eu"),
            pricing.context_limit("claude-opus-5@eu")
        );

        // Only a spelling of the whole id is that id: a longer name that merely
        // contains it keeps the fuzzy match it has always had.
        assert!(pricing.find("claude-opus-5-eu-preview").is_some());
    }

    #[test]
    fn a_dotted_spelling_of_a_dash_spelled_exact_only_id_is_priced_as_that_id() {
        // The catalog writes most exact-only ids with dashes alone, and those
        // have to be indexed under their normalized spelling too: a request for
        // `claude-opus-5.fast` normalizes to the id the snapshot carries, so
        // leaving it out of the index lets the primary fuzzy scan answer with
        // the base `claude-opus-5` entry and bill the tier at list price.
        let pricing = PricingMap::load_embedded();

        let base = pricing.find("claude-opus-5").unwrap();
        let dotted = pricing
            .find("claude-opus-5.fast")
            .expect("the dotted spelling names the Fast tier");
        assert!((dotted.input * 1e6 - 12.0).abs() < 1e-9);
        assert!((dotted.output * 1e6 - 60.0).abs() < 1e-9);
        assert!(dotted.input > base.input);
        assert_eq!(pricing.context_limit("claude-opus-5.fast"), Some(1_000_000));
    }

    #[test]
    fn exact_fallback_lookup_resolves_separator_spellings_of_exact_only_ids() {
        let pricing = PricingMap::load_embedded();
        let expected = pricing
            .find_exact_with_fallback("claude-opus-5@eu")
            .expect("the embedded snapshot should contain the exact-only regional id");

        for spelling in ["claude-opus-5-eu", "claude-opus-5.eu"] {
            let resolved = pricing
                .find_exact_with_fallback(spelling)
                .expect("separator-equivalent exact-only ids should resolve");
            assert_eq!(resolved.input, expected.input, "{spelling}");
            assert_eq!(resolved.output, expected.output, "{spelling}");
        }
        assert!(
            pricing
                .find_exact_with_fallback("claude-opus-5-eu-preview")
                .is_none()
        );
    }

    #[test]
    fn exact_fallback_lookup_prefers_primary_normalized_entries() {
        let mut pricing = PricingMap::load_embedded();
        pricing.load_json(
            r#"{
                "claude-opus-5.eu": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );

        let resolved = pricing
            .find_exact_with_fallback("claude-opus-5-eu")
            .expect("the normalized primary entry should resolve");
        assert_eq!(resolved.input, 0.000009);
        assert_eq!(resolved.output, 0.000010);
    }

    #[test]
    fn configured_alias_of_an_exact_only_tier_beats_a_fuzzy_match_on_the_alias() {
        // The alias target is only tried after this map has answered for the
        // recorded spelling, and that spelling can fuzzy-match a different model
        // on its own - here LiteLLM's base `claude-opus-5` - which would bill the
        // tier at list price and never reach what the alias names.
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "claude-opus-5-turbo",
            "claude-opus-5-fast",
        )]);
        let pricing = PricingMap::load_embedded();

        let turbo = pricing
            .find("claude-opus-5-turbo")
            .expect("the alias resolves to the Fast tier");
        assert!((turbo.input * 1e6 - 12.0).abs() < 1e-9);
        assert!((turbo.output * 1e6 - 60.0).abs() < 1e-9);
        assert_eq!(
            pricing.context_limit("claude-opus-5-turbo"),
            Some(1_000_000)
        );
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
    fn glm_cache_patch_keeps_an_explicitly_published_cache_write_rate() {
        // A published cache-write rate survives the z.ai patch even when it
        // happens to equal the derived `input * 1.25` default: explicitness is
        // tracked, not inferred from the value.
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "zai/glm-test-model": {
                    "input_cost_per_token": 0.2,
                    "output_cost_per_token": 1.1,
                    "cache_read_input_token_cost": 0.03,
                    "cache_creation_input_token_cost": 0.25
                }
            }"#,
        );
        let zeroed = Pricing {
            cache_create: 0.0,
            cache_read: 0.03,
            cache_read_explicit: true,
            cache_create_explicit: true,
            ..Pricing::empty()
        };

        pricing.put_builtin_glm("zai/glm-test-model", zeroed);

        let entry = pricing.find_exact("zai/glm-test-model").unwrap();
        assert_eq!(entry.cache_create, 0.25);
        assert_eq!(entry.cache_read, 0.03);
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
            .find_entry("gpt-retry", Fuzzy::Allowed)
            .expect("successful retry should load pricing");
        assert_eq!(gpt_retry.input, 0.000001);
        assert_eq!(gpt_retry.output, 0.000002);
        assert_eq!(
            pricing.context_limit_entry("gpt-retry", Fuzzy::Allowed),
            Some(42)
        );
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
    fn live_models_dev_pricing_prefers_the_authoring_catalog_over_resellers() {
        // The same model id appears in every catalog that serves it, and the
        // first one loaded keeps it. Reseller rates are their own, so loading a
        // reseller before the author would bill at a promotional price.
        // `302ai` sorts before `moonshotai`, so an id-only ordering would pick a
        // reseller here.
        let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.6, "output": 3.0 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.73, "output": 3.5 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.95, "output": 4.0 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        // One entry loaded, not three: the reseller duplicates are skipped
        // because the authoring catalog claimed the id first.
        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let kimi = pricing.find_exact("kimi-k2.7-code").unwrap();
        assert!((kimi.input * 1e6 - 0.95).abs() < 1e-9);
        assert!((kimi.output * 1e6 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_skips_models_priced_per_asset() {
        // The embedded snapshot excludes these, so the online refresh has to as
        // well or `--offline` and the default path disagree. The authored
        // catalog's verdict is carried in, so a reseller calling an image model
        // text-only cannot reintroduce it.
        let json = r#"{
                "302ai": {
                    "models": {
                        "gemini-2.5-flash-image": {
                            "modalities": { "input": ["text", "image"], "output": ["text"] },
                            "cost": { "input": 0.3, "output": 30 }
                        }
                    }
                },
                "scaleway": {
                    "models": {
                        "some-unlisted-transcriber": {
                            "modalities": { "input": ["audio"], "output": ["text"] },
                            "cost": { "input": 0.003, "output": 0 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k3": {
                            "modalities": { "input": ["text", "image", "video"], "output": ["text"] },
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        // Only kimi-k3 loads: audio-only input is rejected from the payload
        // itself, and the image model is rejected by the authored catalog.
        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("gemini-2.5-flash-image").is_none());
        assert!(pricing.find_exact("some-unlisted-transcriber").is_none());
        assert!(pricing.find_exact("kimi-k3").is_some());
    }

    #[test]
    fn live_models_dev_pricing_keys_asset_pricing_on_the_source_model_id() {
        // `assetPricedModelIds` records the authored source ids generation
        // matches on, so a catalog serving one under a different `id` must be
        // rejected on its key, not on the pricing key that key resolves to.
        let json = r#"{
                "google": {
                    "models": {
                        "gemini-2.5-flash-image": {
                            "id": "models/gemini-2.5-flash-image",
                            "modalities": { "input": ["text"], "output": ["text"] },
                            "cost": { "input": 0.3, "output": 30 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(0));
        assert!(
            pricing
                .find_exact("models/gemini-2.5-flash-image")
                .is_none()
        );
    }

    #[test]
    fn live_models_dev_pricing_rejects_explicitly_empty_modalities() {
        // `isTokenPricedModel` defaults only a *missing* list to `['text']`, so an
        // empty one has to fail here too or the snapshot and the online refresh
        // disagree about the same payload.
        let json = r#"{
                "moonshotai": {
                    "models": {
                        "empty-output-model": {
                            "modalities": { "input": ["text"], "output": [] },
                            "cost": { "input": 1, "output": 2 }
                        },
                        "empty-input-model": {
                            "modalities": { "input": [], "output": ["text"] },
                            "cost": { "input": 1, "output": 2 }
                        },
                        "no-modalities-model": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("empty-output-model").is_none());
        assert!(pricing.find_exact("empty-input-model").is_none());
        assert!(pricing.find_exact("no-modalities-model").is_some());
    }

    #[test]
    fn live_models_dev_pricing_carries_the_reseller_ids_generation_carries() {
        // Generation prunes no ids any more: every id a catalog publishes is
        // embedded, and the fuzzy lookup is gated instead. Pruning here would
        // price a smaller set of ids online than `--offline` carries, and leave
        // the two maps offering different candidates to the same fuzzy lookup.
        let json = r#"{
                "fireworks-ai": {
                    "models": {
                        "accounts/fireworks/models/kimi-k2p6": {
                            "cost": { "input": 0.6, "output": 2.5 }
                        }
                    }
                },
                "venice": {
                    "models": {
                        "claude-opus-5-fast": {
                            "cost": { "input": 12, "output": 60 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "claude-3-haiku-20240307": {
                            "cost": { "input": 0.25, "output": 1.25 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(3));
        assert!(
            pricing
                .find_exact("accounts/fireworks/models/kimi-k2p6")
                .is_some()
        );
        assert!(pricing.find_exact("claude-opus-5-fast").is_some());
        assert!(pricing.find_exact("claude-3-haiku-20240307").is_some());
        // Carried, but a tier is the right rate only for a request naming it,
        // even one the author prices itself.
        assert!(pricing.exact_only.contains("claude-opus-5-fast"));
        assert!(!pricing.exact_only.contains("claude-3-haiku-20240307"));
    }

    #[test]
    fn live_models_dev_pricing_prefers_the_more_detailed_entry_within_a_tier() {
        // Generation breaks same-tier ties by how much pricing detail an entry
        // carries, so the online path has to as well or the two disagree.
        // `kimi-k2.6-nitro` is a separately priced tier of a carried model, so it
        // is one of the ids resellers alone may supply. The ordering tests below
        // use it for that reason.
        let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1 },
                            "limit": { "context": 128000 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        // One model, counted once even though the second entry replaced the first.
        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("kimi-k2.6-nitro").unwrap();
        assert!(entry.cache_read_explicit);
        assert!((entry.cache_read * 1e6 - 0.1).abs() < 1e-9);
        assert_eq!(
            pricing.context_limits.get("kimi-k2.6-nitro"),
            Some(&128_000)
        );
    }

    #[test]
    fn live_models_dev_pricing_keeps_authored_token_models_a_catalog_mislabels() {
        // A catalog serving claude-opus-5 as image-output would drop a model the
        // snapshot carries. The authored classification settles it both ways, so
        // only models the authored catalog never listed fall back to the live
        // modalities.
        let json = r#"{
                "302ai": {
                    "models": {
                        "claude-opus-5": {
                            "modalities": { "input": ["text"], "output": ["text", "image"] },
                            "cost": { "input": 5, "output": 25 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("claude-opus-5").is_some());
    }

    #[test]
    fn live_models_dev_pricing_ranks_by_the_catalog_s_own_provider_id() {
        // The generator reads `provider.id ?? providerId`, so a catalog filed
        // under a different key than its id must still rank as its id - here the
        // authoring catalog, which has to win over the reseller.
        let json = r#"{
                "aaa-filed-under-another-key": {
                    "id": "moonshotai",
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 3, "output": 4 }
                        }
                    }
                },
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("kimi-k2.6-nitro").unwrap();
        assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_orders_detail_the_way_generation_does() {
        // Generation compares cache-read, then cache-write, then context limit, so
        // a cache-read-only entry outranks one carrying the other two. Counting
        // fields instead would pick the other catalog online.
        let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_write": 1.25 },
                            "limit": { "context": 128000 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("kimi-k2.6-nitro").unwrap();
        assert!(entry.cache_read_explicit);
        assert!((entry.cache_read * 1e6 - 0.1).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_drops_the_context_limit_of_a_replaced_catalog() {
        // The winning catalog publishes no limit, so keeping the loser's would
        // report a context window the selected rates do not belong to.
        let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 },
                            "limit": { "context": 128000 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 3, "output": 4 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("kimi-k2.6-nitro").unwrap();
        assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
        assert_eq!(pricing.context_limits.get("kimi-k2.6-nitro"), None);
    }

    #[test]
    fn live_models_dev_pricing_resolves_duplicate_ids_inside_one_catalog_stably() {
        // Two source keys can carry the same `id`. Generation walks a catalog in
        // key order and keeps the first of an equal-strength tie, so the lower key
        // has to win here too rather than whichever the hash map yields first.
        let json = r#"{
                "moonshotai": {
                    "models": {
                        "b-alias": {
                            "id": "shared-model",
                            "cost": { "input": 9, "output": 9 }
                        },
                        "a-alias": {
                            "id": "shared-model",
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("shared-model").unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_merges_spelling_duplicates_onto_one_entry() {
        // The dotted and dashed spellings name one model. Kept apart, the fuzzy
        // lookup ties between them by length and can return whichever, so the
        // flat reseller spelling must lose its slot to the tiered owner one
        // entirely - entry included, not just the claim.
        let json = r#"{
                "llmgateway": {
                    "models": {
                        "grok-9-5": {
                            "cost": { "input": 2, "output": 6 }
                        }
                    }
                },
                "xai": {
                    "models": {
                        "grok-9.5": {
                            "cost": {
                                "input": 2, "output": 6, "cache_read": 0.3,
                                "tiers": [{
                                    "input": 4, "output": 12, "cache_read": 0.6,
                                    "tier": { "type": "context", "size": 200000 }
                                }]
                            }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("grok-9-5").is_none());
        let entry = pricing.find_exact("grok-9.5").unwrap();
        assert_eq!(entry.long_context_threshold, Some(200_000));
        // The dashed spelling still resolves - through the fuzzy lookup - and
        // lands on the tiered entry rather than a flat duplicate.
        let via_dash = pricing.find("grok-9-5").unwrap();
        assert_eq!(via_dash.long_context_threshold, Some(200_000));
    }

    #[test]
    fn live_models_dev_pricing_prefers_a_tiered_catalog_within_a_trust_tier() {
        // Within one trust tier, the catalog publishing a long-context band
        // outranks one with more cache detail: the band is the rate data
        // hardest to come by.
        let json = r#"{
                "llmgateway": {
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1, "cache_write": 1.25 },
                            "limit": { "context": 500000 }
                        }
                    }
                },
                "venice": {
                    "models": {
                        "some-reseller-only-model": {
                            "cost": {
                                "input": 1, "output": 2,
                                "tiers": [{
                                    "input": 2, "output": 4,
                                    "tier": { "type": "context", "size": 200000 }
                                }]
                            }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("some-reseller-only-model").unwrap();
        assert_eq!(entry.long_context_threshold, Some(200_000));
    }

    #[test]
    fn live_models_dev_pricing_stores_a_model_with_an_empty_declared_id_by_its_key() {
        // A live catalog can declare `id` as an empty string. The generator's
        // `selectModelsDevPricingKey` falls back to the source key; keeping ""
        // would store the model under a name no lookup ever asks for.
        let json = r#"{
                "moonshotai": {
                    "models": {
                        "kimi-k9": {
                            "id": "",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("kimi-k9").is_some());
        assert!(pricing.find_exact("").is_none());
    }

    #[test]
    fn live_models_dev_pricing_orders_same_id_catalogs_by_their_map_key() {
        // Two catalogs can declare the same provider id. The declared id ties,
        // so the unique map key settles the order - the same order generation's
        // provider-key walk uses - instead of hash iteration.
        let json = r#"{
                "zzz-alias": {
                    "id": "some-gateway",
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                },
                "aaa-alias": {
                    "id": "some-gateway",
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("some-reseller-only-model").unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_breaks_exact_ties_by_source_key_across_same_id_catalogs() {
        // Two catalogs declaring the same provider id are one provider to
        // generation, which breaks exact-strength ties by the source model key.
        // The smaller source key lives in the catalog with the LARGER map key
        // here, so keeping whichever arrived first would pick the other rate.
        let json = r#"{
                "aaa-key": {
                    "id": "some-gateway",
                    "models": {
                        "zz-spelling": {
                            "id": "some-reseller-only-model",
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                },
                "zzz-key": {
                    "id": "some-gateway",
                    "models": {
                        "aa-spelling": {
                            "id": "some-reseller-only-model",
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing.find_exact("some-reseller-only-model").unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_skips_flat_fee_catalogs() {
        // `kimi-for-coding` is a subscription plan, so it publishes zero token
        // costs. Loading it would make the model free for everyone. The rule is
        // about the rates rather than about who publishes them, so an authoring
        // catalog listing a zero-cost entry has to be skipped as well.
        let json = r#"{
                "kimi-for-coding": {
                    "models": {
                        "kimi-for-coding": {
                            "cost": { "input": 0, "output": 0 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k3": {
                            "cost": { "input": 0, "output": 0 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(0));
        assert!(pricing.find_exact("kimi-for-coding").is_none());
        assert!(pricing.find_exact("kimi-k3").is_none());
    }

    #[test]
    fn live_models_dev_pricing_keeps_a_tier_out_of_the_fuzzy_lookup() {
        // `exactOnly` is a field the generator writes, and a live catalog has no
        // such field, so the verdict has to be rederived here. Otherwise the
        // premium tier stays a fuzzy candidate and wins the base model's lookup,
        // which prefers the longest matching key.
        let json = r#"{
                "moonshotai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 1, "output": 2 }
                        },
                        "kimi-k2.7-code-highspeed": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(2));
        let base = pricing.find("kimi-k2-7-code").unwrap();
        assert!((base.input * 1e6 - 1.0).abs() < 1e-9);
        let tier = pricing.find("kimi-k2.7-code-highspeed").unwrap();
        assert!((tier.input * 1e6 - 9.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_marks_a_regional_alias_of_a_model_exact_only() {
        // Vertex spells its regional entries with `@`, which the tier check has
        // to fold the way `normalizeModelId` does or `claude-opus-5@eu` stays a
        // fuzzy candidate online while the snapshot marks it exact-only.
        let json = r#"{
                "google-vertex-anthropic": {
                    "models": {
                        "claude-opus-5@eu": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.exact_only.contains("claude-opus-5@eu"));
    }

    #[test]
    fn live_models_dev_pricing_keeps_an_unversioned_id_out_of_the_fuzzy_lookup() {
        // `auto` names no particular model, so as a fuzzy candidate it answered
        // `codex-auto-review`, a label the Codex adapter resolves by date.
        let json = r#"{
                "moonshotai": {
                    "models": {
                        "auto": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
        let mut pricing = PricingMap::default();

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(pricing.find_exact("auto").is_some());
        assert!(pricing.find("codex-auto-review").is_none());
        assert!(pricing.context_limit("codex-auto-review").is_none());
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
            map.load_models_dev_json_missing(build_time_models_dev_json())
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
            .find(|model| offline.find_entry(model, Fuzzy::Allowed).is_none())
        else {
            return;
        };
        // The primary table alone misses it, but the offline embedded fallback
        // resolves it; a bare map without the fallback flag must not.
        assert!(offline.find_entry(model, Fuzzy::Allowed).is_none());
        assert!(offline.find(model).is_some());
        assert!(PricingMap::default().find(model).is_none());
    }

    #[test]
    fn offline_prices_new_anthropic_model_from_embedded_models_dev() {
        use ccusage_cli::PricingOverride;
        assert!(
            embedded_models_dev_pricing()
                .find_entry("claude-fable-5-1", Fuzzy::Allowed)
                .is_some(),
            "embedded models.dev snapshot should include claude-fable-5-1"
        );
        let offline = PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &PricingOverride)>(),
        );
        assert!(offline.find("claude-fable-5-1").is_some());
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

        // The updater owns the numeric rates, so this regression fence checks
        // the billing structure that ccusage requires without freezing prices.
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let entry = pricing.find(model).unwrap();
            assert!(entry.input > 0.0, "{model}");
            assert!(entry.output > 0.0, "{model}");
            assert!(entry.cache_create > 0.0, "{model}");
            assert!(entry.cache_read > 0.0, "{model}");
            assert!(entry.cache_create_explicit, "{model}");
            assert!(entry.cache_read_explicit, "{model}");
            assert!(
                entry
                    .input_above_200k
                    .is_some_and(|rate| rate >= entry.input),
                "{model}"
            );
            assert!(
                entry
                    .output_above_200k
                    .is_some_and(|rate| rate >= entry.output),
                "{model}"
            );
            assert!(
                entry
                    .cache_create_above_200k
                    .is_some_and(|rate| rate >= entry.cache_create),
                "{model}"
            );
            assert!(
                entry
                    .cache_read_above_200k
                    .is_some_and(|rate| rate >= entry.cache_read),
                "{model}"
            );
            let threshold = entry.long_context_threshold.expect(model);
            assert!(
                pricing
                    .context_limit(model)
                    .is_some_and(|limit| limit > threshold),
                "{model}"
            );
        }
    }

    #[test]
    fn gpt_5_6_alias_resolves_to_sol_when_the_generic_entry_is_missing() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004,
                    "output_cost_per_token": 0.00002,
                    "cache_creation_input_token_cost": 0.000005,
                    "cache_read_input_token_cost": 0.0000004,
                    "input_cost_per_token_above_200k_tokens": 0.000008,
                    "output_cost_per_token_above_200k_tokens": 0.00003,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.00001,
                    "cache_read_input_token_cost_above_200k_tokens": 0.0000008,
                    "max_input_tokens": 1050000
                }
            }"#,
        );
        let alias = pricing.find("gpt-5.6").unwrap();
        let sol = pricing.find("gpt-5.6-sol").unwrap();

        assert_eq!(alias.input, sol.input);
        assert_eq!(alias.output, sol.output);
        assert_eq!(alias.cache_create, sol.cache_create);
        assert_eq!(alias.cache_read, sol.cache_read);
        assert_eq!(alias.input_above_200k, sol.input_above_200k);
        assert_eq!(alias.output_above_200k, sol.output_above_200k);
        assert_eq!(
            pricing.context_limit("gpt-5.6"),
            pricing.context_limit("gpt-5.6-sol")
        );
        assert_eq!(
            long_context_split_threshold("gpt-5.6"),
            long_context_split_threshold("gpt-5.6-sol")
        );
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
        pricing.fill_long_context_rates_from_models_dev();

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
        pricing.fill_long_context_rates_from_models_dev();

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(gpt_55.input_above_200k, Some(12e-6));
        assert_eq!(gpt_55.long_context_threshold, None);
    }

    #[test]
    fn embedded_models_dev_carries_grok_long_context_tiers() {
        // xAI bills grok-4.5 and grok-4.6 at double rates above 200K input
        // tokens. LiteLLM's embed does not carry xai keys at all, so these come
        // from the models.dev snapshot's `cost.tiers` - without them every
        // long-context Grok request is billed at the base rate (#1541).
        let pricing = PricingMap::load_embedded();

        for model in ["grok-4.5", "grok-4.6"] {
            let entry = pricing.find(model).unwrap();
            assert!((entry.input * 1e6 - 2.0).abs() < 1e-9, "{model} base input");
            assert!(
                (entry.input_above_200k.unwrap() * 1e6 - 4.0).abs() < 1e-9,
                "{model} long-context input"
            );
            assert!(
                (entry.output_above_200k.unwrap() * 1e6 - 12.0).abs() < 1e-9,
                "{model} long-context output"
            );
            assert_eq!(entry.long_context_threshold, Some(200_000), "{model}");
        }
        assert_eq!(long_context_split_threshold("grok-4.5"), 200_000);
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

        let original = pricing
            .find_entry("claude-opus-4-8", Fuzzy::Allowed)
            .unwrap();
        let resolved = pricing.find("claude-opus-4-8").unwrap();

        assert_eq!(resolved.input, original.input);
        assert_eq!(
            pricing.context_limit("claude-opus-4-8"),
            pricing.context_limit_entry("claude-opus-4-8", Fuzzy::Allowed)
        );
    }

    #[test]
    fn embedded_pricing_includes_codex_priority_multiplier() {
        let pricing = PricingMap::load_embedded();

        assert_eq!(pricing.find("gpt-5.6-sol").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.6-terra").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.6-luna").unwrap().fast_multiplier, 2.0);
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
        let opus_47 = pricing.find("claude-opus-4-7").unwrap();

        assert_eq!(
            pricing.find("claude-opus-4.7-20260416").unwrap().input,
            opus_47.input
        );
        assert_eq!(
            pricing.context_limit("claude-opus-4.7"),
            pricing.context_limit("claude-opus-4-7")
        );
    }

    #[test]
    fn embedded_pricing_resolves_opus_48_dot_model_names() {
        let pricing = PricingMap::load_embedded();
        let canonical = pricing.find("claude-opus-4-8").unwrap();

        let opus_48 = pricing.find("claude-opus-4.8-20260528").unwrap();
        assert_eq!(opus_48.input, canonical.input);
        assert_eq!(opus_48.output, canonical.output);
        assert_eq!(opus_48.cache_create, canonical.cache_create);
        assert_eq!(opus_48.cache_read, canonical.cache_read);
        assert_eq!(
            pricing.context_limit("claude-opus-4.8"),
            pricing.context_limit("claude-opus-4-8")
        );
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
                cache_create_explicit: true,
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
                cache_create_explicit: true,
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
                cache_create_explicit: true,
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
        let json = build_time_pricing_json();
        assert!(json.len() < 200_000);
        assert!(!json.contains("\"source\""));
        assert!(!json.contains("vertex_ai/"));
        assert!(json.contains("claude-opus-4-6"));
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
                cache_create_explicit: true,
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
                cache_create_explicit: true,
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
        fn exact_override_wins_over_gpt_5_6_alias() {
            let mut pricing = PricingMap::load_embedded();
            let sol = pricing.find("gpt-5.6-sol").unwrap();
            let overrides = build_overrides("gpt-5.6", |o| {
                o.input_cost_per_token = Some(42e-6);
                o.max_input_tokens = Some(654_321);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("gpt-5.6").unwrap();
            assert_eq!(entry.input, 42e-6);
            assert_eq!(entry.output, sol.output);
            assert_eq!(entry.cache_create, sol.cache_create);
            assert_eq!(entry.cache_read, sol.cache_read);
            assert_eq!(entry.input_above_200k, sol.input_above_200k);
            assert_eq!(entry.output_above_200k, sol.output_above_200k);
            assert_eq!(entry.cache_create_above_200k, sol.cache_create_above_200k);
            assert_eq!(entry.cache_read_above_200k, sol.cache_read_above_200k);
            assert_eq!(entry.long_context_threshold, sol.long_context_threshold);
            assert_eq!(entry.fast_multiplier, sol.fast_multiplier);
            assert_eq!(pricing.context_limit("gpt-5.6"), Some(654_321));
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
                    cache_create_explicit: true,
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
                    cache_create_explicit: false,
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
                    cache_create_explicit: false,
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
                    cache_create_explicit: false,
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
