use std::{
    borrow::Cow,
    sync::{Arc, Mutex, OnceLock, RwLock},
    thread::{self, JoinHandle},
};

use ccusage_cli::PricingOverride;

use serde::Deserialize;
use serde_json::Value;

use crate::{
    MILLIS_PER_DAY, MILLIS_PER_HOUR, TimestampMs,
    fast::{FxHashMap, FxHashSet},
};

const FAST_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("fast-multiplier-overrides.json");
const FLEX_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("flex-multiplier-overrides.json");
const ULTRAFAST_MULTIPLIER_OVERRIDES_JSON: &str =
    include_str!("ultrafast-multiplier-overrides.json");

/// The pricing snapshots the binary embeds, minified and deflated - the
/// models.dev one alone would otherwise add a quarter megabyte of JSON - and
/// inflated once, on first use, by the accessors below.
///
/// They are compiled by `ccusage-pricing-snapshots`, which only the binary
/// depends on, and handed over through [`set_embedded_snapshots`]. Embedding
/// them here instead would make every routine snapshot refresh recompile this
/// crate and every adapter built on it.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedSnapshots {
    pub litellm_pricing: &'static [u8],
    /// The committed models.dev pricing snapshot.
    pub models_dev_pricing: &'static [u8],
    /// The committed models.dev catalog rules.
    pub models_dev_catalog_rules: &'static [u8],
}

static EMBEDDED_SNAPSHOTS: OnceLock<EmbeddedSnapshots> = OnceLock::new();

/// Installs the embedded pricing snapshots.
///
/// The binary calls this at startup, before anything loads pricing; later calls
/// are ignored. Tests install them through
/// `ccusage_test_support::install_pricing_snapshots`.
///
/// # Example
///
/// ```ignore
/// ccusage_core::pricing::set_embedded_snapshots(ccusage_core::pricing::EmbeddedSnapshots {
///     litellm_pricing: ccusage_pricing_snapshots::LITELLM_PRICING,
///     models_dev_pricing: ccusage_pricing_snapshots::MODELS_DEV_PRICING,
///     models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
/// });
/// ```
pub fn set_embedded_snapshots(snapshots: EmbeddedSnapshots) {
    let _ = EMBEDDED_SNAPSHOTS.set(snapshots);
}

fn embedded_snapshots() -> &'static EmbeddedSnapshots {
    EMBEDDED_SNAPSHOTS
        .get()
        .expect("embedded pricing snapshots must be installed with set_embedded_snapshots")
}

/// Inflate one of the embedded snapshots. Infallible by construction: the
/// snapshot crate's build script produced the bytes from JSON it had just
/// serialized.
fn inflate_snapshot(cell: &'static OnceLock<String>, deflated: &[u8]) -> &'static str {
    cell.get_or_init(|| {
        let bytes = miniz_oxide::inflate::decompress_to_vec(deflated)
            .expect("inflate embedded pricing snapshot");
        String::from_utf8(bytes).expect("embedded pricing snapshot is UTF-8")
    })
}

fn build_time_pricing_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().litellm_pricing)
}

fn build_time_models_dev_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().models_dev_pricing)
}

fn models_dev_catalog_rules_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().models_dev_catalog_rules)
}
const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
// Anthropic date-suffixed model aliases use YYYYMMDD, while other numeric
// suffixes are treated as distinct model versions.
const MODEL_DATE_SUFFIX_DIGITS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub(crate) cache_create: Option<f64>,
    pub cache_read: Option<f64>,
    pub(crate) cache_create_1h: Option<f64>,
    pub(crate) cache_create_1h_above_200k: Option<f64>,
    pub input_above_200k: Option<f64>,
    pub output_above_200k: Option<f64>,
    pub cache_create_above_200k: Option<f64>,
    pub cache_read_above_200k: Option<f64>,
    // Token count above which the `*_above_200k` rates apply. The field names
    // keep the LiteLLM `_above_200k_tokens` suffix for JSON compatibility, but
    // some providers switch tiers at a different point (OpenAI long-context
    // pricing starts above 272K input tokens), so the threshold is per model.
    pub(crate) long_context_threshold: Option<u64>,
    pub fast_multiplier: f64,
    pub ultrafast_multiplier: f64,
    pub flex_multiplier: f64,
}

/// Default tier boundary for LiteLLM `*_above_200k_tokens` pricing fields.
pub(crate) const DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 200_000;

impl Pricing {
    pub fn cache_creation_rate(self) -> f64 {
        self.cache_create.unwrap_or(self.input * 1.25)
    }

    const fn empty() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_create: None,
            cache_read: None,
            cache_create_1h: None,
            cache_create_1h_above_200k: None,
            input_above_200k: None,
            output_above_200k: None,
            cache_create_above_200k: None,
            cache_read_above_200k: None,
            long_context_threshold: None,
            fast_multiplier: 1.0,
            ultrafast_multiplier: 1.0,
            flex_multiplier: 1.0,
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
    pricing.cache_create = Some(rates.cache_create);
    pricing.cache_read = Some(rates.cache_read);
    pricing.input_above_200k = Some(rates.input);
    pricing.output_above_200k = Some(rates.output);
    pricing.cache_create_above_200k = Some(rates.cache_create);
    pricing.cache_read_above_200k = Some(rates.cache_read);

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
        pricing.cache_create = Some(value);
    }
    if let Some(value) = override_value.cache_creation_input_token_cost_above_1hr {
        pricing.cache_create_1h = Some(value);
    }
    if let Some(value) = override_value.cache_read_input_token_cost {
        pricing.cache_read = Some(value);
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
    if let Some(value) = override_value.flex_multiplier {
        pricing.flex_multiplier = value;
    }
}

/// Whether a lookup may fall back to the fuzzy scan, or has to answer from
/// exact entries because a fuzzy match would shadow an exact-only id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fuzzy {
    Allowed,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Primary,
    ModelsDev,
}

struct Table<'a, T> {
    rows: &'a FxHashMap<String, T>,
    models_dev: &'a FxHashSet<String>,
}

impl<T> Clone for Table<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Table<'_, T> {}

impl<'a, T> Table<'a, T> {
    fn layer_of(self, key: &str) -> Layer {
        if self.models_dev.contains(key) {
            Layer::ModelsDev
        } else {
            Layer::Primary
        }
    }

    fn key_in(self, model: &str, layer: Layer) -> Option<&'a str> {
        self.rows
            .get_key_value(model)
            .map(|(key, _)| key.as_str())
            .filter(|key| self.layer_of(key) == layer)
    }
}

#[derive(Clone, Copy)]
enum Lookup {
    Pricing,
    Context,
    Exact,
}

/// One slot per looked-up model, so concurrent first lookups of a model share
/// a single fuzzy search instead of each running it.
type FindCache = RwLock<FxHashMap<String, Arc<OnceLock<Option<Pricing>>>>>;

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
    context_defaults: FxHashSet<String>,
    models_dev_keys: FxHashSet<String>,
    models_dev_limits: FxHashSet<String>,
    live_merged: OnceLock<Box<PricingMap>>,
    enable_models_dev_fallback: bool,
    find_cache: OnceLock<FindCache>,
    /// Every entry key under its normalized spelling, mapped to the one key
    /// carrying that spelling, or `None` when several keys share it.
    ///
    /// Built on the first normalized lookup so [`Self::find_exact_normalized`]
    /// does not rescan the whole table per call; parsers that price every
    /// event through it would otherwise spend most of a report there. Keys
    /// rather than rates are stored, so only a change to the key set has to
    /// drop it.
    normalized_keys: OnceLock<FxHashMap<String, Option<String>>>,
}

/// The ids of [`PricingMap::exact_only`], indexed both as written and under the
/// separator-normalized spelling `pricing_key_matches` compares.
///
/// The gate that keeps a request naming an exact-only id off the fuzzy scan has
/// to recognize the same spellings that scan does, or `claude-opus-5@eu` written
/// as `claude-opus-5-eu` passes the gate and is billed at the base model's rate,
/// which is exactly what marking the id exact-only was meant to prevent. That
/// spelling names the tier, so it resolves to it rather than losing its rate.
#[derive(Clone, Debug, Default)]
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
            context_defaults: FxHashSet::default(),
            models_dev_keys: FxHashSet::default(),
            models_dev_limits: FxHashSet::default(),
            live_merged: OnceLock::new(),
            enable_models_dev_fallback: false,
            find_cache: OnceLock::new(),
            normalized_keys: OnceLock::new(),
        }
    }
}

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
    provider_specific_entry: Option<ProviderSpecificEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderSpecificEntry {
    fast: Option<f64>,
    flex: Option<f64>,
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
        .any(|byte| is_pricing_key_separator(byte) || byte.is_ascii_uppercase())
    {
        Cow::Owned(dash_pricing_key_separators(model_id.to_lowercase().chars()))
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

struct SpeedMultipliers {
    fast: SpeedMultiplierOverrides,
    ultrafast: SpeedMultiplierOverrides,
    flex: SpeedMultiplierOverrides,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PublishedSpeed {
    fast: Option<f64>,
    ultrafast: Option<f64>,
    flex: Option<f64>,
}

impl SpeedMultipliers {
    fn get() -> &'static Self {
        static TABLES: OnceLock<SpeedMultipliers> = OnceLock::new();
        TABLES.get_or_init(|| Self {
            fast: SpeedMultiplierOverrides::load(FAST_MULTIPLIER_OVERRIDES_JSON),
            ultrafast: SpeedMultiplierOverrides::load(ULTRAFAST_MULTIPLIER_OVERRIDES_JSON),
            flex: SpeedMultiplierOverrides::load(FLEX_MULTIPLIER_OVERRIDES_JSON),
        })
    }

    fn for_model(&self, model: &str) -> PublishedSpeed {
        PublishedSpeed {
            fast: self.fast.multiplier_for(model),
            ultrafast: self.ultrafast.multiplier_for(model),
            flex: self.flex.multiplier_for(model),
        }
    }
}

impl PublishedSpeed {
    fn or(self, other: Self) -> Self {
        Self {
            fast: self.fast.or(other.fast),
            ultrafast: self.ultrafast.or(other.ultrafast),
            flex: self.flex.or(other.flex),
        }
    }

    fn applied_to(self, pricing: Pricing) -> Pricing {
        Pricing {
            fast_multiplier: self.fast.unwrap_or(1.0),
            ultrafast_multiplier: self.ultrafast.unwrap_or(1.0),
            flex_multiplier: self.flex.unwrap_or(1.0),
            ..pricing
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct SpeedMultiplierOverrides {
    exact: FxHashMap<String, f64>,
    normalized_prefix: FxHashMap<String, f64>,
}

impl SpeedMultiplierOverrides {
    fn load(json: &str) -> Self {
        serde_json::from_str(json).expect("parse embedded speed multiplier overrides")
    }

    fn multiplier_for(&self, model: &str) -> Option<f64> {
        if let Some(multiplier) = self.exact.get(model) {
            return Some(*multiplier);
        }
        // A default family alias bills at the variant it points to, so it
        // shares that variant's service-tier multiplier.
        if let Some(multiplier) = pricing_alias(model).and_then(|alias| self.exact.get(alias)) {
            return Some(*multiplier);
        }
        model
            .split(['/', ':'])
            .flat_map(|part| {
                // Keep decimal model versions intact while trying dotted provider suffixes.
                std::iter::once(part)
                    .chain(part.match_indices('.').map(|(index, _)| &part[index + 1..]))
            })
            .find_map(|part| {
                let base = model_without_date_suffix(part);
                self.exact
                    .get(part)
                    .or_else(|| self.exact.get(base))
                    .copied()
                    .or_else(|| {
                        pricing_alias(base).and_then(|alias| self.exact.get(alias).copied())
                    })
                    .or_else(|| {
                        let normalized = normalized_pricing_key(part);
                        self.normalized_prefix
                            .iter()
                            .find_map(|(base, multiplier)| {
                                matches_model_suffix(&normalized, base).then_some(*multiplier)
                            })
                    })
            })
    }
}

static EMBEDDED_PREWARM: Mutex<Option<JoinHandle<PricingMap>>> = Mutex::new(None);

/// Starts building the embedded pricing table on a background thread.
///
/// The next [`PricingMap::load_embedded`] call takes that table instead of
/// building its own, so a report can discover its log files while the
/// snapshots inflate and parse. Nothing starts when the
/// [`PricingMap::load_with_overrides`] base for `offline` is already loaded
/// or a prewarmed table is still waiting to be taken. The embedded snapshots
/// must already be installed.
pub fn prewarm_embedded(offline: bool) {
    if base_pricing(offline).get().is_some() {
        return;
    }
    let Ok(mut prewarm) = EMBEDDED_PREWARM.lock() else {
        return;
    };
    if prewarm.is_none() {
        *prewarm = thread::Builder::new()
            .spawn(PricingMap::build_embedded)
            .ok();
    }
}

/// Every agent of a combined report prices its own rows, so the tables are
/// parsed, and in online mode fetched, once per process rather than per agent.
fn base_pricing(offline: bool) -> &'static OnceLock<PricingMap> {
    static ONLINE: OnceLock<PricingMap> = OnceLock::new();
    static OFFLINE: OnceLock<PricingMap> = OnceLock::new();
    if offline { &OFFLINE } else { &ONLINE }
}

impl PricingMap {
    pub fn load_embedded() -> Self {
        let prewarm = EMBEDDED_PREWARM
            .lock()
            .ok()
            .and_then(|mut prewarm| prewarm.take());
        prewarm
            .and_then(|handle| handle.join().ok())
            .unwrap_or_else(Self::build_embedded)
    }

    fn build_embedded() -> Self {
        Self::from_litellm(build_time_pricing_json())
    }

    fn from_litellm(json: &str) -> Self {
        let mut map = Self::default();
        map.load_json(json);
        map.complete_layers(embedded_models_dev_pricing());
        map
    }

    fn complete_layers(&mut self, embedded_models_dev: &Self) {
        self.put_builtin_pricing();
        self.fill_long_context_rates_from_models_dev(embedded_models_dev);
        self.merge_models_dev(embedded_models_dev);
    }

    pub fn load_with_overrides<'a, I>(offline: bool, log: bool, overrides: I) -> Self
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        base_pricing(offline)
            .get_or_init(|| Self::load_base(offline, log))
            .with_user_overrides(overrides)
    }

    fn load_base(offline: bool, log: bool) -> Self {
        let mut map = Self::load_embedded();
        if !offline {
            crate::progress::track_status(
                log && crate::progress::usage_load_output_is_tty(),
                "Refreshing model pricing from LiteLLM...",
                || map.refresh_litellm(fetch_pricing_json),
            );
        }

        map.enable_models_dev_fallback = !offline;
        map
    }

    /// Returns an independent pricing map with additional user overrides.
    ///
    /// This lets callers share one fetched or embedded base between reports
    /// while keeping each report's override cache separate.
    /// The base map must not already contain user overrides.
    pub fn with_user_overrides<'a, I>(&self, overrides: I) -> Self
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        let base = self.live_merged.get().map_or(self, |map| map.as_ref());
        let mut map = Self {
            entries: base.entries.clone(),
            user_overrides: base.user_overrides.clone(),
            exact_only: base.exact_only.clone(),
            context_limits: base.context_limits.clone(),
            context_defaults: base.context_defaults.clone(),
            models_dev_keys: base.models_dev_keys.clone(),
            models_dev_limits: base.models_dev_limits.clone(),
            live_merged: OnceLock::new(),
            enable_models_dev_fallback: self.enable_models_dev_fallback,
            find_cache: OnceLock::new(),
            normalized_keys: OnceLock::new(),
        };
        map.apply_overrides(overrides);
        map
    }

    fn refresh_litellm<F>(&mut self, fetch_json: F)
    where
        F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
    {
        let mut loaded_count = 0;
        let fetched = fetch_json(&mut |json| {
            let mut refreshed = Self::default();
            loaded_count = refreshed.load_json(json);
            if loaded_count > 0 {
                refreshed.complete_layers(embedded_models_dev_pricing());
                *self = refreshed;
            }
            loaded_count > 0
        });
        if !should_log_pricing_refresh_details() {
            return;
        }
        match fetched {
            Ok(()) if loaded_count == 0 => {
                eprintln!("WARN  Failed to parse LiteLLM pricing; using embedded pricing.");
            }
            Ok(()) => {}
            Err(error) => {
                eprintln!(
                    "WARN  Failed to fetch LiteLLM pricing ({error}); using embedded pricing."
                );
            }
        }
    }

    pub fn load_json(&mut self, json: &str) -> usize {
        let Ok(raw) = serde_json::from_str::<FxHashMap<String, serde_json::Value>>(json) else {
            return 0;
        };
        let speed = SpeedMultipliers::get();
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

            let provider_specific = pricing
                .provider_specific_entry
                .as_ref()
                .map(|entry| PublishedSpeed {
                    fast: entry.fast,
                    ultrafast: None,
                    flex: entry.flex,
                })
                .unwrap_or_default();
            let normalized = normalized_pricing_key(&model);
            let replaced: Vec<_> = self
                .models_dev_keys
                .iter()
                .filter(|key| normalized_pricing_key(key) == normalized)
                .cloned()
                .collect();
            for key in replaced {
                self.entries.remove(&key);
                self.exact_only.remove(&key);
                self.models_dev_keys.remove(&key);
            }
            self.entries.insert(
                model.clone(),
                provider_specific
                    .or(speed.for_model(&model))
                    .applied_to(Pricing {
                        input,
                        output,
                        cache_create: pricing.cache_creation_input_token_cost,
                        cache_read: pricing.cache_read_input_token_cost,
                        cache_create_1h: pricing.cache_creation_input_token_cost_above_1hr,
                        cache_create_1h_above_200k: pricing
                            .cache_creation_input_token_cost_above_1hr_above_200k_tokens,
                        input_above_200k: pricing.input_cost_per_token_above_200k_tokens,
                        output_above_200k: pricing.output_cost_per_token_above_200k_tokens,
                        cache_create_above_200k: pricing
                            .cache_creation_input_token_cost_above_200k_tokens,
                        cache_read_above_200k: pricing
                            .cache_read_input_token_cost_above_200k_tokens,
                        ..Pricing::empty()
                    }),
            );
            if let Some(context_limit) = context_limit {
                self.context_defaults.remove(&model);
                self.models_dev_limits.remove(&model);
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

    /// Loads a synthetic models.dev payload for cross-module regression tests.
    #[cfg(test)]
    pub(crate) fn load_models_dev_json_for_tests(&mut self, json: &str) -> Option<usize> {
        self.load_models_dev_json_missing(json)
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
        let speed = SpeedMultipliers::get();
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

            self.entries.insert(
                model_id.clone(),
                speed.for_model(&model_id).applied_to(Pricing {
                    input,
                    output,
                    cache_create: cost.cache_write.map(|value| value / 1_000_000.0),
                    cache_read: cost.cache_read.map(|value| value / 1_000_000.0),
                    input_above_200k: long_context.and_then(|rates| rates.input),
                    output_above_200k: long_context.and_then(|rates| rates.output),
                    cache_create_above_200k: long_context.and_then(|rates| rates.cache_create),
                    cache_read_above_200k: long_context.and_then(|rates| rates.cache_read),
                    long_context_threshold: long_context.map(|rates| rates.threshold),
                    ..Pricing::empty()
                }),
            );
            if exact_only {
                self.exact_only.insert(model_id.clone());
            } else {
                self.exact_only.remove(&model_id);
            }
            match context_limit {
                Some(context_limit) => {
                    self.context_defaults.remove(&model_id);
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
        let cache = self
            .find_cache
            .get_or_init(|| RwLock::new(FxHashMap::default()));
        if let Some(&cached) = cache
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(model)
            .and_then(|slot| slot.get())
        {
            return cached;
        }
        let slot = Arc::clone(
            cache
                .write()
                .unwrap_or_else(|error| error.into_inner())
                .entry(model.to_string())
                .or_default(),
        );
        *slot.get_or_init(|| {
            let map = self.for_lookup(model, Lookup::Pricing);
            map.lookup(map.prices(), model, Fuzzy::Allowed)
        })
    }

    fn prices(&self) -> Table<'_, Pricing> {
        Table {
            rows: &self.entries,
            models_dev: &self.models_dev_keys,
        }
    }

    fn limits(&self) -> Table<'_, u64> {
        Table {
            rows: &self.context_limits,
            models_dev: &self.models_dev_limits,
        }
    }

    fn merge_models_dev(&mut self, source: &Self) {
        let mut primary = FxHashSet::default();
        let mut replaceable: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for key in self.entries.keys() {
            let normalized = normalized_pricing_key(key).into_owned();
            if self.models_dev_keys.contains(key) {
                replaceable.entry(normalized).or_default().push(key.clone());
            } else {
                primary.insert(normalized);
            }
        }
        for (model, pricing) in &source.entries {
            let normalized = normalized_pricing_key(model);
            if primary.contains(normalized.as_ref()) {
                continue;
            }
            for key in replaceable
                .remove(normalized.as_ref())
                .into_iter()
                .flatten()
            {
                self.entries.remove(&key);
                self.exact_only.remove(&key);
                self.models_dev_keys.remove(&key);
            }
            self.entries.insert(model.clone(), *pricing);
            self.models_dev_keys.insert(model.clone());
            if source.exact_only.contains(model) {
                self.exact_only.insert(model.clone());
            } else {
                self.exact_only.remove(model);
            }
        }
        for (model, limit) in &source.context_limits {
            if self.context_limits.contains_key(model)
                && !self.models_dev_limits.contains(model)
                && !self.context_defaults.remove(model)
            {
                continue;
            }
            self.context_limits.insert(model.clone(), *limit);
            self.models_dev_limits.insert(model.clone());
        }
        self.clear_find_cache();
    }

    fn for_lookup(&self, model: &str, lookup: Lookup) -> &Self {
        self.with_live_layer(model, lookup, models_dev_pricing)
    }

    fn with_live_layer<'a>(
        &self,
        model: &str,
        lookup: Lookup,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> &Self {
        if let Some(merged) = self.live_merged.get() {
            return merged;
        }
        if !self.enable_models_dev_fallback {
            return self;
        }
        let primary_hit = match lookup {
            Lookup::Context => self
                .primary_key(self.limits(), model)
                .is_some_and(|key| !self.context_defaults.contains(key)),
            Lookup::Pricing => self.primary_key(self.prices(), model).is_some(),
            Lookup::Exact => self
                .exact_normalized_key(model)
                .is_some_and(|key| self.prices().layer_of(key) == Layer::Primary),
        };
        if primary_hit {
            return self;
        }
        self.live_merged.get_or_init(|| {
            let mut merged = self.with_user_overrides(std::iter::empty());
            merged.enable_models_dev_fallback = false;
            if let Some(live) = live() {
                merged.merge_models_dev(live);
            }
            if let Some(cache) = self.find_cache.get() {
                cache
                    .write()
                    .unwrap_or_else(|error| error.into_inner())
                    .clear();
            }
            Box::new(merged)
        })
    }

    fn lookup<T: Copy>(&self, table: Table<'_, T>, model: &str, fuzzy: Fuzzy) -> Option<T> {
        self.resolve_key(table, model, fuzzy)
            .and_then(|key| table.rows.get(key).copied())
    }

    fn primary_key<'a, T>(&self, table: Table<'a, T>, model: &str) -> Option<&'a str> {
        let alias = crate::model_aliases::resolve_model_name(model);
        self.layer_key(table, model, alias.as_ref(), Layer::Primary, Fuzzy::Denied)
    }

    fn resolve_key<'a, T>(
        &self,
        table: Table<'a, T>,
        model: &str,
        fuzzy: Fuzzy,
    ) -> Option<&'a str> {
        let alias = crate::model_aliases::resolve_model_name(model);
        let resolved = alias.as_ref();
        let fuzzy = if fuzzy == Fuzzy::Allowed
            && ![model, resolved].into_iter().any(|candidate| {
                self.exact_only.contains_any_spelling(candidate)
                    || pricing_alias(candidate)
                        .is_some_and(|alias| self.exact_only.contains_any_spelling(alias))
            }) {
            Fuzzy::Allowed
        } else {
            Fuzzy::Denied
        };
        [Layer::Primary, Layer::ModelsDev]
            .into_iter()
            .find_map(|layer| {
                self.layer_key(table, model, resolved, layer, Fuzzy::Denied)
                    .or_else(|| {
                        (fuzzy == Fuzzy::Allowed)
                            .then(|| self.layer_key(table, model, resolved, layer, fuzzy))
                            .flatten()
                    })
            })
    }

    fn layer_key<'a, T>(
        &self,
        table: Table<'a, T>,
        model: &str,
        resolved: &str,
        layer: Layer,
        fuzzy: Fuzzy,
    ) -> Option<&'a str> {
        self.entry_or_alias_key(table, model, layer, fuzzy)
            .or_else(|| {
                (resolved != model)
                    .then(|| self.entry_or_alias_key(table, resolved, layer, fuzzy))
                    .flatten()
            })
    }

    fn entry_or_alias_key<'a, T>(
        &self,
        table: Table<'a, T>,
        model: &str,
        layer: Layer,
        fuzzy: Fuzzy,
    ) -> Option<&'a str> {
        table
            .key_in(model, layer)
            .or_else(|| {
                pricing_alias(model).and_then(|alias| self.entry_key(table, alias, layer, fuzzy))
            })
            .or_else(|| self.entry_key(table, model, layer, fuzzy))
    }

    fn entry_key<'a, T>(
        &self,
        table: Table<'a, T>,
        model: &str,
        layer: Layer,
        fuzzy: Fuzzy,
    ) -> Option<&'a str> {
        table.key_in(model, layer).or_else(|| {
            if let Some(id) = self.exact_only.id_spelled_by(model) {
                return table.key_in(id, layer);
            }
            if fuzzy == Fuzzy::Denied || self.exact_only.contains_any_spelling(model) {
                return None;
            }
            let normalized = normalized_pricing_key(model);
            table
                .rows
                .keys()
                .map(String::as_str)
                .filter(|key| table.layer_of(key) == layer)
                .filter(|key| !self.exact_only.contains(key))
                .filter(|key| pricing_key_matches(key, model, normalized.as_ref()))
                .max_by(|left, right| left.len().cmp(&right.len()).then_with(|| right.cmp(left)))
        })
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

    /// Whether a user supplied pricing for this model id or its configured alias.
    pub fn has_user_override(&self, model: &str) -> bool {
        self.user_overrides.contains_key(model)
            || self
                .user_overrides
                .contains_key(crate::model_aliases::resolve_model_name(model).as_ref())
    }

    pub fn find_exact_with_fallback(&self, model: &str) -> Option<Pricing> {
        if let Some(merged) = self.live_merged.get() {
            return merged.find_exact_normalized(model);
        }
        self.find_exact_normalized(model).or_else(|| {
            self.for_lookup(model, Lookup::Exact)
                .find_exact_normalized(model)
        })
    }

    fn find_exact_normalized(&self, model: &str) -> Option<Pricing> {
        self.exact_normalized_key(model)
            .and_then(|key| self.entries.get(key).copied())
    }

    fn exact_normalized_key(&self, model: &str) -> Option<&str> {
        if let Some((key, _)) = self.entries.get_key_value(model) {
            return Some(key);
        }
        if self.exact_only.contains_any_spelling(model) {
            return self
                .exact_only
                .id_spelled_by(model)
                .and_then(|id| self.entries.get_key_value(id))
                .map(|(key, _)| key.as_str());
        }
        let key = self
            .normalized_keys()
            .get(normalized_pricing_key(model).as_ref())?
            .as_deref()?;
        self.entries.get_key_value(key).map(|(key, _)| key.as_str())
    }

    fn normalized_keys(&self) -> &FxHashMap<String, Option<String>> {
        self.normalized_keys.get_or_init(|| {
            let mut index = FxHashMap::default();
            for key in self.entries.keys() {
                index
                    .entry(normalized_pricing_key(key).into_owned())
                    // A second key with the same spelling makes it name
                    // neither entry.
                    .and_modify(|slot| *slot = None)
                    .or_insert_with(|| Some(key.clone()));
            }
            index
        })
    }

    pub fn context_limit(&self, model: &str) -> Option<u64> {
        let map = self.for_lookup(model, Lookup::Context);
        map.lookup(map.limits(), model, Fuzzy::Allowed)
    }

    fn apply_overrides<'a, I>(&mut self, overrides: I)
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        let overrides: FxHashMap<&'a str, &'a PricingOverride> = overrides
            .into_iter()
            .map(|(model, override_value)| (model.as_str(), override_value))
            .collect();
        let mut resolved = FxHashMap::default();
        for model in overrides.keys() {
            self.resolve_override(model, &overrides, &mut resolved);
        }
        for (model, pricing) in resolved {
            self.models_dev_keys.remove(model);
            self.entries.insert(model.to_string(), pricing);
        }
        for (model, override_value) in overrides {
            self.user_overrides
                .insert(model.to_string(), override_value.clone());
            if let Some(limit) = override_value.max_input_tokens {
                self.context_defaults.remove(model);
                self.models_dev_limits.remove(model);
                self.context_limits.insert(model.to_string(), limit);
            }
        }
        self.clear_find_cache();
    }

    fn resolve_override<'a>(
        &self,
        model: &'a str,
        overrides: &FxHashMap<&'a str, &'a PricingOverride>,
        resolved: &mut FxHashMap<&'a str, Pricing>,
    ) -> Pricing {
        if let Some(pricing) = resolved.get(model) {
            return *pricing;
        }
        let follow_overrides = !self.override_in_cycle(model, overrides);
        let base = match self.override_base(model, overrides, follow_overrides) {
            OverrideBase::Override(dependency) => {
                self.resolve_override(dependency, overrides, resolved)
            }
            OverrideBase::Pricing(base) => base,
        };
        let pricing = overridden_pricing(base, overrides[model]);
        resolved.insert(model, pricing);
        pricing
    }

    fn override_base<'a>(
        &self,
        model: &str,
        overrides: &FxHashMap<&'a str, &'a PricingOverride>,
        follow_overrides: bool,
    ) -> OverrideBase<'a> {
        let resolved_model = crate::model_aliases::resolve_model_name(model);
        let candidates = [
            Some(model),
            pricing_alias(model),
            Some(resolved_model.as_ref()),
            pricing_alias(resolved_model.as_ref()),
        ];
        for candidate in candidates.into_iter().flatten() {
            if follow_overrides
                && candidate != model
                && let Some((&dependency, _)) = overrides.get_key_value(candidate)
            {
                return OverrideBase::Override(dependency);
            }
            if let Some(pricing) = self.entries.get(candidate) {
                return OverrideBase::Pricing(*pricing);
            }
        }
        let speed = SpeedMultipliers::get();
        OverrideBase::Pricing(
            speed
                .for_model(model)
                .or(speed.for_model(resolved_model.as_ref()))
                .applied_to(Pricing { ..Pricing::empty() }),
        )
    }

    fn override_in_cycle(
        &self,
        model: &str,
        overrides: &FxHashMap<&str, &PricingOverride>,
    ) -> bool {
        let mut current = model;
        for _ in 0..overrides.len() {
            match self.override_base(current, overrides, true) {
                OverrideBase::Override(next) if next == model => return true,
                OverrideBase::Override(next) => current = next,
                OverrideBase::Pricing(_) => return false,
            }
        }
        false
    }

    fn clear_find_cache(&mut self) {
        self.live_merged.take();
        if let Some(cache) = self.find_cache.get_mut() {
            cache
                .get_mut()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        self.normalized_keys.take();
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
    /// is billed at the base rate.
    ///
    /// Entries that already carry any tier rate are left untouched so upstream
    /// data wins once it exists. The check is deliberately all-or-nothing
    /// rather than per field: each source's rates assume that source's own
    /// boundary, so mixing fields across sources would price both tiers wrong.
    fn fill_long_context_rates_from_models_dev(&mut self, tiers: &Self) {
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

    fn supplement_pricing(&mut self, model: &str, pricing: Pricing, cache_fields: bool) {
        let pricing = SpeedMultipliers::get().for_model(model).applied_to(pricing);
        match self.entries.entry(model.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut existing) if cache_fields => {
                let existing = existing.get_mut();
                existing.cache_read = existing.cache_read.or(pricing.cache_read);
                existing.cache_create = existing.cache_create.or(pricing.cache_create);
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(pricing);
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
        self.clear_find_cache();
    }

    fn put_builtin_pricing(&mut self) {
        for supplement in BUILTIN_PRICING {
            self.supplement_pricing(
                supplement.model,
                supplement.pricing,
                supplement.cache_fields,
            );
            if let Some(limit) = supplement.context_limit
                && let std::collections::hash_map::Entry::Vacant(entry) =
                    self.context_limits.entry(supplement.model.to_string())
            {
                entry.insert(limit);
                if supplement.context_fallback {
                    self.context_defaults.insert(supplement.model.to_string());
                }
            }
        }
    }
}

struct Supplement {
    model: &'static str,
    pricing: Pricing,
    context_limit: Option<u64>,
    cache_fields: bool,
    context_fallback: bool,
}

const fn rates(input: f64, output: f64, cache_create: f64, cache_read: f64) -> Pricing {
    Pricing {
        input,
        output,
        cache_create: Some(cache_create),
        cache_read: Some(cache_read),
        ..Pricing::empty()
    }
}

#[path = "builtin-pricing.rs"]
mod builtin_pricing;

use builtin_pricing::BUILTIN_PRICING;

enum OverrideBase<'a> {
    Override(&'a str),
    Pricing(Pricing),
}

fn overridden_pricing(mut base: Pricing, override_value: &PricingOverride) -> Pricing {
    apply_explicit_pricing_override(&mut base, override_value);
    base
}

fn parse_litellm_pricing(value: Value) -> Option<LiteLlmPricing> {
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

fn is_pricing_key_separator(byte: u8) -> bool {
    matches!(byte, b'.' | b'@')
}

fn dash_pricing_key_separators(chars: impl Iterator<Item = char>) -> String {
    chars
        .map(|char| if matches!(char, '.' | '@') { '-' } else { char })
        .collect()
}

/// Normalizes known model separator variants without allocating for canonical keys.
fn normalized_pricing_key(value: &str) -> Cow<'_, str> {
    if value.bytes().any(is_pricing_key_separator) {
        Cow::Owned(dash_pricing_key_separators(value.chars()))
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
        "gpt-reserve" => Some("gpt-5.6-luna"),
        "gpt-5.6" => Some("gpt-5.6-sol"),
        "gpt-5.3-spark" => Some("gpt-5.3-codex-spark"),
        _ => None,
    }
}

fn matches_model_suffix(part: &str, base: &str) -> bool {
    // Model ids are a few dozen bytes, so scanning windows beats building a
    // substring searcher for each of the thousands of startup lookups.
    let (part, base) = (part.as_bytes(), base.as_bytes());
    if base.is_empty() {
        return true;
    }
    let Some(index) = part.windows(base.len()).rposition(|window| window == base) else {
        return false;
    };
    part.get(index + base.len())
        .is_none_or(|&next| next == b'-')
}

fn should_log_pricing_refresh_details() -> bool {
    crate::log_level().is_some_and(|level| level >= 4)
}

fn models_dev_pricing() -> Option<&'static PricingMap> {
    static MODELS_DEV_PRICING: OnceLock<Option<PricingMap>> = OnceLock::new();
    load_models_dev_pricing_once(&MODELS_DEV_PRICING, fetch_models_dev_json)
}

fn load_models_dev_pricing_once<F>(
    cell: &OnceLock<Option<PricingMap>>,
    fetch_json: F,
) -> Option<&PricingMap>
where
    F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
{
    cell.get_or_init(|| load_models_dev_pricing(fetch_json))
        .as_ref()
}

/// Pricing built from the models.dev snapshot embedded at build time. Unlike the
/// network source this is always available, so it lets offline runs price models
/// that LiteLLM and the built-in table do not cover (for example newly released
/// Anthropic models).
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
    F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
{
    let mut map = PricingMap::default();
    let mut loaded = false;
    let fetched = fetch_json(&mut |json| {
        loaded = map
            .load_models_dev_json_missing(json)
            .is_some_and(|loaded_count| loaded_count > 0);
        loaded
    });
    match fetched {
        Ok(()) if loaded => Some(map),
        Ok(()) => {
            if should_log_pricing_refresh_details() {
                eprintln!("WARN  Failed to parse models.dev pricing; using LiteLLM pricing.");
            }
            None
        }
        Err(error) => {
            if should_log_pricing_refresh_details() {
                eprintln!(
                    "WARN  Failed to fetch models.dev pricing ({error}); using LiteLLM pricing."
                );
            }
            None
        }
    }
}

fn fetch_pricing_json(load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    fetch_json_url(LITELLM_PRICING_URL, load)
}

fn fetch_models_dev_json(load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    fetch_json_url(MODELS_DEV_API_URL, load)
}

pub type JsonFetcher = fn(&str, &mut dyn FnMut(&str) -> bool) -> std::io::Result<()>;

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

fn fetch_json_url(url: &str, load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    let Some(fetch) = JSON_FETCHER.get() else {
        return Err(std::io::Error::other(
            "no HTTP client installed for pricing refresh",
        ));
    };
    fetch(url, load)
}

#[cfg(test)]
mod tests {
    use super::{
        EMBEDDED_PREWARM, Fuzzy, Lookup, Pricing, PricingMap, build_time_models_dev_json,
        build_time_pricing_json, embedded_models_dev_pricing, long_context_split_threshold,
        model_without_date_suffix, prewarm_embedded,
    };
    use ccusage_test_support::fs_fixture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    impl PricingMap {
        fn put_builtin_entry(&mut self, model: &str, pricing: Pricing) {
            self.supplement_pricing(model, pricing, false);
        }

        fn supplement_cache_rates(&mut self, model: &str, pricing: Pricing) {
            self.supplement_pricing(model, pricing, true);
        }

        fn find_entry(&self, model: &str, fuzzy: Fuzzy) -> Option<Pricing> {
            self.lookup(self.prices(), model, fuzzy)
        }

        fn context_limit_entry(&self, model: &str, fuzzy: Fuzzy) -> Option<u64> {
            self.lookup(self.limits(), model, fuzzy)
        }

        fn find_with_live<'a>(
            &self,
            model: &str,
            live: impl FnOnce() -> Option<&'a Self>,
        ) -> Option<Pricing> {
            self.with_live_layer(model, Lookup::Pricing, live)
                .find(model)
        }

        fn context_limit_with_live<'a>(
            &self,
            model: &str,
            live: impl FnOnce() -> Option<&'a Self>,
        ) -> Option<u64> {
            self.with_live_layer(model, Lookup::Context, live)
                .context_limit(model)
        }
    }

    fn online(primary: &PricingMap) -> PricingMap {
        let mut map = primary.with_user_overrides(std::iter::empty());
        map.enable_models_dev_fallback = true;
        map
    }

    fn layered(primary: &PricingMap, embedded_models_dev: Option<&PricingMap>) -> PricingMap {
        let empty = PricingMap::default();
        let mut map = online(primary);
        map.complete_layers(embedded_models_dev.unwrap_or(&empty));
        map
    }

    #[test]
    fn live_merge_is_lazy_once_only_and_invalidates_cached_fuzzy_hits_and_misses() {
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
        assert_eq!(pricing.find("sample-special").unwrap().input, 1.0);
        assert!(pricing.find("other-model").is_none());
        let mut live = PricingMap::default();
        live.load_json(
            r#"{
            "sample-special": {"input_cost_per_token": 3, "output_cost_per_token": 4},
            "other-model": {"input_cost_per_token": 5, "output_cost_per_token": 6}
        }"#,
        );
        live.exact_only.insert("sample-special".to_string());
        pricing.enable_models_dev_fallback = true;
        let attempts = AtomicUsize::new(0);
        let fetch = || {
            attempts.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        assert_eq!(
            pricing
                .with_live_layer("sample", super::Lookup::Pricing, fetch)
                .find_exact("sample")
                .unwrap()
                .input,
            1.0
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
        let merged = pricing.with_live_layer("trigger", super::Lookup::Pricing, fetch);
        assert_eq!(merged.find_exact("sample-special").unwrap().input, 3.0);
        assert_eq!(pricing.find("sample-special").unwrap().input, 3.0);
        assert_eq!(pricing.find("other-model").unwrap().input, 5.0);
        pricing.with_live_layer("trigger-again", super::Lookup::Pricing, fetch);
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn live_context_metadata_replaces_a_curated_default() {
        let mut pricing = PricingMap::default();
        pricing.put_builtin_pricing();
        pricing.enable_models_dev_fallback = true;
        let mut live = PricingMap::default();
        live.context_limits
            .insert("gpt-5.6-sol".to_string(), 654_321);
        let merged = pricing.with_live_layer("gpt-5.6-sol", super::Lookup::Context, || Some(&live));
        assert_eq!(merged.context_limit("gpt-5.6-sol"), Some(654_321));
        assert_eq!(pricing.context_limit("gpt-5.6-sol"), Some(654_321));
    }

    #[test]
    fn cache_supplement_uses_the_shared_speed_policy_when_the_primary_entry_is_missing() {
        let mut pricing = PricingMap::default();
        pricing.put_builtin_pricing();
        assert_eq!(pricing.find_exact("gpt-5.2").unwrap().flex_multiplier, 0.5);
    }

    #[test]
    fn supplements_preserve_all_published_prices_and_context_metadata() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{"gpt-5.2-codex": {
            "input_cost_per_token": 10,
            "output_cost_per_token": 20,
            "cache_creation_input_token_cost": 30,
            "cache_read_input_token_cost": 40,
            "max_input_tokens": 123,
            "provider_specific_entry": {"fast": 3, "flex": 0.25}
        }}"#,
        );
        pricing.put_builtin_pricing();
        let entry = pricing.find("gpt-5.2-codex").unwrap();
        assert_eq!(entry.input, 10.0);
        assert_eq!(entry.output, 20.0);
        assert_eq!(entry.cache_create, Some(30.0));
        assert_eq!(entry.cache_read, Some(40.0));
        assert_eq!(entry.fast_multiplier, 3.0);
        assert_eq!(entry.flex_multiplier, 0.25);
        assert_eq!(pricing.context_limit("gpt-5.2-codex"), Some(123));
    }

    #[test]
    fn provider_prefixed_ids_are_billed_at_the_vendor_list_price_offline() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for model in [
            "anthropic/claude-sonnet-4",
            "openrouter/anthropic/claude-sonnet-4:beta",
        ] {
            let entry = pricing.find(model).unwrap();
            assert_eq!(entry.input, 3e-6, "{model}");
            assert_eq!(entry.output, 15e-6, "{model}");
            assert_eq!(entry.cache_create, Some(3.75e-6), "{model}");
            assert_eq!(entry.cache_read, Some(0.3e-6), "{model}");
        }
    }

    #[test]
    fn online_and_offline_apply_the_same_cache_supplements() {
        crate::test_support::install_pricing_snapshots();
        let offline = PricingMap::load_embedded();
        let mut online = PricingMap::load_embedded();
        online.refresh_litellm(|load| {
            assert!(load(build_time_pricing_json()));
            Ok(())
        });
        for (model, expected_write) in [("zai/glm-4.5", 0.0), ("gpt-5.2-codex", 1.75e-6)] {
            let live = online.find(model).unwrap();
            let embedded = offline.find(model).unwrap();
            assert_eq!(live.cache_creation_rate(), expected_write, "{model}");
            assert_eq!(
                live.cache_creation_rate(),
                embedded.cache_creation_rate(),
                "{model}"
            );
            assert_eq!(live.cache_read, embedded.cache_read, "{model}");
        }
    }

    #[test]
    fn primary_fuzzy_pricing_beats_an_embedded_exact_reseller_listing() {
        let mut primary = PricingMap::default();
        primary.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{
                "vendor/sample": { "cost": { "input": 3000000, "output": 4000000 } },
                "sample-tier": { "cost": { "input": 5000000, "output": 6000000 }, "exactOnly": true },
                "unrelated-9": { "cost": { "input": 7000000, "output": 8000000 } }
            }"#,
        );
        let pricing = layered(&primary, Some(&embedded));
        assert_eq!(pricing.find("vendor/sample").unwrap().input, 1.0);
        assert_eq!(pricing.find("sample-tier").unwrap().input, 5.0);
        assert_eq!(pricing.find("unrelated-9").unwrap().input, 7.0);
    }

    #[test]
    fn live_models_dev_replaces_the_embedded_snapshot_for_a_models_dev_only_id() {
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"md-only-1": { "cost": { "input": 1000000, "output": 2000000 }, "limit": { "context": 1000 } }}"#,
        );
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"md-only-1": { "cost": { "input": 3000000, "output": 4000000 }, "limit": { "context": 3000 } }}"#,
        );
        let pricing = layered(&PricingMap::default(), Some(&embedded));
        let lookups = AtomicUsize::new(0);
        let fetch = || {
            lookups.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        assert_eq!(
            pricing.find_with_live("md-only-1", fetch).unwrap().input,
            3.0
        );
        assert_eq!(
            pricing.context_limit_with_live("md-only-1", fetch),
            Some(3000)
        );
        assert_eq!(lookups.load(Ordering::Relaxed), 1);

        let offline = layered(&PricingMap::default(), Some(&embedded));
        assert_eq!(
            offline.find_with_live("md-only-1", || None).unwrap().input,
            1.0
        );
    }

    #[test]
    fn loads_embedded_claude_pricing() {
        crate::test_support::install_pricing_snapshots();
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

    fn exact_only_fast_fallback_fixture() -> (PricingMap, PricingMap, PricingMap) {
        let mut primary = PricingMap::default();
        assert_eq!(
            primary.load_json(
                r#"{
                    "claude-opus-5": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    }
                }"#,
            ),
            1
        );

        let mut models_dev = PricingMap::default();
        assert_eq!(
            models_dev.load_models_dev_json_for_tests(
                r#"{
                    "claude-opus-5": {
                        "cost": { "input": 3, "output": 4 },
                        "limit": { "context": 234567 }
                    }
                }"#,
            ),
            Some(1)
        );

        let mut embedded_models_dev = PricingMap::default();
        assert_eq!(
            embedded_models_dev.load_models_dev_json_for_tests(
                r#"{
                    "claude-opus-5-fast": {
                        "cost": { "input": 9, "output": 17 },
                        "limit": { "context": 765432 },
                        "exactOnly": true
                    }
                }"#,
            ),
            Some(1)
        );
        (primary, models_dev, embedded_models_dev)
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
            for (actual, expected) in [flash.input, flash.output, flash.cache_read.unwrap()]
                .map(|rate| rate * 1e6)
                .into_iter()
                .zip(flash_expected)
            {
                assert!((actual - expected).abs() < 1e-12);
            }

            let pro = pricing
                .find_at("deepseek-v4-pro", timestamp(timestamp_text))
                .unwrap();
            for (actual, expected) in [pro.input, pro.output, pro.cache_read.unwrap()]
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
        assert_eq!(direct.cache_create.unwrap() * 1e6, 0.44);

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
        assert_eq!(reseller.cache_read.unwrap() * 1e6, 8.0);
        assert_eq!(reseller.cache_create.unwrap() * 1e6, 7.0);
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
        crate::test_support::install_pricing_snapshots();
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
        assert!((flash.cache_read.unwrap() * 1e6 - 0.014).abs() < 1e-12);
    }

    #[test]
    fn prewarm_skips_a_build_once_the_base_pricing_is_loaded() {
        crate::test_support::install_pricing_snapshots();
        PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &ccusage_cli::PricingOverride)>(),
        );
        prewarm_embedded(true);

        assert!(EMBEDDED_PREWARM.lock().unwrap().is_none());
    }

    #[test]
    fn one_hour_cache_override_replaces_the_published_rate() {
        crate::test_support::install_pricing_snapshots();
        let model = "claude-sonnet-4-20250514".to_string();
        for rate in [0.0, 7e-6] {
            let override_value = ccusage_cli::PricingOverride {
                cache_creation_input_token_cost_above_1hr: Some(rate),
                ..Default::default()
            };
            let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);
            assert_eq!(pricing.find(&model).unwrap().cache_create_1h, Some(rate));
        }
    }

    #[test]
    fn user_deepseek_v4_overrides_remain_authoritative() {
        crate::test_support::install_pricing_snapshots();
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
        assert_eq!(flash.cache_read.unwrap() * 1e6, 8.0);
        assert_eq!(flash.cache_create.unwrap() * 1e6, 7.0);
    }

    #[test]
    fn partial_deepseek_v4_override_keeps_schedule_for_unspecified_fields() {
        crate::test_support::install_pricing_snapshots();
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
        assert_eq!(peak.cache_read.unwrap() * 1e6, 0.014);
        assert_eq!(peak.cache_create.unwrap() * 1e6, 0.44);

        let off_peak = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T04:00:00Z"))
            .unwrap();
        assert_eq!(off_peak.input * 1e6, 9.0);
        assert_eq!(off_peak.output * 1e6, 0.66);
        assert_eq!(off_peak.cache_read.unwrap() * 1e6, 0.007);
        assert_eq!(off_peak.cache_create.unwrap() * 1e6, 0.22);
    }

    #[test]
    fn each_pricing_loader_loads_only_its_own_document() {
        crate::test_support::install_pricing_snapshots();
        let litellm =
            r#"{"gpt-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
        let models_dev =
            r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;

        assert_eq!(PricingMap::default().load_json(litellm), 1);
        assert_eq!(PricingMap::default().load_json(models_dev), 0);
        assert_eq!(
            PricingMap::default().load_models_dev_json_missing(models_dev),
            Some(1)
        );
        assert_eq!(
            PricingMap::default().load_models_dev_json_missing(litellm),
            None
        );
    }

    #[test]
    fn accepts_a_fetched_models_dev_body_only_when_it_prices_a_model() {
        crate::test_support::install_pricing_snapshots();
        let models_dev =
            r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;
        let litellm =
            r#"{"gpt-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
        let free = r#"{"openai":{"models":{"free":{"cost":{"input":0.0,"output":0.0}}}}}"#;
        let asset_priced = r#"{"openai":{"models":{"unpriced":{"modalities":{"input":[],"output":["text"]},"cost":{"input":1.0,"output":2.0}}}}}"#;

        for (body, expected) in [
            (models_dev, true),
            (litellm, false),
            ("{}", false),
            (free, false),
            (asset_priced, false),
        ] {
            let mut accepted = None;
            let pricing = super::load_models_dev_pricing(|load| {
                accepted = Some(load(body));
                Ok(())
            });
            assert_eq!(accepted, Some(expected), "{body}");
            assert_eq!(pricing.is_some(), expected, "{body}");
        }
    }

    #[test]
    fn accepts_a_fetched_litellm_body_only_when_it_prices_a_model() {
        crate::test_support::install_pricing_snapshots();
        let litellm =
            r#"{"gpt-fetched":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
        let models_dev =
            r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;

        for (body, expected) in [(litellm, true), (models_dev, false), ("{}", false)] {
            let mut pricing = PricingMap::default();
            let mut accepted = None;
            pricing.refresh_litellm(|load| {
                accepted = Some(load(body));
                Ok(())
            });
            assert_eq!(accepted, Some(expected), "{body}");
            assert_eq!(
                pricing.find_exact("gpt-fetched").is_some(),
                expected,
                "{body}"
            );
        }
    }

    #[test]
    fn reads_embedded_model_context_limits() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        let _ = pricing.context_limit("anthropic.claude-3-5-sonnet-20240620-v1:0");
    }

    #[test]
    fn embedded_pricing_includes_hermes_frontier_models() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        assert!(pricing.find("gpt-5.5").is_some());
        assert!(pricing.find("grok-4.3").is_some());
        assert_eq!(pricing.context_limit("grok-4.3"), Some(1_000_000));
    }

    #[test]
    fn embedded_pricing_includes_moonshot_kimi_for_offline_reports() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let kimi_k25 = pricing.find("moonshot/kimi-k2.5").unwrap();
        let kimi_k26 = pricing.find("moonshot/kimi-k2.6").unwrap();

        assert_eq!(kimi_k25.input, 0.6e-6);
        assert_eq!(kimi_k25.output, 3e-6);
        assert_eq!(kimi_k25.cache_read, Some(0.1e-6));
        assert!(kimi_k25.cache_read.is_some());
        assert_eq!(kimi_k26.input, 0.95e-6);
        assert_eq!(kimi_k26.output, 4e-6);
        assert_eq!(kimi_k26.cache_read, Some(0.16e-6));
        assert!(kimi_k26.cache_read.is_some());
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.5"), Some(262_144));
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.6"), Some(262_144));
    }

    #[test]
    fn offline_prices_models_outside_the_claude_and_moonshot_families() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
    fn exact_only_fallback_beats_a_fuzzy_primary_match() {
        crate::test_support::install_pricing_snapshots();
        // The snapshot lives in its own map, consulted only after the primary
        // one misses, so marking `claude-opus-5-fast` exact-only there is not
        // enough on its own: a primary or live fuzzy scan could answer with the
        // base `claude-opus-5` entry and bill the tier at list price.
        let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
        let pricing = layered(&primary, Some(&embedded_models_dev));

        let base = pricing.find("claude-opus-5").unwrap();
        let fast = pricing
            .find_with_live("claude-opus-5-fast", || Some(&models_dev))
            .expect("the exact-only fallback prices the Fast tier");
        assert_eq!(fast.input, 9e-6);
        assert_eq!(fast.output, 17e-6);
        assert_ne!(fast.input, base.input);
        assert_eq!(
            pricing.context_limit_with_live("claude-opus-5-fast", || Some(&models_dev)),
            Some(765_432)
        );
    }

    #[test]
    fn live_exact_only_fallback_beats_a_fuzzy_primary_match() {
        crate::test_support::install_pricing_snapshots();
        // The live catalog can add a separately priced tier before the
        // embedded snapshot is regenerated. Its exact-only verdict must gate
        // the primary map's fuzzy scan or that scan bills the base model.
        let mut primary = PricingMap::default();
        assert_eq!(
            primary.load_json(
                r#"{
                    "kimi-k2.7-code": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    }
                }"#,
            ),
            1
        );

        let mut models_dev = PricingMap::default();
        assert_eq!(
            models_dev.load_models_dev_json_missing(
                r#"{
                    "moonshotai": {
                        "models": {
                            "kimi-k2.7-code-highspeed": {
                                "cost": { "input": 9, "output": 17 },
                                "limit": { "context": 765432 }
                            }
                        }
                    }
                }"#,
            ),
            Some(1)
        );
        assert!(models_dev.exact_only.contains("kimi-k2.7-code-highspeed"));

        let pricing = layered(&primary, None);
        let tier = pricing
            .find_with_live("kimi-k2.7-code-highspeed", || Some(&models_dev))
            .expect("the live fallback prices the Highspeed tier");
        assert_eq!(tier.input, 9e-6);
        assert_eq!(tier.output, 17e-6);
        assert_eq!(
            pricing.context_limit_with_live("kimi-k2.7-code-highspeed", || Some(&models_dev)),
            Some(765_432)
        );

        let pricing = layered(&primary, None);
        let live_lookups = AtomicUsize::new(0);
        let live = || {
            live_lookups.fetch_add(1, Ordering::Relaxed);
            Some(&models_dev)
        };
        let base = pricing
            .find_with_live("kimi-k2.7-code", live)
            .expect("the primary map prices the base model exactly");
        assert_eq!(base.input, 1e-6);
        assert_eq!(
            pricing.context_limit_with_live("kimi-k2.7-code", live),
            Some(123_456)
        );
        assert_eq!(live_lookups.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn embedded_pricing_carries_exact_only_catalog_entries() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let embedded = embedded_models_dev_pricing();

        for model in ["claude-opus-5-fast", "claude-opus-5@eu"] {
            assert!(embedded.exact_only.contains(model), "{model}");
            assert!(pricing.find(model).is_some(), "{model}");
            assert!(pricing.context_limit(model).is_some(), "{model}");
        }

        let expected_fast = embedded.find_exact("claude-opus-5-fast").unwrap();
        let actual_fast = pricing.find("claude-opus-5-fast").unwrap();
        assert_eq!(actual_fast.input, expected_fast.input);
        assert_eq!(actual_fast.output, expected_fast.output);
        assert_eq!(
            pricing.context_limit("claude-opus-5-fast"),
            embedded.context_limit("claude-opus-5-fast")
        );

        // Gating keys the snapshot carries must not cost keys nothing prices
        // exactly their fuzzy match.
        assert!(pricing.find("claude-opus-5-20260115").is_some());
    }

    #[test]
    fn a_separator_spelling_of_an_exact_only_id_is_priced_as_that_id() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
        // The catalog writes most exact-only ids with dashes alone, and those
        // have to be indexed under their normalized spelling too: a request for
        // `claude-opus-5.fast` normalizes to the id the snapshot carries, so
        // leaving it out of the index lets a primary or live fuzzy scan answer
        // with the base `claude-opus-5` entry and bill the tier at list price.
        let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
        let pricing = layered(&primary, Some(&embedded_models_dev));

        let base = pricing.find("claude-opus-5").unwrap();
        let dotted = pricing
            .find_with_live("claude-opus-5.fast", || Some(&models_dev))
            .expect("the dotted spelling names the Fast tier");
        assert_eq!(dotted.input, 9e-6);
        assert_eq!(dotted.output, 17e-6);
        assert!(dotted.input > base.input);
        assert_eq!(
            pricing.context_limit_with_live("claude-opus-5.fast", || Some(&models_dev)),
            Some(765_432)
        );
    }

    #[test]
    fn exact_fallback_lookup_resolves_separator_spellings_of_exact_only_ids() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
    fn exact_fallback_lookup_rejects_a_spelling_shared_by_two_entries() {
        // `acme-alpha-1.5` and `acme.alpha@1.5` both normalize to
        // `acme-alpha-1-5`, so that spelling names neither entry.
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "acme-alpha-1.5": { "input_cost_per_token": 0.000001 },
                "acme.alpha@1.5": { "input_cost_per_token": 0.000002 }
            }"#,
        );

        assert!(pricing.find_exact_with_fallback("acme-alpha-1-5").is_none());
    }

    #[test]
    fn exact_fallback_lookup_sees_a_colliding_entry_added_after_a_lookup() {
        let mut pricing = PricingMap::default();
        pricing.put_builtin_entry(
            "acme-alpha-1.5",
            Pricing {
                input: 1e-6,
                ..Pricing::empty()
            },
        );
        let resolved = pricing
            .find_exact_with_fallback("acme-alpha-1-5")
            .expect("a single normalized match should resolve");
        assert_eq!(resolved.input, 1e-6);

        pricing.put_builtin_entry(
            "acme.alpha@1.5",
            Pricing {
                input: 2e-6,
                ..Pricing::empty()
            },
        );

        assert!(pricing.find_exact_with_fallback("acme-alpha-1-5").is_none());
    }

    #[test]
    fn exact_fallback_lookup_sees_rates_patched_after_a_lookup() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "zai/glm-test.5": {
                    "input_cost_per_token": 0.2,
                    "output_cost_per_token": 1.1
                }
            }"#,
        );
        let before = pricing
            .find_exact_with_fallback("zai/glm-test-5")
            .expect("the normalized entry should resolve");
        assert!(before.cache_read.is_none());

        pricing.supplement_cache_rates(
            "zai/glm-test.5",
            Pricing {
                cache_read: Some(0.03),

                ..Pricing::empty()
            },
        );

        let after = pricing.find_exact_with_fallback("zai/glm-test-5").unwrap();
        assert_eq!(after.cache_read, Some(0.03));
    }

    #[test]
    fn configured_alias_of_an_exact_only_tier_beats_a_fuzzy_match_on_the_alias() {
        crate::test_support::install_pricing_snapshots();
        // The alias target is only tried after this map has answered for the
        // recorded spelling, and that spelling can fuzzy-match a different model
        // in the primary or live source - here the base `claude-opus-5` - which
        // would bill the tier at list price and never reach what the alias names.
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "claude-opus-5-turbo",
            "claude-opus-5-fast",
        )]);
        let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
        let pricing = layered(&primary, Some(&embedded_models_dev));

        let turbo = pricing
            .find_with_live("claude-opus-5-turbo", || Some(&models_dev))
            .expect("the alias resolves to the Fast tier");
        assert_eq!(turbo.input, 9e-6);
        assert_eq!(turbo.output, 17e-6);
        assert_eq!(
            pricing.context_limit_with_live("claude-opus-5-turbo", || Some(&models_dev)),
            Some(765_432)
        );

        let mut builtin = PricingMap::default();
        builtin
            .context_limits
            .insert("claude-opus-5".to_string(), 123_456);
        let mut exact_only_without_context = PricingMap::default();
        assert_eq!(
            exact_only_without_context.load_models_dev_json_for_tests(
                r#"{
                    "claude-opus-5-fast": {
                        "cost": { "input": 9, "output": 17 },
                        "exactOnly": true
                    }
                }"#,
            ),
            Some(1)
        );
        assert_eq!(
            layered(&builtin, Some(&exact_only_without_context))
                .context_limit_with_live("claude-opus-5-turbo", || None),
            None,
            "an exact-only alias without a published limit must not inherit a fuzzy builtin limit"
        );
    }

    #[test]
    fn offline_prices_kimi_k3_from_embedded_models_dev() {
        crate::test_support::install_pricing_snapshots();
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
        assert_eq!(kimi_k3.cache_read, Some(0.3e-6));
        assert!(kimi_k3.cache_read.is_some());
        assert!(
            pricing
                .context_limit("moonshot/kimi-k3")
                .or_else(|| pricing.context_limit("kimi-k3"))
                == Some(1_048_576)
        );
    }

    #[test]
    fn embedded_pricing_includes_z_ai_glm_models_for_offline_reports() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        let glm_51 = pricing.find("glm-5.1").unwrap();
        assert_eq!(glm_51.input, 1.4e-6);
        assert_eq!(glm_51.output, 4.4e-6);
        assert_eq!(glm_51.cache_create, Some(0.0));
        assert_eq!(glm_51.cache_read, Some(0.26e-6));
        assert!(glm_51.cache_read.is_some());

        let glm_5 = pricing.find("glm-5").unwrap();
        assert_eq!(glm_5.input, 1.0e-6);
        assert_eq!(glm_5.output, 3.2e-6);
        assert_eq!(glm_5.cache_create, Some(0.0));
        assert_eq!(glm_5.cache_read, Some(0.2e-6));
        assert_eq!(pricing.context_limit("zai/glm-5"), Some(200_000));

        let glm_5_turbo = pricing.find("glm-5-turbo").unwrap();
        assert_eq!(glm_5_turbo.input, 1.2e-6);
        assert_eq!(glm_5_turbo.output, 4.0e-6);
        assert_eq!(glm_5_turbo.cache_create, Some(0.0));
        assert_eq!(glm_5_turbo.cache_read, Some(0.24e-6));

        let glm_47 = pricing.find("glm-4.7").unwrap();
        assert_eq!(glm_47.input, 0.6e-6);
        assert_eq!(glm_47.output, 2.2e-6);
        assert_eq!(glm_47.cache_create, Some(0.0));
        assert_eq!(glm_47.cache_read, Some(0.11e-6));

        let glm_46 = pricing.find("glm-4.6").unwrap();
        assert_eq!(glm_46.input, 0.6e-6);
        assert_eq!(glm_46.output, 2.2e-6);
        assert_eq!(glm_46.cache_create, Some(0.0));
        assert_eq!(glm_46.cache_read, Some(0.11e-6));

        let glm_45 = pricing.find("glm-4.5").unwrap();
        assert_eq!(glm_45.input, 0.6e-6);
        assert_eq!(glm_45.output, 2.2e-6);
        assert_eq!(glm_45.cache_create, Some(0.0));
        assert_eq!(glm_45.cache_read, Some(0.11e-6));

        let zai_glm_45 = pricing.find("zai/glm-4.5").unwrap();
        assert_eq!(zai_glm_45.input, 0.6e-6);
        assert_eq!(zai_glm_45.output, 2.2e-6);
        assert_eq!(zai_glm_45.cache_create, Some(0.0));
        assert_eq!(zai_glm_45.cache_read, Some(0.11e-6));
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
            cache_create: Some(0.0),
            cache_read: Some(0.03),

            ..Pricing::empty()
        };

        pricing.supplement_cache_rates("zai/glm-test-model", zeroed);

        let entry = pricing.find_exact("zai/glm-test-model").unwrap();
        assert_eq!(entry.cache_create, Some(0.25));
        assert_eq!(entry.cache_read, Some(0.03));
    }

    #[test]
    fn embedded_pricing_patches_z_ai_glm_entries_without_litellm_cache_rates() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        let glm_45_air = pricing.find("zai/glm-4.5-air").unwrap();
        assert_eq!(glm_45_air.input, 0.2e-6);
        assert_eq!(glm_45_air.output, 1.1e-6);
        assert_eq!(glm_45_air.cache_create, Some(0.0));
        assert_eq!(glm_45_air.cache_read, Some(0.03e-6));

        let glm_45_x = pricing.find("zai/glm-4.5-x").unwrap();
        assert_eq!(glm_45_x.input, 2.2e-6);
        assert_eq!(glm_45_x.output, 8.9e-6);
        assert_eq!(glm_45_x.cache_create, Some(0.0));
        assert_eq!(glm_45_x.cache_read, Some(0.45e-6));

        let glm_45v = pricing.find("zai/glm-4.5v").unwrap();
        assert_eq!(glm_45v.input, 0.6e-6);
        assert_eq!(glm_45v.output, 1.8e-6);
        assert_eq!(glm_45v.cache_create, Some(0.0));
        assert_eq!(glm_45v.cache_read, Some(0.11e-6));
    }

    #[test]
    fn keeps_absent_litellm_cache_rates_absent() {
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

        assert!(pricing.find("gpt-with-cache").unwrap().cache_read.is_some());
        assert!(
            !pricing
                .find("gpt-without-cache")
                .unwrap()
                .cache_read
                .is_some()
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
    fn loads_every_litellm_rate_field() {
        let mut pricing = PricingMap::default();
        let loaded = pricing.load_json(
            r#"{
                "gpt-every-rate": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_creation_input_token_cost": 0.00000125,
                    "cache_read_input_token_cost": 0.0000001,
                    "input_cost_per_token_above_200k_tokens": 0.000002,
                    "output_cost_per_token_above_200k_tokens": 0.000020,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.0000025,
                    "cache_read_input_token_cost_above_200k_tokens": 0.0000002,
                    "max_input_tokens": 123456,
                    "provider_specific_entry": { "fast": 1.5 }
                }
            }"#,
        );

        assert_eq!(loaded, 1);
        let entry = pricing.find("gpt-every-rate").unwrap();
        assert_eq!(entry.input, 1e-6);
        assert_eq!(entry.output, 10e-6);
        assert_eq!(entry.cache_create, Some(1.25e-6));
        assert_eq!(entry.cache_read, Some(0.1e-6));
        assert!(entry.cache_read.is_some());
        assert_eq!(entry.input_above_200k, Some(2e-6));
        assert_eq!(entry.output_above_200k, Some(20e-6));
        assert_eq!(entry.cache_create_above_200k, Some(2.5e-6));
        assert_eq!(entry.cache_read_above_200k, Some(0.2e-6));
        assert_eq!(entry.fast_multiplier, 1.5);
        assert_eq!(pricing.context_limit("gpt-every-rate"), Some(123456));
    }

    #[test]
    fn keeps_models_dev_fallback_disabled_for_embedded_and_offline_pricing() {
        crate::test_support::install_pricing_snapshots();
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
    fn tries_models_dev_pricing_once_per_process() {
        let cell = std::sync::OnceLock::new();
        let attempts = AtomicUsize::new(0);

        let failed = super::load_models_dev_pricing_once(&cell, |_| {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "temporary failure",
            ))
        });
        let later = super::load_models_dev_pricing_once(&cell, |load| {
            attempts.fetch_add(1, Ordering::Relaxed);
            load(r#"{"openai":{"models":{"gpt-late":{"cost":{"input":1.0,"output":2.0}}}}}"#);
            Ok(())
        });

        assert!(failed.is_none());
        assert!(later.is_none());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn keeps_the_models_dev_pricing_its_one_attempt_loaded() {
        crate::test_support::install_pricing_snapshots();
        let cell = std::sync::OnceLock::new();
        let loaded = super::load_models_dev_pricing_once(&cell, |load| {
            load(
                r#"{
                "openai": {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-first": {
                            "id": "gpt-first",
                            "name": "GPT First",
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
            }"#,
            );
            Ok(())
        })
        .expect("a usable models.dev body should load");

        let gpt_first = loaded
            .find_entry("gpt-first", Fuzzy::Allowed)
            .expect("the loaded body should price its model");
        assert_eq!(gpt_first.input, 0.000001);
        assert_eq!(gpt_first.output, 0.000002);
        assert_eq!(
            loaded.context_limit_entry("gpt-first", Fuzzy::Allowed),
            Some(42)
        );
        assert!(
            super::load_models_dev_pricing_once(&cell, |_| {
                Err(std::io::Error::other("must not fetch again"))
            })
            .is_some()
        );
    }

    #[test]
    fn loads_missing_models_dev_pricing_without_overriding_litellm() {
        crate::test_support::install_pricing_snapshots();
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
        assert_eq!(primary.cache_read, Some(0.1e-6));
        assert_eq!(pricing.context_limit("gpt-primary"), Some(123));
        assert!((fallback.input - 2e-6).abs() < f64::EPSILON);
        assert!((fallback.output - 8e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_create.unwrap() - 2.5e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_read.unwrap() - 0.2e-6).abs() < f64::EPSILON);
        assert!(fallback.cache_read.is_some());
        assert_eq!(fallback.input_above_200k, None);
        assert_eq!(fallback.output_above_200k, None);
        assert_eq!(fallback.fast_multiplier, 1.0);
        assert_eq!(pricing.context_limit("gpt-fallback"), Some(456));
        assert!((alias.input - 4e-6).abs() < f64::EPSILON);
        assert_eq!(pricing.context_limits.get("gpt-alias"), Some(&654));
    }

    #[test]
    fn applies_fast_multiplier_override_to_models_dev_fallback() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        let models_dev_json = r#"{
            "openai": {
                "id": "openai",
                "models": {
                    "gpt-6-astra": {
                        "id": "gpt-6-astra",
                        "cost": {
                            "input": 10.0,
                            "output": 50.0
                        }
                    }
                }
            }
        }"#;

        assert_eq!(
            pricing.load_models_dev_json_missing(models_dev_json),
            Some(1)
        );
        assert_eq!(
            pricing.find_exact("gpt-6-astra").unwrap().fast_multiplier,
            2.0
        );
        assert_eq!(
            pricing.find_exact("gpt-6-astra").unwrap().flex_multiplier,
            0.5
        );
    }

    #[test]
    fn live_models_dev_pricing_prefers_the_authoring_catalog_over_resellers() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        assert!(entry.cache_read.is_some());
        assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
        assert_eq!(
            pricing.context_limits.get("kimi-k2.6-nitro"),
            Some(&128_000)
        );
    }

    #[test]
    fn live_models_dev_pricing_keeps_authored_token_models_a_catalog_mislabels() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        assert!(entry.cache_read.is_some());
        assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_drops_the_context_limit_of_a_replaced_catalog() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        assert!((fallback.cache_create.unwrap() - 3.75e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_read.unwrap() - 0.3e-6).abs() < f64::EPSILON);
        assert_eq!(pricing.context_limit("claude-fallback"), Some(200000));
    }

    #[test]
    fn applies_speed_multipliers_to_qualified_flat_models_dev_models() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        let models_dev_json = r#"{
                "openai/gpt-6-astra": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "openai.gpt-5.4": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "global.openai.gpt-6.1-sol": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "global.openai.gpt-5.4-mini-2026-03-17": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "openai.gpt-5.4-mini-extra": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                }
            }"#;

        assert_eq!(
            pricing.load_models_dev_json_missing(models_dev_json),
            Some(5)
        );

        for (name, fast, flex) in [
            ("openai/gpt-6-astra", 2.0, 0.5),
            ("openai.gpt-5.4", 2.0, 0.5),
            ("global.openai.gpt-6.1-sol", 2.0, 0.5),
            ("global.openai.gpt-5.4-mini-2026-03-17", 1.0, 0.5),
            ("openai.gpt-5.4-mini-extra", 1.0, 1.0),
        ] {
            let model = pricing.find_exact(name).unwrap();
            assert_eq!(model.fast_multiplier, fast, "{name}");
            assert_eq!(model.flex_multiplier, flex, "{name}");
        }
    }

    #[test]
    fn embedded_models_dev_snapshot_is_parseable() {
        crate::test_support::install_pricing_snapshots();
        let mut map = PricingMap::default();
        assert!(
            map.load_models_dev_json_missing(build_time_models_dev_json())
                .is_some()
        );
    }

    #[test]
    fn offline_resolves_models_only_in_embedded_models_dev() {
        crate::test_support::install_pricing_snapshots();
        use ccusage_cli::PricingOverride;
        let offline = PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &PricingOverride)>(),
        );
        let mut primary = PricingMap::default();
        primary.load_json(build_time_pricing_json());
        primary.put_builtin_pricing();
        let model = embedded_models_dev_pricing()
            .entries
            .keys()
            .find(|model| primary.find(model).is_none())
            .expect(
                "the embedded models.dev snapshot should price a model the primary table misses",
            );
        assert!(offline.find(model).is_some());
        assert!(PricingMap::default().find(model).is_none());
    }

    #[test]
    fn offline_prices_new_anthropic_model_from_embedded_models_dev() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let gpt_55 = pricing.find("gpt-5.5").unwrap();

        assert_eq!(gpt_55.input, 5e-6);
        assert_eq!(gpt_55.output, 30e-6);
        assert_eq!(gpt_55.cache_read, Some(0.5e-6));
        assert!(gpt_55.cache_read.is_some());
        assert_eq!(gpt_55.fast_multiplier, 2.5);
        assert_eq!(pricing.context_limit("gpt-5.5"), Some(1_050_000));
    }

    #[test]
    fn embedded_pricing_preserves_gpt_5_6_generated_sources() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let mut litellm_snapshot = PricingMap::default();
        litellm_snapshot.load_json(build_time_pricing_json());
        let models_dev_snapshot = embedded_models_dev_pricing();

        // The updater owns the numeric rates, so this regression fence checks
        // that the merged map preserves its generated sources without freezing
        // prices or requiring every optional rate bucket to exist.
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let entry = pricing.find(model).unwrap();
            let litellm_source = litellm_snapshot.find_exact(model);
            let base_source = litellm_source
                .or_else(|| models_dev_snapshot.find_exact(model))
                .unwrap();
            let tier_source = litellm_source
                .filter(|source| {
                    source.input_above_200k.is_some()
                        || source.output_above_200k.is_some()
                        || source.cache_create_above_200k.is_some()
                        || source.cache_read_above_200k.is_some()
                })
                .or_else(|| models_dev_snapshot.find_exact(model))
                .unwrap();
            assert_eq!(entry.input, base_source.input, "{model}");
            assert_eq!(entry.output, base_source.output, "{model}");
            assert_eq!(entry.cache_create, base_source.cache_create, "{model}");
            assert_eq!(entry.cache_read, base_source.cache_read, "{model}");
            assert_eq!(
                entry.input_above_200k, tier_source.input_above_200k,
                "{model}"
            );
            assert_eq!(
                entry.output_above_200k, tier_source.output_above_200k,
                "{model}"
            );
            assert_eq!(
                entry.cache_create_above_200k, tier_source.cache_create_above_200k,
                "{model}"
            );
            assert_eq!(
                entry.cache_read_above_200k, tier_source.cache_read_above_200k,
                "{model}"
            );
            assert_eq!(
                entry.long_context_threshold, tier_source.long_context_threshold,
                "{model}"
            );
            assert_eq!(
                pricing.context_limit(model),
                litellm_snapshot
                    .context_limits
                    .get(model)
                    .copied()
                    .or_else(|| models_dev_snapshot.context_limits.get(model).copied())
                    .or_else(|| super::BUILTIN_PRICING
                        .iter()
                        .find(|entry| entry.model == model)
                        .and_then(|entry| entry.context_limit)),
                "{model}"
            );
        }
    }

    #[test]
    fn builtin_gpt_5_6_fallbacks_preserve_source_precedence() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004123,
                    "output_cost_per_token": 0.000020123,
                    "cache_creation_input_token_cost": 0.000005123,
                    "cache_read_input_token_cost": 0.0000004123,
                    "max_input_tokens": 654321
                }
            }"#,
        );
        let loaded = pricing.find_exact("gpt-5.6-sol").unwrap();
        let loaded_context_limit = pricing.context_limit("gpt-5.6-sol");

        let mut builtin = PricingMap::default();
        builtin.put_builtin_pricing();
        let builtin_rates = builtin.find_exact("gpt-5.6-sol").unwrap();
        assert_ne!(loaded.input, builtin_rates.input);
        assert_ne!(loaded.output, builtin_rates.output);

        pricing.put_builtin_pricing();
        let resolved = pricing.find_exact("gpt-5.6-sol").unwrap();
        assert_eq!(resolved.input, loaded.input);
        assert_eq!(resolved.output, loaded.output);
        assert_eq!(resolved.cache_create, loaded.cache_create);
        assert_eq!(resolved.cache_read, loaded.cache_read);
        assert_eq!(resolved.fast_multiplier, loaded.fast_multiplier);
        assert_eq!(pricing.context_limit("gpt-5.6-sol"), loaded_context_limit);

        let mut embedded_models_dev = PricingMap::default();
        embedded_models_dev
            .context_limits
            .insert("gpt-5.6-sol".to_string(), 777_777);
        let mut live_models_dev = PricingMap::default();
        live_models_dev
            .context_limits
            .insert("gpt-5.6-sol".to_string(), 888_888);
        let empty_models_dev = PricingMap::default();
        assert_eq!(
            layered(&pricing, Some(&embedded_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
            Some(654_321)
        );
        assert_eq!(
            layered(&builtin, Some(&embedded_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
            Some(888_888)
        );
        assert_eq!(
            layered(&builtin, Some(&embedded_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || None),
            Some(777_777)
        );
        assert_eq!(
            layered(&builtin, Some(&empty_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || None),
            Some(1_050_000)
        );
    }

    #[test]
    fn gpt_5_6_alias_resolves_to_sol_when_the_generic_entry_is_missing() {
        crate::test_support::install_pricing_snapshots();
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
    fn embedded_gpt_5_6_exact_entry_wins_over_the_sol_alias() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let exact = pricing.find_exact("gpt-5.6").unwrap();
        let resolved = pricing.find("gpt-5.6").unwrap();

        assert_eq!(resolved.input, exact.input);
        assert_eq!(resolved.output, exact.output);
        assert_eq!(resolved.cache_create, exact.cache_create);
        assert_eq!(resolved.cache_read, exact.cache_read);
        assert_eq!(resolved.input_above_200k, exact.input_above_200k);
        assert_eq!(resolved.output_above_200k, exact.output_above_200k);
        assert_eq!(
            resolved.long_context_threshold,
            exact.long_context_threshold
        );
        assert_eq!(
            pricing.context_limit("gpt-5.6"),
            pricing.context_limits.get("gpt-5.6").copied()
        );
    }

    #[test]
    fn embedded_pricing_fills_gpt_long_context_tier_rates() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        pricing.fill_long_context_rates_from_models_dev(embedded_models_dev_pricing());

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
        pricing.fill_long_context_rates_from_models_dev(embedded_models_dev_pricing());

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(gpt_55.input_above_200k, Some(12e-6));
        assert_eq!(gpt_55.long_context_threshold, None);
    }

    #[test]
    fn embedded_models_dev_carries_grok_long_context_tiers() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
    fn pricing_lookup_prefers_an_exact_alias_target_over_a_fuzzy_original_match() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "claude-opus-5-turbo",
            "sentinel-exact-target",
        )]);
        let mut pricing = PricingMap::default();
        assert_eq!(
            pricing.load_json(
                r#"{
                    "claude-opus-5": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    },
                    "sentinel-exact-target": {
                        "input_cost_per_token": 0.000009,
                        "output_cost_per_token": 0.000017,
                        "max_input_tokens": 765432
                    }
                }"#,
            ),
            2
        );

        let pricing = online(&pricing);
        let live_lookups = AtomicUsize::new(0);
        let live = || {
            live_lookups.fetch_add(1, Ordering::Relaxed);
            None
        };
        let resolved = pricing.find_with_live("claude-opus-5-turbo", live).unwrap();
        assert_eq!(resolved.input, 9e-6);
        assert_eq!(resolved.output, 17e-6);
        assert_eq!(
            pricing.context_limit_with_live("claude-opus-5-turbo", live),
            Some(765_432)
        );
        assert_eq!(live_lookups.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pricing_lookup_preserves_a_live_exact_only_builtin_alias_target() {
        let mut pricing = PricingMap::default();
        assert_eq!(
            pricing.load_json(
                r#"{
                    "gpt-5.6": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    }
                }"#,
            ),
            1
        );
        let mut models_dev = PricingMap::default();
        assert_eq!(
            models_dev.load_json(
                r#"{
                    "gpt-5.6-luna": {
                        "input_cost_per_token": 0.000009,
                        "output_cost_per_token": 0.000017,
                        "max_input_tokens": 765432
                    }
                }"#,
            ),
            1
        );
        models_dev.exact_only.insert("gpt-5.6-luna".to_string());

        let pricing = online(&pricing);
        let resolved = pricing
            .find_with_live("gpt-reserve", || Some(&models_dev))
            .unwrap();
        assert_eq!(resolved.input, 9e-6);
        assert_eq!(resolved.output, 17e-6);
        assert_eq!(
            pricing.context_limit_with_live("gpt-reserve", || Some(&models_dev)),
            Some(765_432)
        );
    }

    #[test]
    fn embedded_pricing_includes_codex_priority_multiplier() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        assert_eq!(pricing.find("gpt-5.6-sol").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.6-terra").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.6-luna").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
        assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-6-astra").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-6-astra").unwrap().flex_multiplier, 0.5);
    }

    #[test]
    fn embedded_pricing_includes_all_openai_flex_models() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        for model in [
            "gpt-6-astra",
            "gpt-6.1-sol",
            "gpt-6.1-terra",
            "gpt-6-luna",
            "gpt-6-sol",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "gpt-5.4-pro",
            "gpt-5.2",
            "gpt-5.1",
            "gpt-5",
            "gpt-5-mini",
            "gpt-5-nano",
            "o3",
            "o4-mini",
        ] {
            assert_eq!(pricing.find(model).unwrap().flex_multiplier, 0.5, "{model}");
        }
    }

    #[test]
    fn dated_openai_models_inherit_published_fast_multipliers() {
        let mut pricing = PricingMap::default();

        for (model, expected) in [
            ("gpt-6-astra-2026-09-24", 2.0),
            ("gpt-6.1-terra", 2.0),
            ("gpt-5.4-2026-03-05", 2.0),
            ("openai/gpt-5.6-sol-20261001", 2.0),
            ("gpt-5.6-terra-2026-10-01", 2.0),
            ("gpt-5.6-luna-2026-10-01", 2.0),
            ("global.openai.gpt-5.5-2026-04-23", 2.5),
            ("gpt-5.2-codex-2026-01-14", 1.0),
            ("gpt-5.4-unsupported", 1.0),
        ] {
            let json = serde_json::json!({
                model: {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                },
            })
            .to_string();
            assert_eq!(pricing.load_json(&json), 1);
            assert_eq!(
                pricing.find_exact(model).unwrap().fast_multiplier,
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn resolves_ultrafast_multiplier_with_the_standard_pricing_entry() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        for (model, expected) in [
            ("gpt-6.1-sol", 6.0),
            ("gpt-6-1-sol", 6.0),
            ("openai/gpt-6-1-sol", 6.0),
            ("openai@gpt-6-1-sol", 6.0),
            ("gpt-6-1-sol-2026-09-24", 6.0),
            ("gpt-6-astra", 6.0),
            ("gpt-6-1-terra", 1.0),
            ("gpt-6-sol", 1.0),
        ] {
            assert_eq!(
                pricing.find(model).unwrap().ultrafast_multiplier,
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn resolves_ultrafast_multiplier_through_model_aliases() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-sol", "gpt-6.1-sol"),
            ("gpt-6.1-terra", "gpt-6.1-sol"),
        ]);
        // Whether the embedded LiteLLM snapshot has an exact Terra entry depends
        // on the build environment, so the test supplies both entries itself.
        let mut pricing = PricingMap::default();
        let json = serde_json::json!({
            "gpt-6.1-sol": {
                "input_cost_per_token": 0.000002,
                "output_cost_per_token": 0.00001,
            },
            "gpt-6.1-terra": {
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000005,
            },
        })
        .to_string();
        assert_eq!(pricing.load_json(&json), 2);

        let private_sol = pricing.find("private-sol").unwrap();
        assert_eq!(private_sol.ultrafast_multiplier, 6.0);
        assert_eq!(private_sol.input, 2e-6);

        // An exact Terra entry wins over the alias, so its rates and multiplier stay Terra's.
        let terra = pricing.find("gpt-6.1-terra").unwrap();
        assert_eq!(terra.ultrafast_multiplier, 1.0);
        assert_eq!(terra.input, 1e-6);
    }

    #[test]
    fn keeps_published_speed_multipliers_for_user_price_overrides() {
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-sol", "gpt-6.1-sol"),
            ("private-dated-sol", "gpt-6.1-sol-2026-09-24"),
        ]);
        let override_value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(3e-6),
            ..Default::default()
        };
        let sol = PricingMap::load_embedded().find("gpt-6.1-sol").unwrap();

        // Each map holds one override, so no other override creates an exact entry to inherit from.
        for model in ["gpt-6.1-sol-2026-09-24", "private-dated-sol", "private-sol"] {
            let key = model.to_string();
            let pricing = PricingMap::load_with_overrides(true, false, [(&key, &override_value)]);
            let overridden = pricing.find(model).unwrap();
            assert_eq!(overridden.input, 3e-6, "{model}");
            assert_eq!(overridden.fast_multiplier, 2.0, "{model}");
            assert_eq!(overridden.ultrafast_multiplier, 6.0, "{model}");
            assert_eq!(overridden.flex_multiplier, 0.5, "{model}");
        }

        let key = "private-sol".to_string();
        let aliased = PricingMap::load_with_overrides(true, false, [(&key, &override_value)])
            .find(&key)
            .unwrap();
        assert_eq!(aliased.output, sol.output);
    }

    #[test]
    fn alias_cycle_overrides_build_on_the_snapshot_not_on_each_other() {
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("cycle-a", "cycle-b"),
            ("cycle-b", "cycle-a"),
            ("into-cycle", "cycle-b"),
        ]);
        let overrides = std::collections::BTreeMap::from([
            (
                "cycle-a".to_string(),
                ccusage_cli::PricingOverride {
                    input_cost_per_token: Some(4e-6),
                    ..Default::default()
                },
            ),
            (
                "cycle-b".to_string(),
                ccusage_cli::PricingOverride {
                    output_cost_per_token: Some(9e-6),
                    ..Default::default()
                },
            ),
            (
                "into-cycle".to_string(),
                ccusage_cli::PricingOverride {
                    cache_read_input_token_cost: Some(1e-7),
                    ..Default::default()
                },
            ),
        ]);
        let pricing = PricingMap::load_with_overrides(true, false, &overrides);

        let cycle_a = pricing.find_exact("cycle-a").unwrap();
        assert_eq!((cycle_a.input, cycle_a.output), (4e-6, 0.0));
        let cycle_b = pricing.find_exact("cycle-b").unwrap();
        assert_eq!((cycle_b.input, cycle_b.output), (0.0, 9e-6));
        let into_cycle = pricing.find_exact("into-cycle").unwrap();
        assert_eq!(
            (into_cycle.input, into_cycle.output, into_cycle.cache_read),
            (0.0, 9e-6, Some(1e-7))
        );
    }

    #[test]
    fn applies_target_overrides_before_aliases_that_sort_first() {
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("a-private-sol", "gpt-6.1-sol"),
            ("cycle-a", "cycle-b"),
            ("cycle-b", "cycle-a"),
            ("a-into-cycle", "cycle-a"),
        ]);
        let target_override = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(3e-6),
            output_cost_per_token: Some(7e-6),
            ..Default::default()
        };
        let alias_override = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(5e-6),
            ..Default::default()
        };
        let cycle_override = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(4e-6),
            output_cost_per_token: Some(9e-6),
            ..Default::default()
        };
        let overrides = std::collections::BTreeMap::from([
            ("a-private-sol".to_string(), alias_override.clone()),
            ("gpt-6.1-sol".to_string(), target_override),
            ("cycle-a".to_string(), cycle_override.clone()),
            ("cycle-b".to_string(), cycle_override),
            ("a-into-cycle".to_string(), alias_override),
        ]);
        let pricing = PricingMap::load_with_overrides(true, false, &overrides);

        let alias = pricing.find("a-private-sol").unwrap();
        assert_eq!(alias.input, 5e-6);
        assert_eq!(alias.output, 7e-6);
        assert_eq!(pricing.find("cycle-a").unwrap().input, 4e-6);
        assert_eq!(pricing.find("cycle-b").unwrap().input, 4e-6);
        let into_cycle = pricing.find("a-into-cycle").unwrap();
        assert_eq!(into_cycle.input, 5e-6);
        assert_eq!(into_cycle.output, 9e-6);
    }

    #[test]
    fn fills_flex_multiplier_only_for_supported_openai_model_names() {
        let mut pricing = PricingMap::default();

        for (model, expected) in [
            ("gpt-6-astra-2026-09-24", 0.5),
            ("gpt-6.1-sol-20260924", 0.5),
            ("gpt-6.1-terra", 0.5),
            ("openai/gpt-5.5", 0.5),
            ("openrouter/openai/gpt-5.4-2026-03-05", 0.5),
            ("azure/gpt-5.2", 0.5),
            ("openai/gpt-5-mini-2025-08-07", 0.5),
            ("o3-2025-04-16", 0.5),
            ("openai/o4-mini-2025-04-16", 0.5),
            ("gpt-5.6", 0.5),
            ("openai/gpt-reserve", 0.5),
            ("gpt-4.1", 1.0),
            ("gpt-5-pro", 1.0),
            ("gpt-5.2-pro", 1.0),
            ("gpt-5.1-codex", 1.0),
            ("gpt-5.2-codex", 1.0),
            ("gpt-5.3-codex", 1.0),
            ("gpt-5.4-mini-extra", 1.0),
            ("gpt-6-astra-unsupported", 1.0),
            ("o3-mini", 1.0),
            ("o3-pro", 1.0),
            ("openai/o4-mini-deep-research", 1.0),
            ("gpt-unknown", 1.0),
        ] {
            let json = serde_json::json!({
                model: {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                },
            })
            .to_string();
            assert_eq!(pricing.load_json(&json), 1);
            assert_eq!(
                pricing.find_exact(model).unwrap().flex_multiplier,
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn embedded_pricing_does_not_resolve_undated_codex_auto_review_model() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        assert!(pricing.find("codex-auto-review").is_none());
        assert!(pricing.context_limit("codex-auto-review").is_none());
    }

    #[test]
    fn embedded_pricing_resolves_codex_spark_short_model_alias() {
        crate::test_support::install_pricing_snapshots();
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
    fn embedded_pricing_resolves_codex_gpt_reserve_to_gpt_5_6_luna() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let reserve = pricing
            .find("gpt-reserve")
            .expect("gpt-reserve should resolve via model alias");
        let luna = pricing
            .find("gpt-5.6-luna")
            .expect("GPT-5.6 Luna pricing should exist");

        assert_eq!(reserve.input, luna.input);
        assert_eq!(reserve.output, luna.output);
        assert_eq!(reserve.cache_read, luna.cache_read);
        assert_eq!(reserve.fast_multiplier, luna.fast_multiplier);
        assert_eq!(
            long_context_split_threshold("gpt-reserve"),
            long_context_split_threshold("gpt-5.6-luna")
        );
    }

    #[test]
    fn model_suffix_matches_only_at_the_last_occurrence() {
        assert!(super::matches_model_suffix("claude-opus-4-6", "opus-4-6"));
        assert!(super::matches_model_suffix(
            "us-claude-opus-4-6-v1",
            "opus-4-6"
        ));
        assert!(!super::matches_model_suffix(
            "opus-4-6-x-opus-4-60",
            "opus-4-6"
        ));
        assert!(!super::matches_model_suffix("opus-4", "opus-4-6"));
        assert!(super::matches_model_suffix("modèle-opus", "opus"));
        assert!(super::matches_model_suffix("anything", ""));
    }

    #[test]
    fn embedded_pricing_includes_claude_fast_multiplier_for_provider_models() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
                cache_create: Some(6.25e-6),
                cache_read: Some(0.5e-6),

                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
                ultrafast_multiplier: 1.0,
                flex_multiplier: 1.0,
                ..Pricing::empty()
            },
        );
        pricing.entries.insert(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: Some(18.75e-6),
                cache_read: Some(1.5e-6),

                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
                ultrafast_multiplier: 1.0,
                flex_multiplier: 1.0,
                ..Pricing::empty()
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
                cache_create: Some(18.75e-6),
                cache_read: Some(1.5e-6),

                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
                ultrafast_multiplier: 1.0,
                flex_multiplier: 1.0,
                ..Pricing::empty()
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
        crate::test_support::install_pricing_snapshots();
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
                },
                "gpt-6-astra": {
                    "input_cost_per_token": 0.000010,
                    "output_cost_per_token": 0.000050,
                    "cache_read_input_token_cost": 0.000001
                }
            }"#,
        );

        assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
        assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-5.2-codex").unwrap().fast_multiplier, 1.0);
        assert_eq!(pricing.find("gpt-6-astra").unwrap().fast_multiplier, 2.0);
        assert_eq!(pricing.find("gpt-6-astra").unwrap().flex_multiplier, 0.5);
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
    fn embedded_build_time_pricing_is_filtered() {
        crate::test_support::install_pricing_snapshots();
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
                cache_create: Some(0.0),
                cache_read: Some(0.0),

                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
                ultrafast_multiplier: 1.0,
                flex_multiplier: 1.0,
                ..Pricing::empty()
            },
        );
        pricing.entries.insert(
            "claude-sonnet-4-20250514".to_string(),
            Pricing {
                input: 2.0,
                output: 0.0,
                cache_create: Some(0.0),
                cache_read: Some(0.0),

                input_above_200k: None,
                output_above_200k: None,
                cache_create_above_200k: None,
                cache_read_above_200k: None,
                long_context_threshold: None,
                fast_multiplier: 1.0,
                ultrafast_multiplier: 1.0,
                flex_multiplier: 1.0,
                ..Pricing::empty()
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
        fn user_override_detection_resolves_configured_model_aliases() {
            let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
                "private-model",
                "custom-model",
            )]);
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("custom-model", |o| {
                o.input_cost_per_token = Some(0.001);
            });
            pricing.apply_overrides(overrides.iter());

            assert!(pricing.has_user_override("custom-model"));
            assert!(pricing.has_user_override("private-model"));
            assert!(!pricing.has_user_override("unrelated-model"));
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
            assert_eq!(entry.cache_create, Some(3e-6));
            assert_eq!(entry.cache_read, Some(4e-7));
            assert!(entry.cache_read.is_some());
            assert_eq!(entry.fast_multiplier, 2.0);
            assert_eq!(pricing.context_limit("custom-model"), Some(123_456));
        }

        #[test]
        fn exact_override_wins_over_gpt_5_6_alias() {
            crate::test_support::install_pricing_snapshots();
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
                    cache_create: Some(30e-6),
                    cache_read: Some(40e-6),

                    input_above_200k: Some(15e-6),
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.5,
                    ultrafast_multiplier: 1.0,
                    flex_multiplier: 1.0,
                    ..Pricing::empty()
                },
            );

            let overrides = build_overrides("existing", |o| {
                o.input_cost_per_token = Some(99e-6);
                o.flex_multiplier = Some(0.5);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("existing").unwrap();
            assert_eq!(entry.input, 99e-6);
            assert_eq!(entry.output, 20e-6);
            assert_eq!(entry.cache_create, Some(30e-6));
            assert_eq!(entry.cache_read, Some(40e-6));
            assert!(entry.cache_read.is_some());
            assert_eq!(entry.input_above_200k, Some(15e-6));
            assert_eq!(entry.fast_multiplier, 1.5);
            assert_eq!(entry.flex_multiplier, 0.5);
        }

        #[test]
        fn override_without_cache_read_keeps_it_absent() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("new-model", |o| {
                o.input_cost_per_token = Some(1e-6);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("new-model").unwrap();
            assert_eq!(entry.cache_read, None);
        }

        #[test]
        fn override_with_zero_cache_read_preserves_zero() {
            let mut pricing = PricingMap::default();
            let overrides = build_overrides("new-model", |o| {
                o.cache_read_input_token_cost = Some(0.0);
            });

            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("new-model").unwrap();
            assert!(entry.cache_read.is_some());
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
        fn input_override_recomputes_absent_cache_rates_and_preserves_published_tiers() {
            let mut pricing = PricingMap::default();
            pricing.entries.insert(
                "claude-model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    cache_create: None,
                    cache_read: None,

                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: Some(4.6875e-6),
                    cache_read_above_200k: Some(3.75e-7),
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                    ultrafast_multiplier: 1.0,
                    flex_multiplier: 1.0,
                    ..Pricing::empty()
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
            assert_eq!(entry.cache_create, None);
            assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
            assert_eq!(entry.cache_read, None);
            assert!((entry.input * 0.1 - 2e-7).abs() < 1e-15);
            assert_eq!(entry.cache_create_above_200k, Some(4.6875e-6));
            assert_eq!(entry.cache_read_above_200k, Some(3.75e-7));
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
                    cache_create: Some(0.0),
                    cache_read: Some(0.0),
                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                    ultrafast_multiplier: 1.0,
                    flex_multiplier: 1.0,
                    ..Pricing::empty()
                },
            );

            let overrides = build_overrides("no-cache-model", |o| {
                o.input_cost_per_token = Some(2e-6);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("no-cache-model").unwrap();
            assert_eq!(entry.cache_create, Some(0.0));
            assert_eq!(entry.cache_read, Some(0.0));
        }

        #[test]
        fn explicit_cache_override_replaces_the_default() {
            let mut pricing = PricingMap::default();
            pricing.entries.insert(
                "model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    cache_create: None,
                    cache_read: None,

                    input_above_200k: None,
                    output_above_200k: None,
                    cache_create_above_200k: None,
                    cache_read_above_200k: None,
                    long_context_threshold: None,
                    fast_multiplier: 1.0,
                    ultrafast_multiplier: 1.0,
                    flex_multiplier: 1.0,
                    ..Pricing::empty()
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
            assert_eq!(entry.cache_read, Some(5e-7)); // explicit value, not 2e-7
            assert_eq!(entry.cache_create, None);
            assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
        }
    }
}
