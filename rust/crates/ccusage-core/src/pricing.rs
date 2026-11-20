use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

use ccusage_cli::PricingOverride;

use serde::Deserialize;
use serde_json::Value;

use crate::{
    MILLIS_PER_DAY, MILLIS_PER_HOUR, TimestampMs,
    fast::{FxHashMap, FxHashSet},
};

mod builtin_pricing;
mod cache;
use cache::{CachedHit, Candidate, HitKind, Name, NameCache, Pass, RequestCache, RequestHit};
mod data;
mod overrides;
use overrides::Overrides;
mod row;
pub use row::Source;
use row::{Entries, Row, Scope, cache_defaults, row_scope, schedule_for, vendor_id, vendor_start};

use builtin_pricing::BUILTIN_PRICING;

const FAST_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("fast-multiplier-overrides.json");
const FLEX_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("flex-multiplier-overrides.json");
const ULTRAFAST_MULTIPLIER_OVERRIDES_JSON: &str =
    include_str!("ultrafast-multiplier-overrides.json");

/// Compressed pricing snapshots supplied by the binary at startup.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedSnapshots {
    pub litellm_sources: &'static [u8],
    pub litellm_pricing: &'static [u8],
    pub models_dev_pricing: &'static [u8],
    pub models_dev_catalog_rules: &'static [u8],
}

static EMBEDDED_SNAPSHOTS: OnceLock<EmbeddedSnapshots> = OnceLock::new();

/// Installs the snapshots before the first pricing lookup. Later calls are ignored.
pub fn set_embedded_snapshots(snapshots: EmbeddedSnapshots) {
    let _ = EMBEDDED_SNAPSHOTS.set(snapshots);
}

fn embedded_snapshots() -> &'static EmbeddedSnapshots {
    EMBEDDED_SNAPSHOTS
        .get()
        .expect("embedded pricing snapshots must be installed with set_embedded_snapshots")
}

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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiteLlmSources {
    direct_providers: Vec<String>,
}

fn litellm_sources() -> Option<&'static LiteLlmSources> {
    static JSON: OnceLock<String> = OnceLock::new();
    static SOURCES: OnceLock<Option<LiteLlmSources>> = OnceLock::new();
    let snapshots = EMBEDDED_SNAPSHOTS.get()?;
    SOURCES
        .get_or_init(|| {
            match serde_json::from_str(inflate_snapshot(&JSON, snapshots.litellm_sources)) {
                Ok(sources) => Some(sources),
                Err(error) => {
                    eprintln!("WARN  Failed to parse embedded pricing source rules ({error}).");
                    None
                }
            }
        })
        .as_ref()
}

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODEL_DATE_SUFFIX_DIGITS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
    cache_create_1h: Option<f64>,
    pub long_context: Option<LongContext>,
    pub fast_multiplier: f64,
    pub ultrafast_multiplier: f64,
    pub flex_multiplier: f64,
    cache_defaults: &'static CacheDefaults,
    explicit_fields: u16,
}

pub const DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongContextMode {
    Marginal,
    WholeRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LongContextRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_create: Option<f64>,
    cache_create_1h: Option<f64>,
    pub cache_read: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongContext {
    pub(crate) threshold: u64,
    pub rates: LongContextRates,
    pub(crate) mode: LongContextMode,
}

impl LongContext {
    const fn empty_marginal() -> Self {
        Self {
            threshold: DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS,
            mode: LongContextMode::Marginal,
            rates: LongContextRates {
                input: None,
                output: None,
                cache_create: None,
                cache_create_1h: None,
                cache_read: None,
            },
        }
    }
}

impl LongContextRates {
    fn marginal(self) -> Option<LongContext> {
        [
            self.input,
            self.output,
            self.cache_create,
            self.cache_create_1h,
            self.cache_read,
        ]
        .iter()
        .any(Option::is_some)
        .then_some(LongContext {
            rates: self,
            ..LongContext::empty_marginal()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchKind {
    StrictExact,
    Exact,
    Fuzzy,
}

pub struct AgentModel<'a> {
    raw: &'a str,
    display: String,
}

impl<'a> AgentModel<'a> {
    pub fn new(agent: &str, raw: &'a str) -> Self {
        Self {
            raw,
            display: format!("[{agent}] {raw}"),
        }
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn into_display(self) -> String {
        self.display
    }

    pub fn candidates(&self) -> [(&str, MatchKind); 2] {
        [
            (self.display(), MatchKind::StrictExact),
            (self.raw, MatchKind::Fuzzy),
        ]
    }

    pub fn overriding_candidate(&self, pricing: Option<&PricingMap>) -> Option<(&str, MatchKind)> {
        let pricing = pricing?;
        [self.display(), self.raw]
            .into_iter()
            .find(|model| pricing.has_user_override(model))
            .map(|model| (model, MatchKind::Fuzzy))
    }
}

#[derive(Debug, PartialEq)]
struct CacheDefaults {
    write_x: f64,
    write_1h_x: f64,
    read_x: f64,
}

impl Pricing {
    pub fn long_context_split_threshold(self) -> u64 {
        self.long_context
            .map_or(DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS, |context| {
                context.threshold
            })
    }

    pub fn cache_creation_rate(self) -> f64 {
        self.cache_create
            .unwrap_or(self.input * self.cache_defaults.write_x)
    }

    pub(crate) fn one_hour_cache_creation_rate(self) -> f64 {
        self.cache_create_1h
            .unwrap_or(self.input * self.cache_defaults.write_1h_x)
    }

    pub(crate) fn one_hour_cache_creation_rate_above_200k(self) -> Option<f64> {
        let rates = self.long_context?.rates;
        rates.cache_create_1h.or_else(|| {
            rates
                .input
                .map(|input| input * self.cache_defaults.write_1h_x)
        })
    }

    pub fn cache_read_rate_at(self, input_rate: f64) -> f64 {
        self.cache_read
            .unwrap_or(input_rate * self.cache_defaults.read_x)
    }

    const fn empty() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_create: None,
            cache_read: None,
            cache_create_1h: None,
            long_context: None,
            fast_multiplier: 1.0,
            ultrafast_multiplier: 1.0,
            flex_multiplier: 1.0,
            cache_defaults: &data::WRITES_SURCHARGED,
            explicit_fields: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ScheduledRates {
    input: f64,
    output: f64,
    cache_create: f64,
    cache_read: f64,
}

#[derive(Debug)]
struct PricingSchedule {
    cutoff_ms: i64,
    peak_weekdays: std::ops::RangeInclusive<u32>,
    peak_hours: &'static [std::ops::Range<i64>],
    before: ScheduledRates,
    off_peak: ScheduledRates,
    peak: ScheduledRates,
}

impl PricingSchedule {
    fn rates_at(&self, timestamp: TimestampMs) -> ScheduledRates {
        if timestamp.as_millis() < self.cutoff_ms {
            return self.before;
        }
        let weekday =
            ccusage_cli::weekday_from_epoch_days(timestamp.as_millis().div_euclid(MILLIS_PER_DAY));
        let hour = timestamp.as_millis().rem_euclid(MILLIS_PER_DAY) / MILLIS_PER_HOUR;
        if self.peak_weekdays.contains(&weekday)
            && self.peak_hours.iter().any(|window| window.contains(&hour))
        {
            self.peak
        } else {
            self.off_peak
        }
    }
}

struct OverrideFields;

impl OverrideFields {
    const INPUT: u16 = 1;
    const OUTPUT: u16 = 2;
    const CACHE_CREATE: u16 = 4;
    const CACHE_READ: u16 = 8;
    const LONG_INPUT: u16 = 16;
    const LONG_OUTPUT: u16 = 32;
    const LONG_CACHE_CREATE: u16 = 64;
    const LONG_CACHE_READ: u16 = 128;
    const CACHE_CREATE_1H: u16 = 256;
    const FAST: u16 = 512;
    const FLEX: u16 = 1024;
}

impl Pricing {
    fn at(mut self, schedule: &PricingSchedule, timestamp: TimestampMs) -> Self {
        let explicit = (self.explicit_fields != 0).then_some(self);
        let rates = schedule.rates_at(timestamp);
        let mut long_context = self.long_context.unwrap_or(LongContext::empty_marginal());
        macro_rules! scheduled {
            ($field:ident, $rate:expr, $bit:expr) => {
                self.$field = match explicit {
                    Some(entry) if entry.explicit_fields & $bit != 0 => entry.$field,
                    _ => $rate,
                };
            };
        }
        scheduled!(input, rates.input, OverrideFields::INPUT);
        scheduled!(output, rates.output, OverrideFields::OUTPUT);
        scheduled!(
            cache_create,
            Some(rates.cache_create),
            OverrideFields::CACHE_CREATE
        );
        scheduled!(
            cache_read,
            Some(rates.cache_read),
            OverrideFields::CACHE_READ
        );
        long_context.rates.input = match explicit {
            Some(entry) if entry.explicit_fields & OverrideFields::LONG_INPUT != 0 => {
                entry.long_context.and_then(|context| context.rates.input)
            }
            _ => Some(rates.input),
        };
        long_context.rates.output = match explicit {
            Some(entry) if entry.explicit_fields & OverrideFields::LONG_OUTPUT != 0 => {
                entry.long_context.and_then(|context| context.rates.output)
            }
            _ => Some(rates.output),
        };
        long_context.rates.cache_create = match explicit {
            Some(entry) if entry.explicit_fields & OverrideFields::LONG_CACHE_CREATE != 0 => entry
                .long_context
                .and_then(|context| context.rates.cache_create),
            _ => Some(rates.cache_create),
        };
        long_context.rates.cache_read = match explicit {
            Some(entry) if entry.explicit_fields & OverrideFields::LONG_CACHE_READ != 0 => entry
                .long_context
                .and_then(|context| context.rates.cache_read),
            _ => Some(rates.cache_read),
        };
        scheduled!(
            cache_create_1h,
            self.cache_create_1h,
            OverrideFields::CACHE_CREATE_1H
        );
        scheduled!(fast_multiplier, self.fast_multiplier, OverrideFields::FAST);
        scheduled!(flex_multiplier, self.flex_multiplier, OverrideFields::FLEX);
        self.long_context = Some(long_context);
        self
    }

    fn apply_override(&mut self, value: &PricingOverride) {
        self.explicit_fields = 0;
        macro_rules! rate {
            ($source:ident, $field:ident, $wrap:expr, $bit:expr) => {
                if let Some(value) = value.$source {
                    self.$field = $wrap(value);
                    self.explicit_fields |= $bit;
                }
            };
        }
        rate!(
            input_cost_per_token,
            input,
            std::convert::identity,
            OverrideFields::INPUT
        );
        rate!(
            output_cost_per_token,
            output,
            std::convert::identity,
            OverrideFields::OUTPUT
        );
        rate!(
            cache_creation_input_token_cost,
            cache_create,
            Some,
            OverrideFields::CACHE_CREATE
        );
        rate!(
            cache_read_input_token_cost,
            cache_read,
            Some,
            OverrideFields::CACHE_READ
        );
        let mut context = self.long_context.unwrap_or(LongContext::empty_marginal());
        let mut changed = false;
        macro_rules! long_rate {
            ($source:ident, $field:ident, $bit:expr) => {
                if let Some(value) = value.$source {
                    context.rates.$field = Some(value);
                    self.explicit_fields |= $bit;
                    changed = true;
                }
            };
        }
        long_rate!(
            input_cost_per_token_above_200k_tokens,
            input,
            OverrideFields::LONG_INPUT
        );
        long_rate!(
            output_cost_per_token_above_200k_tokens,
            output,
            OverrideFields::LONG_OUTPUT
        );
        long_rate!(
            cache_creation_input_token_cost_above_200k_tokens,
            cache_create,
            OverrideFields::LONG_CACHE_CREATE
        );
        long_rate!(
            cache_read_input_token_cost_above_200k_tokens,
            cache_read,
            OverrideFields::LONG_CACHE_READ
        );
        if changed {
            self.long_context = Some(context);
        }
        rate!(
            cache_creation_input_token_cost_above_1hr,
            cache_create_1h,
            Some,
            OverrideFields::CACHE_CREATE_1H
        );
        rate!(
            fast_multiplier,
            fast_multiplier,
            std::convert::identity,
            OverrideFields::FAST
        );
        rate!(
            flex_multiplier,
            flex_multiplier,
            std::convert::identity,
            OverrideFields::FLEX
        );
    }
}
#[derive(Clone, Copy)]
enum Lookup {
    Pricing,
    Context,
}

impl Lookup {
    fn includes(self, row: &Row) -> bool {
        match self {
            Self::Pricing => true,
            Self::Context => row.context_limit.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "resolver-trace")]
pub enum ResolutionStep {
    ReportedOverride,
    AuthoritativeIdentity,
    AuthoritativeSuffix,
    ModelsDevIdentity,
    AuthoritativeFuzzy,
    ModelsDevFuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "resolver-trace")]
pub struct Explanation<'a> {
    pub candidate: &'a str,
    pub step: ResolutionStep,
    pub row_key: &'a str,
    pub source: Source,
}

#[derive(Debug, Clone, Copy)]
pub struct PricingHandle {
    rates: Pricing,
    schedule: Option<&'static PricingSchedule>,
}

impl PricingHandle {
    fn new(row: &Row, hit: CachedHit) -> Self {
        Self {
            rates: row.pricing.rates(),
            schedule: row.schedule.filter(|_| hit.scheduled()),
        }
    }

    pub fn is_time_dependent(&self) -> bool {
        self.schedule.is_some()
    }

    pub fn with_rates_at<T>(
        &self,
        timestamp: Option<TimestampMs>,
        calculate: impl FnOnce(&Pricing) -> T,
    ) -> T {
        match (timestamp, self.schedule) {
            (Some(timestamp), Some(schedule)) => calculate(&self.rates.at(schedule, timestamp)),
            _ => calculate(&self.rates),
        }
    }
}

#[derive(Clone, Copy)]
struct RowMatch<'a> {
    key: &'a str,
    row: &'a Row,
    id: usize,
}

impl<'a> RowMatch<'a> {
    fn new(id: usize, row: &'a Row) -> Self {
        Self {
            key: &row.key,
            row,
            id,
        }
    }
}

#[derive(Debug)]
pub struct PricingMap {
    rows: Arc<PricingRows>,
    overrides: Arc<Overrides>,
    override_shadows: OnceLock<FxHashSet<usize>>,
    models_dev: Option<Arc<PricingRows>>,
    live_models_dev: OnceLock<Option<Arc<PricingRows>>>,
    enable_models_dev_fallback: bool,
    find_cache: OnceLock<NameCache>,
    requests: OnceLock<RequestCache>,
}

#[derive(Clone, Debug, Default)]
struct PricingRows {
    entries: Entries,
    indexes: OnceLock<RowIndexes>,
    exact_only_by_identity: OnceLock<FxHashMap<Box<str>, Option<usize>>>,
    normalized_keys: OnceLock<Box<[Option<Box<str>>]>>,
}

type IdentityIndex = FxHashMap<Box<str>, smallvec::SmallVec<[usize; 1]>>;

#[derive(Clone, Debug, Default)]
struct RowIndexes {
    by_identity: IdentityIndex,
    by_vendor_id: IdentityIndex,
    bare_hosted: IdentityIndex,
}

impl PricingRows {
    fn literal(&self, key: &str, lookup: Lookup) -> Option<RowMatch<'_>> {
        let id = self.entries.id(key)?;
        let found = self.at(id)?;
        lookup.includes(found.row).then_some(found)
    }

    fn at(&self, id: usize) -> Option<RowMatch<'_>> {
        self.entries.rows.get(id).map(|row| RowMatch::new(id, row))
    }

    fn matches(&self) -> impl Iterator<Item = RowMatch<'_>> {
        self.entries
            .rows
            .iter()
            .enumerate()
            .map(|(id, row)| RowMatch::new(id, row))
    }

    fn normalized_matches(&self) -> impl Iterator<Item = (RowMatch<'_>, &str)> {
        let keys = self.normalized_keys.get_or_init(|| {
            self.matches()
                .map(|found| match normalized_pricing_key(found.key) {
                    Cow::Borrowed(_) => None,
                    Cow::Owned(key) => Some(key.into_boxed_str()),
                })
                .collect()
        });
        self.matches()
            .zip(keys)
            .map(|(found, key)| (found, key.as_deref().unwrap_or(found.key)))
    }

    fn indexed_row(&self, keys: &[usize], lookup: Lookup) -> Option<RowMatch<'_>> {
        keys.iter()
            .filter_map(|id| self.at(*id))
            .filter(|found| lookup.includes(found.row))
            .min_by_key(|found| found.key)
    }

    fn authoritative<'a>(
        &'a self,
        name: Name<'_>,
        hosted: bool,
        visible: impl Fn(RowMatch<'a>) -> Option<RowMatch<'a>>,
    ) -> Option<RowMatch<'a>> {
        let literal = self.literal(name.raw(), Lookup::Pricing);
        if let Some(found) = literal
            && !found.row.is_bare_hosted(found.key)
            && let Some(found) = visible(found)
        {
            return Some(found);
        }
        let identity = name.identity();
        let indexes = self.indexes();
        let matches = |index: &'a IdentityIndex| {
            index.get(identity).into_iter().flatten().filter_map(|key| {
                let raw = self.at(*key)?;
                Some((raw.key, visible(raw)?))
            })
        };
        let by_key = |index: &'a IdentityIndex| {
            matches(index)
                .min_by_key(|(key, _)| *key)
                .map(|(_, found)| found)
        };
        by_key(&indexes.by_identity)
            .or_else(|| {
                let mut rows = matches(&indexes.by_vendor_id);
                let (key, first) = rows.next()?;
                let mut best = (key, first);
                for (key, found) in rows {
                    if found.row.pricing != first.row.pricing {
                        return None;
                    }
                    if key < best.0 {
                        best = (key, found);
                    }
                }
                Some(best.1)
            })
            .or_else(|| {
                if hosted {
                    literal
                        .filter(|found| found.row.is_bare_hosted(found.key))
                        .and_then(&visible)
                        .or_else(|| by_key(&indexes.bare_hosted))
                } else {
                    None
                }
            })
    }

    fn exact(&self, name: Name<'_>, lookup: Lookup) -> Option<RowMatch<'_>> {
        self.literal(name.raw(), lookup).or_else(|| {
            let indexes = self.indexes();
            self.indexed_row(indexes.by_identity.get(name.identity())?, lookup)
        })
    }

    fn indexes(&self) -> &RowIndexes {
        self.indexes.get_or_init(|| {
            let mut indexes = RowIndexes::default();
            for found in self.matches() {
                let row = found.row;
                let key = found.key;
                let index = if row.is_bare_hosted(key) {
                    &mut indexes.bare_hosted
                } else {
                    &mut indexes.by_identity
                };
                index
                    .entry(normalized_pricing_key(key).into_owned().into_boxed_str())
                    .or_default()
                    .push(found.id);
                let vendor_id = if row.scope == Scope::Direct && row.is_free() {
                    Some(last_segment(key))
                } else {
                    row.vendor_id()
                };
                if let Some(id) = vendor_id.filter(|id| *id != key) {
                    indexes
                        .by_vendor_id
                        .entry(normalized_pricing_key(id).into_owned().into_boxed_str())
                        .or_default()
                        .push(found.id);
                }
            }
            indexes
        })
    }

    fn exact_only_by_identity(&self) -> &FxHashMap<Box<str>, Option<usize>> {
        self.exact_only_by_identity.get_or_init(|| {
            let mut index = FxHashMap::default();
            for found in self.matches().filter(|found| found.row.exact_only) {
                index
                    .entry(
                        normalized_pricing_key(found.key)
                            .into_owned()
                            .into_boxed_str(),
                    )
                    .and_modify(|value| *value = None)
                    .or_insert(Some(found.id));
            }
            index
        })
    }

    fn contains_exact_only_identity(&self, key: &str, identity: &str) -> bool {
        self.entries.get(key).is_some_and(|row| row.exact_only)
            || self.exact_only_by_identity().contains_key(identity)
    }

    fn exact_only_id(&self, key: &str) -> Option<&str> {
        let id = self
            .exact_only_by_identity()
            .get(normalized_pricing_key(key).as_ref())?;
        self.at((*id)?).map(|found| found.key)
    }
}

impl Default for PricingMap {
    fn default() -> Self {
        Self {
            rows: Arc::default(),
            overrides: Arc::default(),
            models_dev: None,
            live_models_dev: OnceLock::new(),
            enable_models_dev_fallback: false,
            find_cache: OnceLock::new(),
            requests: OnceLock::new(),
            override_shadows: OnceLock::new(),
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
    litellm_provider: Option<String>,
    provider_specific_entry: Option<ProviderSpecificEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderSpecificEntry {
    fast: Option<f64>,
    flex: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    id: Option<String>,
    models: FxHashMap<String, ModelsDevModel>,
}

// NOTE(2026-08-15, #1541): The live models.dev API lists each model under each serving catalogue and omits authorship and authored modalities. The source catalogue supplies this information for rate selection.
#[derive(Debug, Deserialize)]
struct ModelsDevCatalogRules {
    owners: FxHashSet<String>,
    platforms: FxHashSet<String>,
    #[serde(rename = "authoredModelIds")]
    authored_model_ids: FxHashSet<String>,
    #[serde(rename = "assetPricedModelIds")]
    asset_priced_model_ids: FxHashSet<String>,
    #[serde(skip)]
    normalized_authored_model_ids: Vec<String>,
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

    fn is_exact_only(&self, source_model_id: &str, pricing_key: &str) -> bool {
        self.is_tier_variant_of_authored_model(source_model_id)
            || is_unversioned_models_dev_model_id(pricing_key)
    }

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

    fn is_token_priced(
        &self,
        source_model_id: &str,
        modalities: Option<&ModelsDevModalities>,
    ) -> bool {
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
        if let Some(multiplier) = pricing_alias(model).and_then(|alias| self.exact.get(alias)) {
            return Some(*multiplier);
        }
        model
            .split(['/', ':'])
            .flat_map(|part| {
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

fn base_pricing(offline: bool) -> &'static OnceLock<PricingMap> {
    static ONLINE: OnceLock<PricingMap> = OnceLock::new();
    static OFFLINE: OnceLock<PricingMap> = OnceLock::new();
    if offline { &OFFLINE } else { &ONLINE }
}

impl PricingMap {
    pub fn load_embedded() -> Self {
        Self::build_embedded()
    }

    fn rows_mut(&mut self) -> &mut PricingRows {
        let rows = Arc::make_mut(&mut self.rows);
        rows.indexes.take();
        rows.normalized_keys.take();
        rows.exact_only_by_identity.take();
        rows
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
        self.models_dev = Some(Arc::clone(&embedded_models_dev.rows));
        self.clear_find_cache();
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

    /// Shares the base prices with a report that supplies its own user overrides.
    /// Start from a base map without user overrides.
    pub fn with_user_overrides<'a, I>(&self, overrides: I) -> Self
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        let mut map = Self {
            rows: Arc::clone(&self.rows),
            overrides: Arc::clone(&self.overrides),
            models_dev: self.models_dev.clone(),
            live_models_dev: self.live_models_dev.clone(),
            enable_models_dev_fallback: self.enable_models_dev_fallback,
            find_cache: OnceLock::new(),
            requests: OnceLock::new(),
            override_shadows: OnceLock::new(),
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
        let Some(sources) = litellm_sources() else {
            return 0;
        };
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
            let provider = pricing.litellm_provider.as_deref();
            let (scope, vendor_id) = if let Some(provider) = provider {
                row_scope(&model, provider, &sources.direct_providers)
            } else {
                (Scope::Hosted, None)
            };
            if scope == Scope::Hosted && !model.contains('/') && input == 0.0 && output == 0.0 {
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
            let published_speed = provider_specific.or(speed.for_model(&model));
            let defaults = cache_defaults(&model, pricing.litellm_provider.as_deref());
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
            row.pricing = published_speed
                .applied_to(Pricing {
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
                    .marginal(),
                    cache_defaults: defaults,
                    ..Pricing::empty()
                })
                .into();
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
        })
    }

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
        let mut sources: Vec<_> = models.into_iter().collect();
        sources.sort_by(|(left, _), (right, _)| left.cmp(right));
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
            let input = input / 1_000_000.0;
            let output = output / 1_000_000.0;

            self.rows_mut().entries.insert(
                model_id.clone(),
                Row {
                    pricing: speed
                        .for_model(&model_id)
                        .applied_to(Pricing {
                            input,
                            output,
                            cache_create: cost.cache_write.map(|value| value / 1_000_000.0),
                            cache_read: cost.cache_read.map(|value| value / 1_000_000.0),
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
                            cache_defaults: cache_defaults(&model_id, None),
                            ..Pricing::empty()
                        })
                        .into(),
                    exact_only,
                    context_limit,
                    key: Arc::default(),
                    source: Source::ModelsDev,
                    scope: Scope::Direct,
                    vendor_start: None,
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

    pub fn find(&self, model: &str) -> Option<Pricing> {
        let (_, found, _) = self.resolve_row_with(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(found.row.pricing.rates())
    }

    #[cfg(feature = "resolver-trace")]
    pub fn explain<'a>(
        &'a self,
        reported_model: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
    ) -> Option<Explanation<'a>> {
        let (candidate, found, hit) = self.resolve_row_with(
            reported_model,
            candidates,
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(Explanation {
            candidate,
            step: hit.step()?,
            row_key: found.key,
            source: found.row.source,
        })
    }

    pub fn resolve_at<'a>(
        &self,
        reported_model: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
        timestamp: Option<TimestampMs>,
    ) -> Option<(&'a str, Pricing)> {
        let (model, found, hit) = self.resolve_row_with(
            reported_model,
            candidates,
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        let handle = PricingHandle::new(found.row, hit);
        Some((model, handle.with_rates_at(timestamp, |rates| *rates)))
    }

    pub fn find_handle(&self, model: &str) -> Option<PricingHandle> {
        let (_, found, hit) = self.resolve_row_with(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            Lookup::Pricing,
            models_dev_pricing,
        )?;
        Some(PricingHandle::new(found.row, hit))
    }

    fn available_models_dev(&self) -> Option<&PricingRows> {
        self.live_models_dev
            .get()
            .and_then(Option::as_deref)
            .or(self.models_dev.as_deref())
    }

    fn select_models_dev<'a>(
        &self,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> Option<&PricingRows> {
        if self.enable_models_dev_fallback {
            self.live_models_dev
                .get_or_init(|| live().map(|source| Arc::clone(&source.rows)));
        }
        self.available_models_dev()
    }

    fn authoritative<'a>(
        &'a self,
        model: Name<'_>,
        lookup: Lookup,
        hosted: bool,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
        visible: impl Fn(&Row) -> bool,
    ) -> Option<RowMatch<'a>> {
        let project = |found: RowMatch<'a>| {
            (lookup.includes(found.row) && visible(found.row)).then_some(found)
        };
        if self.overrides.is_empty() {
            return self.rows.authoritative(model, hosted, project);
        }
        self.overrides
            .id_identity(model.identity())
            .and_then(|id| self.resolve_override(id, models_dev, false))
            .and_then(project)
            .or_else(|| {
                self.rows.authoritative(model, hosted, |found| {
                    let found = match self.overrides.shadow(found) {
                        Some(id) => self.resolve_override(id, models_dev, false)?,
                        None => found,
                    };
                    project(found)
                })
            })
    }

    fn resolve_row_with<'a, 'live>(
        &self,
        reported: Option<&'a str>,
        candidates: &[(&'a str, MatchKind)],
        lookup: Lookup,
        live: impl FnOnce() -> Option<&'live Self>,
    ) -> Option<(&'a str, RowMatch<'_>, CachedHit)> {
        let search = || {
            let live = std::cell::Cell::new(Some(live));
            self.search_request(reported, candidates, lookup, &|| {
                live.take().and_then(|load| load())
            })
        };
        let hit = match lookup {
            Lookup::Pricing => self
                .requests
                .get_or_init(RequestCache::default)
                .get_or_init(reported, candidates, search),
            Lookup::Context => search(),
        }?;
        let model = match hit.candidate {
            Some(index) => candidates.get(index)?.0,
            None => reported?,
        };
        self.cached_row(model, hit.row)
    }

    #[inline(never)]
    fn search_request<'live>(
        &self,
        reported: Option<&str>,
        candidates: &[(&str, MatchKind)],
        lookup: Lookup,
        live: &dyn Fn() -> Option<&'live Self>,
    ) -> Option<RequestHit> {
        let models_dev = || self.select_models_dev(live);
        let cache = self.find_cache.get_or_init(NameCache::default);
        let matching = |name, pass| {
            cache.get_or_init(name, lookup, pass, |slot| {
                self.search_pass(name, slot, lookup, pass, &models_dev)
            })
        };
        if !self.overrides.is_empty()
            && let Some(model) = reported
            && let Some(row) = matching(model, Pass::Label)
        {
            return Some(RequestHit {
                candidate: None,
                row,
            });
        }
        for pass in [Pass::S2, Pass::S34, Pass::S3f, Pass::S56] {
            if matches!((lookup, pass), (Lookup::Context, Pass::S3f)) {
                continue;
            }
            for (index, &(model, kind)) in candidates.iter().enumerate() {
                if pass.allows(kind)
                    && let Some(row) = matching(model, pass)
                {
                    return Some(RequestHit {
                        candidate: Some(index),
                        row,
                    });
                }
            }
        }
        None
    }

    fn search_pass<'a>(
        &'a self,
        model: &str,
        slot: &Candidate<'_>,
        lookup: Lookup,
        pass: Pass,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        match pass {
            Pass::Label => {
                let id = self.overrides.label_id(model, slot)?;
                let found = self.resolve_override(id, models_dev, false)?;
                lookup
                    .includes(found.row)
                    .then(|| CachedHit::new(found, HitKind::ReportedOverride))
            }
            Pass::S2 => slot
                .names(model, |name| {
                    self.authoritative(name, lookup, true, models_dev, |_| true)
                })
                .map(|found| CachedHit::new(found, HitKind::AuthoritativeIdentity)),
            Pass::S34 | Pass::S3f => {
                let free = matches!(pass, Pass::S3f);
                if free && matches!(lookup, Lookup::Context) {
                    return None;
                }
                slot.names(model, |name| {
                    name.suffixes().find_map(|suffix| {
                        self.authoritative(suffix, lookup, false, models_dev, |row| {
                            if free {
                                row.scope == Scope::Direct && row.is_free()
                            } else {
                                !row.is_free()
                                    || (matches!(lookup, Lookup::Context)
                                        && row.scope == Scope::Direct)
                            }
                        })
                    })
                })
                .map(|found| CachedHit::new(found, HitKind::AuthoritativeSuffix))
                .or_else(|| {
                    if free {
                        return None;
                    }
                    let rows = models_dev()?;
                    slot.names(model, |name| rows.exact(name, lookup))
                        .map(|found| CachedHit::new(found, HitKind::ModelsDevIdentity))
                })
            }
            Pass::S56 => {
                let secondary = models_dev();
                if let Some(rows) = secondary
                    && slot
                        .names(model, |name| {
                            rows.contains_exact_only_identity(name.raw(), name.identity())
                                .then_some(())
                        })
                        .is_some()
                {
                    return None;
                }
                slot.names(model, |name| {
                    self.search_fuzzy(name, lookup, secondary, models_dev)
                })
            }
        }
    }

    fn search_fuzzy<'a>(
        &'a self,
        name: Name<'_>,
        lookup: Lookup,
        secondary: Option<&'a PricingRows>,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> Option<CachedHit> {
        let normalized = name.identity();
        let base = self.rows.normalized_matches();
        let primary = if self.overrides.is_empty() {
            Self::fuzzy(
                base.map(|(found, identity)| (found.key, identity, found)),
                name.raw(),
                normalized,
                lookup,
            )
        } else {
            let shadowed = self.override_shadows.get_or_init(|| {
                self.rows
                    .matches()
                    .filter_map(|found| self.overrides.shadow(found))
                    .collect()
            });
            let matches =
                |key, identity| pricing_key_matches(key, identity, name.raw(), normalized);
            let resolve = |id| self.resolve_override(id, models_dev, true);
            let base = base.filter_map(|(raw, identity)| {
                let found = match self.overrides.shadow(raw) {
                    Some(id) if matches(raw.key, identity) => resolve(id)?,
                    Some(_) => return None,
                    None => raw,
                };
                Some((raw.key, identity, found))
            });
            let overrides =
                self.overrides
                    .names()
                    .enumerate()
                    .filter_map(|(id, (key, identity))| {
                        if shadowed.contains(&id) || !matches(key, identity) {
                            return None;
                        }
                        Some((key, identity, resolve(id)?))
                    });
            Self::fuzzy(base.chain(overrides), name.raw(), normalized, lookup)
        };
        primary
            .map(|found| CachedHit::new(found, HitKind::AuthoritativeFuzzy))
            .or_else(|| {
                Self::fuzzy(
                    secondary?
                        .normalized_matches()
                        .map(|(found, identity)| (found.key, identity, found)),
                    name.raw(),
                    normalized,
                    lookup,
                )
                .map(|found| CachedHit::new(found, HitKind::ModelsDevFuzzy))
            })
    }

    fn cached_row<'a>(
        &self,
        name: &'a str,
        hit: CachedHit,
    ) -> Option<(&'a str, RowMatch<'_>, CachedHit)> {
        let row = if hit.overridden() {
            self.overrides.resolved(hit.id())
        } else if hit.is_models_dev() {
            self.available_models_dev()?.at(hit.id())
        } else {
            self.rows.at(hit.id())
        }?;
        Some((name, row, hit))
    }

    fn fuzzy<'a>(
        rows: impl Iterator<Item = (&'a str, &'a str, RowMatch<'a>)>,
        name: &str,
        normalized: &str,
        lookup: Lookup,
    ) -> Option<RowMatch<'a>> {
        let segment = last_segment(normalized);
        let pool = rows.filter(|(key, identity, found)| {
            !found.row.exact_only
                && lookup.includes(found.row)
                && (!found.row.is_free()
                    || (matches!(lookup, Lookup::Context) && found.row.scope == Scope::Direct))
                && pricing_key_matches(key, identity, name, normalized)
        });
        let mut best = None;
        let mut best_not_longer = None;
        let mut equal_segment_seen = false;
        for (key, identity, found) in pool {
            let row_segment = last_segment(identity);
            let rank = (key.len(), std::cmp::Reverse(key));
            if best.is_none_or(|(previous, _)| rank > previous) {
                best = Some((rank, found));
            }
            if row_segment.len() <= segment.len() {
                if best_not_longer.is_none_or(|(previous, _)| rank > previous) {
                    best_not_longer = Some((rank, found));
                }
                equal_segment_seen |= row_segment == segment;
            }
        }
        let winner = if equal_segment_seen {
            best_not_longer
        } else {
            best
        };
        winner.map(|(_, found)| found)
    }

    fn has_user_override(&self, model: &str) -> bool {
        self.overrides
            .label_id(model, &Candidate::default())
            .is_some()
    }

    pub fn context_limit(&self, model: &str) -> Option<u64> {
        self.resolve_context_limit(model, MatchKind::Fuzzy, models_dev_pricing)
    }

    fn resolve_context_limit<'live>(
        &self,
        model: &str,
        kind: MatchKind,
        live: impl FnOnce() -> Option<&'live Self>,
    ) -> Option<u64> {
        let candidates = [(model, kind)];
        let live = std::cell::Cell::new(Some(live));
        let limit = |lookup| {
            self.resolve_row_with(Some(model), &candidates, lookup, || {
                live.take().and_then(|load| load())
            })
            .and_then(|(_, found, _)| found.row.context_limit)
        };
        limit(Lookup::Pricing).or_else(|| limit(Lookup::Context))
    }

    fn clear_find_cache(&mut self) {
        self.find_cache.take();
        self.requests.take();
        self.override_shadows.take();
    }

    fn fill_long_context_rates_from_models_dev(&mut self, tiers: &Self) {
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
            let Some(mut context) = tiers
                .long_context_pricing(model)
                .and_then(|source| source.long_context)
            else {
                continue;
            };
            context.rates.cache_create_1h = previous.cache_create_1h;
            pricing.long_context = Some(Arc::new(context));
        }
        self.clear_find_cache();
    }

    fn long_context_pricing(&self, model: &str) -> Option<Pricing> {
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
                    .map(|row| row.pricing.rates())
            })
    }

    fn supplement_pricing(
        &mut self,
        model: &str,
        pricing: Pricing,
        context_limit: Option<u64>,
        provider: &str,
    ) {
        let pricing = SpeedMultipliers::get().for_model(model).applied_to(pricing);
        let Some(row) = self
            .rows_mut()
            .entries
            .get_or_insert(model.to_string(), || Row {
                pricing: pricing.into(),
                context_limit,
                source: Source::Curated,
                scope: Scope::Direct,
                vendor_start: vendor_start(model, vendor_id(model, provider)),
                schedule: schedule_for(vendor_id(model, provider)),
                ..Row::default()
            })
        else {
            return;
        };
        row.context_limit = row.context_limit.or(context_limit);
        row.pricing.cache_read = row.pricing.cache_read.or(pricing.cache_read);
        row.pricing.cache_create = row.pricing.cache_create.or(pricing.cache_create);
        self.clear_find_cache();
    }

    fn put_builtin_pricing(&mut self) {
        for supplement in BUILTIN_PRICING {
            self.supplement_pricing(
                supplement.model,
                supplement.pricing,
                supplement.context_limit,
                supplement.provider,
            );
        }
    }
}

struct Supplement {
    provider: &'static str,
    model: &'static str,
    pricing: Pricing,
    context_limit: Option<u64>,
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

fn with_names<T>(model: &str, mut choose: impl FnMut(&str) -> Option<T>) -> Option<T> {
    choose(model)
        .or_else(|| pricing_alias(model).and_then(&mut choose))
        .or_else(|| {
            let alias = crate::model_aliases::resolve_model_name(model);
            if alias == model {
                return None;
            }
            choose(alias.as_ref()).or_else(|| pricing_alias(alias.as_ref()).and_then(choose))
        })
}

fn last_segment(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(_, segment)| segment)
}

fn pricing_key_matches(
    candidate: &str,
    normalized_candidate: &str,
    model: &str,
    normalized_model: &str,
) -> bool {
    names_same_model((model, model), (candidate, candidate))
        || names_same_model((normalized_model, model), (normalized_candidate, candidate))
}

// NOTE(2026-11-20, #2236, #2237): Either id may carry only a host or provider qualifier in front of the other; a word in front, as in `chat-gpt-5.4` for `gpt-5.4`, makes it another model. The qualifier is read from the raw id because the identity spells `openai.` as `openai-`.
fn names_same_model((model, raw_model): (&str, &str), (key, raw_key): (&str, &str)) -> bool {
    if model.len() >= key.len() {
        contains_pricing_key(model, raw_model, key)
    } else {
        contains_pricing_key(key, raw_key, model)
    }
}

fn contains_pricing_key(value: &str, raw_value: &str, key: &str) -> bool {
    value.match_indices(key).any(|(index, _)| {
        let before = index
            .checked_sub(1)
            .and_then(|before| raw_value.as_bytes().get(before))
            .copied();
        before.is_none_or(is_qualifier_end) && is_price_neutral_suffix(&value[index + key.len()..])
    })
}

fn is_qualifier_end(byte: u8) -> bool {
    !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_')
}

fn is_price_neutral_suffix(suffix: &str) -> bool {
    let mut rest = suffix;
    while let Some(&separator) = rest.as_bytes().first() {
        if separator == b'[' {
            let Some(end) = rest.find(']') else {
                return false;
            };
            rest = &rest[end + 1..];
            continue;
        }
        if !matches!(separator, b'-' | b'.' | b'_' | b':' | b'@') {
            return false;
        }
        let word_len = rest[1..]
            .bytes()
            .take_while(u8::is_ascii_alphanumeric)
            .count();
        let (word, after) = rest[1..].split_at(word_len);
        rest = if let Some(after) = date_suffix_end(word, after) {
            after
        } else if let Some(after) = deployment_revision_end(word, after) {
            after
        } else if let Some(after) = price_neutral_word_end(&rest[1..]) {
            after
        } else {
            return false;
        };
    }
    true
}

fn price_neutral_word_end(rest: &str) -> Option<&str> {
    data::PRICE_NEUTRAL_SUFFIX_WORDS.iter().find_map(|word| {
        let after = rest.strip_prefix(word)?;
        (!after
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric()))
        .then_some(after)
    })
}

fn date_suffix_end<'a>(word: &str, after: &'a str) -> Option<&'a str> {
    if is_digits(word, MODEL_DATE_SUFFIX_DIGITS) {
        return Some(after);
    }
    let month = after.strip_prefix('-')?.get(..2)?;
    let day = after.get(3..)?.strip_prefix('-')?.get(..2)?;
    let rest = &after[6..];
    (is_digits(word, 4)
        && is_digits(month, 2)
        && is_digits(day, 2)
        && !rest
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric()))
    .then_some(rest)
}

fn deployment_revision_end<'a>(word: &str, after: &'a str) -> Option<&'a str> {
    let revision = word.strip_prefix('v')?;
    let minor = after.strip_prefix(':')?;
    let minor_len = minor.bytes().take_while(u8::is_ascii_digit).count();
    (is_digits(revision, revision.len().max(1)) && minor_len > 0).then(|| &minor[minor_len..])
}

fn is_digits(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_pricing_key_separator(byte: u8) -> bool {
    matches!(byte, b'.' | b'@')
}

fn dash_pricing_key_separators(chars: impl Iterator<Item = char>) -> String {
    chars
        .map(|char| if matches!(char, '.' | '@') { '-' } else { char })
        .collect()
}

fn normalized_pricing_key(value: &str) -> Cow<'_, str> {
    if value.bytes().any(is_pricing_key_separator) {
        Cow::Owned(dash_pricing_key_separators(value.chars()))
    } else {
        Cow::Borrowed(value)
    }
}

#[derive(Clone, Copy)]
struct ModelsDevLongContextRates {
    threshold: u64,
    input: Option<f64>,
    output: Option<f64>,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
}

fn model_without_date_suffix(model: &str) -> &str {
    let bytes = model.as_bytes();
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
    if model.len() > 9 {
        let suffix = &bytes[model.len() - 9..];
        if suffix[0] == b'-' && suffix[1..].iter().all(u8::is_ascii_digit) {
            return &model[..model.len() - 9];
        }
    }
    model
}

fn pricing_alias(model: &str) -> Option<&'static str> {
    let fold = |byte| {
        if is_pricing_key_separator(byte) {
            b'-'
        } else {
            byte
        }
    };
    data::ALIASES.iter().find_map(|(key, alias)| {
        (key.len() == model.len() && key.bytes().map(fold).eq(model.bytes().map(fold)))
            .then_some(*alias)
    })
}

fn matches_model_suffix(part: &str, base: &str) -> bool {
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

/// Installs the HTTP client used to refresh pricing. Without it, only the embedded snapshots are available.
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
    mod scenarios;

    use super::{
        Lookup, MatchKind, Pricing, PricingMap, Row, build_time_models_dev_json,
        build_time_pricing_json, embedded_models_dev_pricing, model_without_date_suffix,
    };
    use ccusage_test_support::fs_fixture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn direct_litellm_row_beats_the_bare_hosted_spelling() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "deepseek/deepseek-v3.2": {
                "litellm_provider": "deepseek",
                "input_cost_per_token": 0.00000028,
                "output_cost_per_token": 0.00000040
            },
            "deepseek.v3.2": {
                "litellm_provider": "bedrock_converse",
                "input_cost_per_token": 0.00000062,
                "output_cost_per_token": 0.00000185
            }
        }"#,
        );
        for model in [
            "deepseek-v3-2",
            "deepseek-v3.2",
            "deepseek.v3.2",
            "deepseek/deepseek-v3.2",
        ] {
            let price = pricing.find(model).unwrap();
            assert_eq!(price.input, 0.28e-6, "{model}");
            assert_eq!(price.output, 0.40e-6, "{model}");
        }
    }

    impl PricingMap {
        pub(crate) fn find_at(
            &self,
            model: &str,
            timestamp: super::TimestampMs,
        ) -> Option<Pricing> {
            self.resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], Some(timestamp))
                .map(|(_, rates)| rates)
        }

        pub(crate) fn load_models_dev_json_for_tests(&mut self, json: &str) -> Option<usize> {
            self.load_models_dev_json_missing(json)
        }

        fn insert_test_pricing(&mut self, key: String, pricing: Pricing) {
            let row = self
                .rows_mut()
                .entries
                .get_or_insert(key, Row::default)
                .unwrap();
            row.scope = super::Scope::Direct;
            row.pricing = pricing.into();
        }

        fn insert_test_limit(&mut self, key: &str, limit: u64) {
            let row = self
                .rows_mut()
                .entries
                .get_or_insert(key.to_string(), Row::default)
                .unwrap();
            row.scope = super::Scope::Direct;
            row.context_limit = Some(limit);
        }

        fn mark_test_exact_only(&mut self, key: &str) {
            self.rows_mut()
                .entries
                .get_or_insert(key.to_string(), Row::default)
                .unwrap()
                .exact_only = true;
        }

        fn find_entry(&self, model: &str, kind: MatchKind) -> Option<Pricing> {
            self.resolve_at(Some(model), &[(model, kind)], None)
                .map(|(_, rates)| rates)
        }

        fn context_limit_entry(&self, model: &str, kind: MatchKind) -> Option<u64> {
            self.resolve_context_limit(model, kind, || None)
        }

        fn find_with_live<'a>(
            &self,
            model: &str,
            live: impl FnOnce() -> Option<&'a Self>,
        ) -> Option<Pricing> {
            self.resolve_row_with(
                Some(model),
                &[(model, MatchKind::Fuzzy)],
                Lookup::Pricing,
                live,
            )
            .map(|(_, found, _)| found.row.pricing.rates())
        }

        fn context_limit_with_live<'a>(
            &self,
            model: &str,
            live: impl FnOnce() -> Option<&'a Self>,
        ) -> Option<u64> {
            self.resolve_context_limit(model, MatchKind::Fuzzy, live)
        }

        fn with_live_layer<'a>(
            &self,
            model: &str,
            lookup: Lookup,
            live: impl FnOnce() -> Option<&'a Self>,
        ) -> &Self {
            self.resolve_row_with(Some(model), &[(model, MatchKind::Fuzzy)], lookup, live);
            self
        }

        fn len(&self) -> usize {
            let mut keys = rustc_hash::FxHashSet::default();
            keys.extend(self.rows.matches().map(|found| found.key));
            if let Some(models_dev) = self.available_models_dev() {
                keys.extend(models_dev.matches().map(|found| found.key));
            }
            keys.len()
        }

        fn models_dev_fallback_enabled(&self) -> bool {
            self.enable_models_dev_fallback
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
    fn all_match_kinds_prefer_the_same_id_over_longer_primary_keys() {
        crate::test_support::install_pricing_snapshots();
        use super::MatchKind;
        let mut primary = PricingMap::default();
        primary.load_json(
            r#"{
            "sample-1.5-pro": {"input_cost_per_token": 9, "output_cost_per_token": 9},
            "sample-1.5": {"input_cost_per_token": 1, "output_cost_per_token": 1}
        }"#,
        );
        let mut embedded = PricingMap::default();
        embedded.load_json(
            r#"{
            "o1": {"input_cost_per_token": 15, "output_cost_per_token": 60}
        }"#,
        );
        primary.load_json(
            r#"{"azure/o1-mini": {"input_cost_per_token": 1.21, "output_cost_per_token": 4.84}}"#,
        );
        primary.complete_layers(&embedded);
        for kind in [MatchKind::StrictExact, MatchKind::Exact, MatchKind::Fuzzy] {
            let model = if kind == MatchKind::StrictExact {
                "sample-1.5"
            } else {
                "sample-1-5"
            };
            assert_eq!(
                primary
                    .resolve_at(None, &[(model, kind)], None)
                    .unwrap()
                    .1
                    .input,
                1.0
            );
        }
        for kind in [MatchKind::Exact, MatchKind::Fuzzy] {
            assert_eq!(
                primary
                    .resolve_at(None, &[("o1", kind)], None)
                    .unwrap()
                    .1
                    .input,
                15.0
            );
        }
    }

    #[test]
    fn strict_candidates_only_admit_literal_primary_keys() {
        crate::test_support::install_pricing_snapshots();
        use super::MatchKind;
        let mut pricing = PricingMap::default();
        let mut embedded = PricingMap::default();
        embedded.load_json(
            r#"{
            "reseller/model-1.5": {"input_cost_per_token": 8, "output_cost_per_token": 9},
            "model-1.5": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
        );
        pricing.complete_layers(&embedded);
        let candidates = [
            ("reseller/model-1.5", MatchKind::StrictExact),
            ("model-1.5", MatchKind::Fuzzy),
        ];
        let (matched, rates) = pricing.resolve_at(None, &candidates, None).unwrap();
        assert_eq!(matched, "model-1.5");
        assert_eq!(rates.input, 1.0);
    }

    #[test]
    fn same_identity_in_the_fallback_beats_a_distinct_vendor_model() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{
            "deepseek-v3.2": {"litellm_provider":"deepseek","input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#);
        let mut embedded = PricingMap::default();
        embedded.load_json(
            r#"{
            "reseller/deepseek-v3.2-speciale": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
        );
        pricing.complete_layers(&embedded);
        assert_eq!(
            pricing
                .find("reseller/deepseek-v3.2-speciale")
                .unwrap()
                .input,
            8.0
        );
    }

    #[test]
    fn direct_pricing_keeps_the_fallback_row_for_distinct_variants() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "deepseek-v4-flash": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
        );
        let mut embedded = PricingMap::default();
        embedded.load_json(
            r#"{
            "reseller/deepseek-v4-flash-0731": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
        );
        pricing.complete_layers(&embedded);
        assert_eq!(pricing.find("deepseek-v4-flash").unwrap().input, 1.0);
        assert_eq!(
            pricing
                .find("reseller/deepseek-v4-flash-0731")
                .unwrap()
                .input,
            8.0
        );
    }

    #[test]
    fn normalized_model_id_keeps_provider_qualification() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        let mut embedded = PricingMap::default();
        embedded.load_json(
            r#"{
            "deepseek-v3-1": {"input_cost_per_token": 1, "output_cost_per_token": 2},
            "deepseek/deepseek-v3.1": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
        );
        pricing.complete_layers(&embedded);
        assert_eq!(pricing.find("deepseek/deepseek-v3-1").unwrap().input, 8.0);
    }

    #[test]
    fn bare_hosted_rows_price_each_separator_spelling_when_no_direct_row_exists() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{
            "deepseek.v3.2": {"litellm_provider":"bedrock_converse","input_cost_per_token": 0.62, "output_cost_per_token": 1.85},
            "free-hosted": {"litellm_provider":"bedrock","input_cost_per_token": 0, "output_cost_per_token": 0}
        }"#);
        for model in ["deepseek.v3.2", "deepseek-v3-2", "deepseek-v3.2"] {
            let price = pricing.find(model).unwrap();
            assert_eq!(price.input, 0.62, "{model}");
            assert_eq!(price.output, 1.85, "{model}");
        }
        assert!(pricing.find("free-hosted").is_none());
    }

    #[test]
    fn override_answers_the_model_id_behind_a_label_path() {
        crate::test_support::install_pricing_snapshots();
        let embedded = PricingMap::load_embedded();
        let mut live = PricingMap::load_embedded();
        live.refresh_litellm(|load| {
            load(r#"{
                "xai/grok-4.3": {"litellm_provider":"xai","input_cost_per_token":1.25e-6,"output_cost_per_token":2.5e-6},
                "gemini/gemini-2.5-pro": {"litellm_provider":"gemini","input_cost_per_token":1.25e-6,"output_cost_per_token":1e-5},
                "hostx/acme-1": {"litellm_provider":"hostx","input_cost_per_token":3e-6,"output_cost_per_token":4e-6}
            }"#);
            Ok(())
        });
        let override_at = |rate| ccusage_cli::PricingOverride {
            input_cost_per_token: Some(rate),
            output_cost_per_token: Some(rate),
            ..Default::default()
        };
        let overrides = ["grok-4.3", "gemini-2.5-pro", "acme-1"]
            .map(|key| (key.to_owned(), override_at(50e-6)));
        let specific = ("hostx/acme-1".to_owned(), override_at(7e-6));
        for base in [&embedded, &live] {
            let pricing = base.with_user_overrides(overrides.iter().map(|(key, rate)| (key, rate)));
            for label in [
                "xai/grok-4.3",
                "openrouter/xai/grok-4.3",
                "gemini/gemini-2.5-pro",
                "hostx/acme-1",
            ] {
                let candidates = [(label, MatchKind::Fuzzy)];
                let found = pricing.explain(Some(label), &candidates).unwrap();
                assert_eq!(
                    found.step,
                    super::ResolutionStep::ReportedOverride,
                    "{label}"
                );
                let rate = pricing.find(label).unwrap();
                assert_eq!((rate.input, rate.output), (50e-6, 50e-6), "{label}");
                assert!(pricing.has_user_override(label), "{label}");
            }
            assert!(!pricing.has_user_override("hostx/acme-1-flash"));
            let pricing = base.with_user_overrides(
                overrides
                    .iter()
                    .chain([&specific])
                    .map(|(key, rate)| (key, rate)),
            );
            let rate = pricing.find("hostx/acme-1").unwrap();
            assert_eq!((rate.input, rate.output), (7e-6, 7e-6));
        }
    }

    #[test]
    fn free_hosted_rows_answer_only_their_qualified_identity() {
        crate::test_support::install_pricing_snapshots();
        let mut table = PricingMap::default();
        assert_eq!(table.load_json(r#"{
            "host/free-1.5": {"litellm_provider":"host","input_cost_per_token":0,"output_cost_per_token":0,"max_input_tokens":123},
            "bare-free": {"litellm_provider":"host","input_cost_per_token":0,"output_cost_per_token":0},
            "unnamed-free": {"input_cost_per_token":0,"output_cost_per_token":0}
        }"#), 1);
        for name in ["host/free-1.5", "host/free-1-5"] {
            let (model, rate) = table
                .resolve_at(Some(name), &[(name, MatchKind::Fuzzy)], None)
                .unwrap();
            assert_eq!(model, name);
            assert_eq!((rate.input, rate.output), (0.0, 0.0));
            assert_eq!(table.context_limit(name), Some(123));
        }
        for name in [
            "bare-free",
            "unnamed-free",
            "free-1.5",
            "nest/host/free-1.5",
            "host/free-1.5-extra",
        ] {
            assert!(table.find(name).is_none(), "{name}");
            assert_eq!(table.context_limit(name), None, "{name}");
        }
    }

    #[test]
    fn direct_providers_cover_every_curated_vendor() {
        crate::test_support::install_pricing_snapshots();
        let providers = &super::litellm_sources().unwrap().direct_providers;
        for model in super::BUILTIN_PRICING {
            assert!(
                providers.iter().any(|provider| provider == model.provider),
                "{}",
                model.provider
            );
        }
    }

    #[test]
    fn live_direct_provider_row_answers_the_unqualified_model_id() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::load_embedded();
        pricing.refresh_litellm(|load| {
            assert!(load(r#"{
                "xai/grok-code-fast-1": {"litellm_provider":"xai","input_cost_per_token":1e-6,"output_cost_per_token":2e-6}
            }"#));
            Ok(())
        });
        let model = "grok-code-fast-1";
        let found = pricing
            .explain(Some(model), &[(model, MatchKind::Exact)])
            .unwrap();
        assert_eq!(
            (found.step, found.row_key),
            (
                super::ResolutionStep::AuthoritativeIdentity,
                "xai/grok-code-fast-1"
            )
        );
        let rate = pricing.find(model).unwrap();
        assert_eq!((rate.input, rate.output), (1e-6, 2e-6));
    }

    #[test]
    fn builtin_alias_applies_to_a_separator_respelling() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (canonical, respelling) in [
            ("gpt-5.3-spark", "gpt-5-3-spark"),
            ("gpt-5.6", "gpt-5-6"),
            ("gpt-reserve", "gpt@reserve"),
        ] {
            let expected = pricing.find(canonical).unwrap();
            let actual = pricing.find(respelling).unwrap();
            assert_eq!(
                (actual.input, actual.output),
                (expected.input, expected.output),
                "{respelling}"
            );
            assert_eq!(
                pricing.context_limit(respelling),
                pricing.context_limit(canonical),
                "{respelling}"
            );
        }
        let spark = pricing.find("gpt-5-3-spark").unwrap();
        assert_eq!((spark.input, spark.output), (1.75e-6, 14e-6));
        assert_eq!(pricing.context_limit("gpt-5-3-spark"), Some(128_000));
    }

    #[test]
    fn override_competes_in_the_fuzzy_pool_under_the_shadowed_key() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        assert_eq!(primary.load_json(r#"{
            "openai/acme-9": {"litellm_provider":"openai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "acme-9-low": {"litellm_provider":"host","input_cost_per_token":2e-6,"output_cost_per_token":2e-6}
        }"#), 2);
        let key = "acme-9".to_owned();
        let value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(50e-6),
            output_cost_per_token: Some(50e-6),
            ..Default::default()
        };
        let pricing = primary.with_user_overrides([(&key, &value)]);
        let label = "openai/acme-9-low-high";
        let candidates = [(label, MatchKind::Fuzzy)];
        let found = pricing.explain(Some(label), &candidates).unwrap();
        assert_eq!(
            (found.step, found.row_key),
            (super::ResolutionStep::AuthoritativeFuzzy, "acme-9")
        );
        let rate = pricing.find(label).unwrap();
        assert_eq!((rate.input, rate.output), (50e-6, 50e-6));
    }

    #[test]
    fn fuzzy_keeps_a_shorter_id_row_and_drops_longer_ones() {
        crate::test_support::install_pricing_snapshots();
        let mut secondary = PricingMap::default();
        assert_eq!(
            secondary.load_models_dev_json_for_tests(
                r#"{
            "hostaaaaa/m-1":{"cost":{"input":1,"output":1}},
            "m-1-20260101":{"cost":{"input":2,"output":2}},
            "hostc/m-2":{"cost":{"input":2,"output":2}},
            "hostc/long/m-2-20260101":{"cost":{"input":3,"output":3}}
        }"#
            ),
            Some(4)
        );
        let pricing = layered(&PricingMap::default(), Some(&secondary));
        for (model, row, rate) in [
            ("hostaaaaa/m-1-20260101", "hostaaaaa/m-1", 1e-6),
            ("m-2", "hostc/m-2", 2e-6),
        ] {
            let candidates = [(model, MatchKind::Fuzzy)];
            let found = pricing.explain(Some(model), &candidates).unwrap();
            assert_eq!(
                (found.step, found.row_key),
                (super::ResolutionStep::ModelsDevFuzzy, row),
                "{model}"
            );
            let found = pricing.find(model).unwrap();
            assert_eq!((found.input, found.output), (rate, rate), "{model}");
        }
    }

    #[test]
    fn free_vendor_row_behind_a_path_yields_to_the_ids_own_listing() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        use super::ResolutionStep::{
            AuthoritativeIdentity, AuthoritativeSuffix, ModelsDevIdentity,
        };
        let model = "novita/glm-4.7-flash";
        let candidates = [
            ("openrouter/novita/glm-4.7-flash", MatchKind::Exact),
            (model, MatchKind::Fuzzy),
        ];
        let found = pricing.explain(Some(model), &candidates).unwrap();
        assert_eq!((found.step, found.row_key), (ModelsDevIdentity, model));
        let (_, rates) = pricing.resolve_at(Some(model), &candidates, None).unwrap();
        assert!((rates.input - 0.0707e-6).abs() < 1e-15);
        assert!((rates.output - 0.404e-6).abs() < 1e-15);

        for (model, input, output, step, row) in [
            (
                "novita/glm-4.7-flash",
                0.0707e-6,
                0.404e-6,
                ModelsDevIdentity,
                "novita/glm-4.7-flash",
            ),
            (
                "z-ai/glm-4.7-flash",
                0.13225e-6,
                0.529e-6,
                ModelsDevIdentity,
                "z-ai/glm-4.7-flash",
            ),
            (
                "@cf/zai-org/glm-4.7-flash",
                0.0605e-6,
                0.4e-6,
                ModelsDevIdentity,
                "@cf/zai-org/glm-4.7-flash",
            ),
            (
                "openrouter/z-ai/glm-4.7-flash",
                0.0,
                0.0,
                AuthoritativeSuffix,
                "zai/glm-4.7-flash",
            ),
            (
                "kyma/glm-4.7-flash",
                0.0,
                0.0,
                AuthoritativeSuffix,
                "zai/glm-4.7-flash",
            ),
            (
                "glm-4.7-flash",
                0.0,
                0.0,
                AuthoritativeIdentity,
                "zai/glm-4.7-flash",
            ),
            (
                "zai/glm-4.7-flash",
                0.0,
                0.0,
                AuthoritativeIdentity,
                "zai/glm-4.7-flash",
            ),
        ] {
            for kind in [MatchKind::Exact, MatchKind::Fuzzy] {
                let candidates = [(model, kind)];
                let (_, rate) = pricing.resolve_at(Some(model), &candidates, None).unwrap();
                let found = pricing.explain(Some(model), &candidates).unwrap();
                assert_eq!((found.step, found.row_key), (step, row), "{model} {kind:?}");
                assert!(
                    (rate.input - input).abs() < 1e-15,
                    "{model} {kind:?}: {}",
                    rate.input
                );
                assert!(
                    (rate.output - output).abs() < 1e-15,
                    "{model} {kind:?}: {}",
                    rate.output
                );
            }
        }
    }

    #[test]
    fn provider_candidates_cannot_bypass_a_reported_override() {
        crate::test_support::install_pricing_snapshots();
        use super::MatchKind;
        let mut primary = PricingMap::default();
        primary.load_json(
            r#"{
            "vendor/sample-4.7": {"input_cost_per_token": 2, "output_cost_per_token": 3}
        }"#,
        );
        let override_id = "sample.4-7".to_string();
        let rates = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(5.0),
            output_cost_per_token: Some(6.0),
            ..ccusage_cli::PricingOverride::default()
        };
        let pricing = primary.with_user_overrides([(&override_id, &rates)]);
        let (_, entry) = pricing
            .resolve_at(
                Some("sample-4.7"),
                &[
                    ("vendor/sample-4.7", MatchKind::Exact),
                    ("sample-4.7", MatchKind::Fuzzy),
                ],
                None,
            )
            .unwrap();
        assert_eq!((entry.input, entry.output), (5.0, 6.0));
    }

    #[test]
    fn live_catalogue_is_selected_once_after_authoritative_misses() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
        assert_eq!(pricing.find("sample-20260101").unwrap().input, 1.0);
        assert!(pricing.find("other-model").is_none());
        let mut live = PricingMap::default();
        live.load_json(
            r#"{
            "sample-20260101": {"input_cost_per_token": 3, "output_cost_per_token": 4},
            "other-model": {"input_cost_per_token": 5, "output_cost_per_token": 6}
        }"#,
        );
        live.mark_test_exact_only("sample-20260101");
        let pricing = online(&pricing);
        let attempts = AtomicUsize::new(0);
        let fetch = || {
            attempts.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        assert_eq!(
            pricing
                .with_live_layer("sample", super::Lookup::Pricing, fetch)
                .resolve_at(None, &[("sample", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .input,
            1.0
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
        let merged = pricing.with_live_layer("trigger", super::Lookup::Pricing, fetch);
        assert_eq!(
            merged
                .resolve_at(None, &[("sample-20260101", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .input,
            3.0
        );
        assert_eq!(pricing.find("sample-20260101").unwrap().input, 3.0);
        assert_eq!(pricing.find("other-model").unwrap().input, 5.0);
        pricing.with_live_layer("trigger-again", super::Lookup::Pricing, fetch);
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_map_derived_after_the_live_merge_does_not_merge_it_again() {
        crate::test_support::install_pricing_snapshots();
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"md-only-1": { "cost": { "input": 3000000, "output": 4000000 } }}"#,
        );
        let base = layered(&PricingMap::default(), None);
        assert_eq!(
            base.find_with_live("md-only-1", || Some(&live))
                .unwrap()
                .input,
            3.0
        );
        let derived = base.with_user_overrides(std::iter::empty());
        let lookups = AtomicUsize::new(0);
        let fetch = || {
            lookups.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        assert_eq!(
            derived.find_with_live("md-only-1", fetch).unwrap().input,
            3.0
        );
        assert_eq!(lookups.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn gpt_vendor_defaults_do_not_depend_on_the_serving_host() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        for host in ["azure_ai", "github_copilot", "bedrock_mantle", "databricks"] {
            let model = format!("{host}/gpt-5.4");
            pricing.load_json(&format!(
                r#"{{"{model}": {{"input_cost_per_token": 0.0000025, "output_cost_per_token": 0.000015, "litellm_provider": "{host}"}}}}"#
            ));
            let price = pricing
                .resolve_at(None, &[(&model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(price.cache_creation_rate(), 2.5e-6, "{host}");
        }
    }

    #[test]
    fn input_override_moves_default_writes_on_curated_loaded_models() {
        crate::test_support::install_pricing_snapshots();
        for (model, expected) in [("gpt-5", 2e-6), ("moonshot/kimi-k2.5", 2.5e-6)] {
            let mut pricing = PricingMap::default();
            pricing.load_json(&format!(
                r#"{{"{model}": {{"input_cost_per_token": 0.00000125, "output_cost_per_token": 0.00001, "litellm_provider": "{}"}}}}"#,
                if model.starts_with("gpt-") { "openai" } else { "moonshot" }
            ));
            pricing.put_builtin_pricing();
            let overrides = [(
                model.to_string(),
                ccusage_cli::PricingOverride {
                    input_cost_per_token: Some(2e-6),
                    ..ccusage_cli::PricingOverride::default()
                },
            )];
            let overridden =
                pricing.with_user_overrides(overrides.iter().map(|(id, value)| (id, value)));
            let price = overridden.find(model).unwrap();
            assert_eq!(price.cache_create, None, "{model}");
            assert!(
                (price.cache_creation_rate() - expected).abs() < 1e-15,
                "{model}"
            );
        }
    }

    #[test]
    fn every_curated_row_fills_only_the_rates_a_loaded_row_leaves_out() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "glm-5": {"input_cost_per_token": 9, "output_cost_per_token": 10},
            "zai/glm-4.5": {"input_cost_per_token": 9, "output_cost_per_token": 10, "cache_read_input_token_cost": 7}
        }"#,
        );
        pricing.put_builtin_pricing();
        let glm_5 = pricing
            .resolve_at(None, &[("glm-5", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!((glm_5.input, glm_5.output), (9.0, 10.0));
        assert_eq!(
            (glm_5.cache_create, glm_5.cache_read),
            (Some(0.0), Some(0.2e-6))
        );
        let glm_45 = pricing
            .resolve_at(None, &[("zai/glm-4.5", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(
            (glm_45.cache_create, glm_45.cache_read),
            (Some(0.0), Some(7.0))
        );
    }

    #[test]
    fn curated_context_limits_fill_litellm_gaps_ahead_of_models_dev() {
        crate::test_support::install_pricing_snapshots();
        let mut live = PricingMap::default();
        live.insert_test_limit("gpt-5.6-sol", 654_321);
        let lookups = AtomicUsize::new(0);
        let fetch = || {
            lookups.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        let curated = layered(&PricingMap::default(), None);
        assert_eq!(
            curated.context_limit_with_live("gpt-5.6-sol", fetch),
            Some(1_050_000)
        );
        assert_eq!(lookups.load(Ordering::Relaxed), 0);

        let mut litellm = PricingMap::default();
        litellm.load_json(
            r#"{"gpt-5.6-sol": {"input_cost_per_token": 1, "output_cost_per_token": 2, "max_input_tokens": 922000}}"#,
        );
        assert_eq!(
            layered(&litellm, None).context_limit_with_live("gpt-5.6-sol", fetch),
            Some(922_000)
        );
    }

    #[test]
    fn curated_rows_use_the_shared_speed_policy_when_the_primary_entry_is_missing() {
        let mut pricing = PricingMap::default();
        pricing.put_builtin_pricing();
        assert_eq!(
            pricing
                .resolve_at(None, &[("gpt-5.1", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .flex_multiplier,
            0.5
        );
    }

    #[test]
    fn litellm_gpt_rows_without_a_cache_write_rate_bill_it_at_the_input_rate() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (model, input, read) in [
            ("gpt-5.1-codex", 1.25e-6, 0.125e-6),
            ("gpt-5.2-codex", 1.75e-6, 0.175e-6),
            ("gpt-5.2", 1.75e-6, 0.175e-6),
        ] {
            let entry = pricing
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(entry.cache_create, None, "{model}");
            assert_eq!(entry.cache_creation_rate(), input, "{model}");
            assert_eq!(entry.cache_read, Some(read), "{model}");
        }
    }

    #[test]
    fn supplements_preserve_all_published_prices_and_context_metadata() {
        crate::test_support::install_pricing_snapshots();
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
    fn vendor_path_suffix_and_same_identity_outrank_fuzzy_matches() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        primary.load_json(r#"{"sample": {"litellm_provider":"openai","input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{
                "vendor-sample": { "cost": { "input": 3000000, "output": 4000000 } },
                "vendor/sample": { "cost": { "input": 3000000, "output": 4000000 } },
                "sample-tier": { "cost": { "input": 5000000, "output": 6000000 }, "exactOnly": true },
                "unrelated-9": { "cost": { "input": 7000000, "output": 8000000 } }
            }"#,
        );
        let pricing = layered(&primary, Some(&embedded));
        assert_eq!(pricing.find("vendor/sample").unwrap().input, 1.0);
        assert_eq!(pricing.find("vendor-sample").unwrap().input, 3.0);
        assert_eq!(
            pricing
                .resolve_at(None, &[("vendor/sample", super::MatchKind::Exact)], None)
                .unwrap()
                .1
                .input,
            1.0
        );
        assert_eq!(pricing.find("sample-tier").unwrap().input, 5.0);
        assert_eq!(pricing.find("unrelated-9").unwrap().input, 7.0);
    }

    #[test]
    fn live_models_dev_replaces_the_embedded_snapshot_for_a_models_dev_only_id() {
        crate::test_support::install_pricing_snapshots();
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
    fn exact_fallback_uses_live_models_dev_before_the_embedded_snapshot() {
        crate::test_support::install_pricing_snapshots();
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"md-only-1": {"cost": {"input": 1000000, "output": 2000000}}}"#,
        );
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"md-only-1": {"cost": {"input": 3000000, "output": 4000000}}}"#,
        );
        let pricing = layered(&PricingMap::default(), Some(&embedded));
        let lookups = AtomicUsize::new(0);
        let fetch = || {
            lookups.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        for _ in 0..2 {
            assert_eq!(
                pricing
                    .with_live_layer("md-only-1", Lookup::Pricing, fetch)
                    .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
                    .map(|(_, pricing)| pricing)
                    .unwrap()
                    .input,
                3.0
            );
        }
        assert_eq!(lookups.load(Ordering::Relaxed), 1);
        let offline = layered(&PricingMap::default(), Some(&embedded));
        assert_eq!(
            offline
                .with_live_layer("md-only-1", Lookup::Pricing, || None)
                .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
                .map(|(_, pricing)| pricing)
                .unwrap()
                .input,
            1.0
        );
    }

    #[test]
    fn resolver_table_override_inherits_from_the_selected_live_catalogue() {
        crate::test_support::install_pricing_snapshots();
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"sample-only":{"cost":{"input":1,"output":2},"limit":{"context":120000}}}"#,
        );
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"sample-only":{"cost":{"input":8,"output":13,"tiers":[{"input":17,"tier":{"type":"context","size":200000}}]},"limit":{"context":300000}}}"#,
        );
        let id = "sample-only".to_string();
        let fields = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(5e-6),
            ..Default::default()
        };
        let map =
            layered(&PricingMap::default(), Some(&embedded)).with_user_overrides([(&id, &fields)]);
        let attempts = AtomicUsize::new(0);
        let load = || {
            attempts.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
        let rate = map.find_with_live(&id, load).unwrap();
        assert_eq!(rate.input, 5e-6);
        assert!((rate.output - 13e-6).abs() < 1e-12);
        assert!(rate.long_context.is_none());
        assert_eq!(map.context_limit(&id), Some(300_000));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn fuzzy_override_uses_the_live_exact_only_flag() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        assert_eq!(
            primary.load_json(
                r#"{
            "sample-fast": {"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":2}
        }"#
            ),
            1
        );
        let mut embedded = PricingMap::default();
        let mut live = PricingMap::default();
        assert_eq!(
            embedded.load_models_dev_json_for_tests(
                r#"{
            "sample-fast":{"cost":{"input":3,"output":4}}
        }"#
            ),
            Some(1)
        );
        assert_eq!(
            live.load_models_dev_json_for_tests(
                r#"{
            "sample-fast":{"cost":{"input":5,"output":6},"exactOnly":true}
        }"#
            ),
            Some(1)
        );
        let fields = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(8.0),
            ..Default::default()
        };
        let key = "sample-fast".to_owned();
        let map = layered(&primary, Some(&embedded)).with_user_overrides([(&key, &fields)]);
        let attempts = AtomicUsize::new(0);
        let load = || {
            attempts.fetch_add(1, Ordering::Relaxed);
            Some(&live)
        };

        assert_eq!(map.find_with_live(&key, load).unwrap().input, 8.0);
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
        assert!(map.find_with_live("sample-fast-20260101", load).is_none());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert!(map.overrides.resolved(0).unwrap().row.exact_only);
        assert_eq!(map.find_with_live(&key, load).unwrap().input, 8.0);
    }

    #[test]
    fn resolver_table_override_stands_in_for_the_qualified_vendor_row() {
        crate::test_support::install_pricing_snapshots();
        let mut map = PricingMap::default();
        map.load_json(r#"{
            "sample-4.7": {"litellm_provider":"zai","input_cost_per_token":1,"output_cost_per_token":2,"max_input_tokens":120000,"input_cost_per_token_above_200k_tokens":5},
            "zai/sample-4.7": {"litellm_provider":"zai","input_cost_per_token":3,"output_cost_per_token":4,"max_input_tokens":90000}
        }"#);
        let id = "sample-4-7".to_string();
        let fields = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(50.0),
            ..Default::default()
        };
        let map = map.with_user_overrides([(&id, &fields)]);
        for (case, model) in [
            ("reported", "sample-4.7"),
            ("respelled", "sample@4-7"),
            ("48_qualified", "zai/sample-4.7"),
            ("host_path", "reseller/zai/sample-4.7"),
        ] {
            let rate = map.find(model).expect(case);
            assert_eq!((rate.input, rate.output), (50.0, 2.0), "{case}");
            assert_eq!(rate.long_context.unwrap().rates.input, Some(5.0), "{case}");
            assert_eq!(map.context_limit(model), Some(120_000), "{case}");
        }
    }

    #[test]
    fn overrides_retain_the_models_dev_exact_only_boundary() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        primary.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"sample-premium": {"exactOnly": true, "cost": {"input": 3000000, "output": 4000000}}}"#,
        );
        let mut pricing = layered(&primary, Some(&embedded));
        pricing.enable_models_dev_fallback = false;
        let model = "sample-premium".to_string();
        let override_value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(5.0),
            ..Default::default()
        };
        let overridden = pricing.with_user_overrides([(&model, &override_value)]);
        assert_eq!(overridden.find("sample-premium").unwrap().input, 5.0);
        assert!(overridden.find("sample-premium-20260101").is_none());
    }

    #[test]
    fn models_dev_layers_share_rows_without_copying_them_into_primary() {
        crate::test_support::install_pricing_snapshots();
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"md-only-1": {"cost": {"input": 1000000, "output": 2000000}}}"#,
        );
        let pricing = layered(&PricingMap::default(), Some(&embedded));
        assert!(!pricing.rows.entries.contains_key("md-only-1"));
        assert!(std::sync::Arc::ptr_eq(
            pricing.models_dev.as_ref().unwrap(),
            &embedded.rows,
        ));
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"md-only-1": {"cost": {"input": 3000000, "output": 4000000}}}"#,
        );
        let merged = pricing.with_live_layer("md-only-1", Lookup::Pricing, || Some(&live));
        assert!(std::sync::Arc::ptr_eq(&merged.rows, &pricing.rows));
        assert!(std::sync::Arc::ptr_eq(
            merged.live_models_dev.get().unwrap().as_ref().unwrap(),
            &live.rows,
        ));
        assert_eq!(
            merged
                .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
                .map(|(_, pricing)| pricing)
                .unwrap()
                .input,
            3.0
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
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek/deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                },
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                },
                "deepseek-v4-pro": {
                    "litellm_provider": "deepseek",
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
        crate::test_support::install_pricing_snapshots();
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
                    },
                    "claude-opus-5-fast": {
                        "cost": { "input": 9, "output": 17 },
                        "limit": { "context": 765432 },
                        "exactOnly": true
                    }
                }"#,
            ),
            Some(2)
        );

        let mut embedded_models_dev = PricingMap::default();
        assert_eq!(
            embedded_models_dev.load_models_dev_json_for_tests(
                r#"{
                    "claude-opus-5-fast": {
                        "cost": { "input": 5, "output": 10 },
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
    fn resolver_table_schedules_only_a_vendor_identity_hit() {
        crate::test_support::install_pricing_snapshots();
        let mut map = PricingMap::default();
        map.load_json(r#"{
            "deepseek-v4-flash": {"litellm_provider":"bedrock","input_cost_per_token":0.00000062,"output_cost_per_token":0.00000185},
            "deepseek/deepseek-v4-pro": {"litellm_provider":"deepseek","input_cost_per_token":0.00000132,"output_cost_per_token":0.00000396},
            "deepseek-v4-pro": {"litellm_provider":"deepseek","input_cost_per_token":0.00000132,"output_cost_per_token":0.00000396}
        }"#);
        for (case, model, peak, off_peak) in [
            (
                "hosted_exact_is_static",
                "deepseek.v4.flash",
                (0.62, 1.85),
                (0.62, 1.85),
            ),
            (
                "39_vendor_identity",
                "deepseek.v4.pro",
                (1.32, 3.96),
                (0.66, 1.98),
            ),
            (
                "89_path_suffix_is_static",
                "host/deepseek/deepseek-v4-pro",
                (1.32, 3.96),
                (1.32, 3.96),
            ),
            (
                "87_longer_fuzzy_is_static",
                "deepseek-v4-pro-20260812",
                (1.32, 3.96),
                (1.32, 3.96),
            ),
        ] {
            for (instant, expected) in [
                ("2026-11-17T03:00:00Z", peak),
                ("2026-11-17T10:00:00Z", off_peak),
            ] {
                let rates = map.find_at(model, timestamp(instant)).expect(case);
                assert!(
                    (rates.input * 1e6 - expected.0).abs() < 1e-12,
                    "{case}: {rates:?}"
                );
                assert!(
                    (rates.output * 1e6 - expected.1).abs() < 1e-12,
                    "{case}: {rates:?}"
                );
            }
        }
    }

    #[test]
    fn time_dependent_pricing_excludes_reseller_models() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "deepseek-latest",
            "deepseek-v4-flash",
        )]);

        let pricing = direct_deepseek_pricing();
        for model in [
            "deepseek-v4-flash",
            "deepseek.v4.flash",
            "deepseek@v4@flash",
            "deepseek-latest",
            "deepseek/deepseek-v4-flash",
        ] {
            assert!(
                pricing
                    .find_handle(model)
                    .is_some_and(|handle| handle.is_time_dependent())
            );
        }
        for model in [
            "openrouter/deepseek-v4-flash",
            "openrouter/deepseek.v4.flash",
            "gpt-5",
        ] {
            assert!(
                !pricing
                    .find_handle(model)
                    .is_some_and(|handle| handle.is_time_dependent())
            );
        }
    }

    #[test]
    fn find_at_replaces_stale_deepseek_long_context_rates() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
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

        assert_eq!(
            peak.long_context.and_then(|context| context.rates.input),
            Some(0.44e-6)
        );
        assert_eq!(
            peak.long_context.and_then(|context| context.rates.output),
            Some(1.32e-6)
        );
        assert_eq!(
            peak.long_context
                .and_then(|context| context.rates.cache_create),
            Some(0.44e-6)
        );
        assert_eq!(
            peak.long_context
                .and_then(|context| context.rates.cache_read),
            Some(0.014e-6)
        );
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
    fn loading_with_overrides_keeps_the_shared_base_unchanged() {
        crate::test_support::install_pricing_snapshots();
        let model = "gpt-5".to_string();
        let overrides = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(42e-6),
            ..Default::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&model, &overrides)]);
        let base = PricingMap::load_with_overrides(true, false, std::iter::empty());
        assert_eq!(pricing.find(&model).unwrap().input, 42e-6);
        assert_ne!(base.find(&model).unwrap().input, 42e-6);
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
    fn scheduled_override_precedence_selects_one_patch_without_inheriting_its_base_patch() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "scheduled-alias",
            "deepseek.v4.flash",
        )]);
        let base = direct_deepseek_pricing();
        let overrides = std::collections::BTreeMap::from([
            (
                "deepseek-v4-flash".to_string(),
                ccusage_cli::PricingOverride {
                    output_cost_per_token: Some(19.0),
                    ..Default::default()
                },
            ),
            (
                "deepseek.v4.flash".to_string(),
                ccusage_cli::PricingOverride {
                    cache_read_input_token_cost: Some(0.0),
                    ..Default::default()
                },
            ),
            (
                "scheduled-alias".to_string(),
                ccusage_cli::PricingOverride {
                    input_cost_per_token: Some(7.0),
                    ..Default::default()
                },
            ),
        ]);
        let pricing = base.with_user_overrides(&overrides);
        let instant = timestamp("2026-08-17T01:00:00Z");
        let alias = pricing.find_at("scheduled-alias", instant).unwrap();
        assert_eq!(alias.input, 7.0);
        assert_eq!(alias.output, 1.32e-6);
        assert_eq!(alias.cache_read, Some(0.014e-6));
        let resolved = pricing.find_at("deepseek.v4.flash", instant).unwrap();
        assert_eq!(resolved.input, 0.44e-6);
        assert_eq!(resolved.cache_read, Some(0.0));
        let direct = pricing.find_at("deepseek-v4-flash", instant).unwrap();
        assert_eq!(direct.output, 19.0);
        assert_eq!(resolved.output, 19.0);
        assert_eq!(direct.cache_read, Some(0.0));
        assert!(pricing.has_user_override("scheduled-alias"));
        assert!(!base.has_user_override("deepseek-v4-flash"));
    }

    #[test]
    fn long_context_view_names_the_existing_source_billing_modes() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "marginal": {"input_cost_per_token": 1, "output_cost_per_token": 2,
                "input_cost_per_token_above_200k_tokens": 3},
            "flat": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
        );
        let marginal = pricing.find("marginal").unwrap().long_context.unwrap();
        assert_eq!(marginal.mode, super::LongContextMode::Marginal);
        assert_eq!(marginal.threshold, 200_000);
        assert_eq!(marginal.rates.input, Some(3.0));
        assert!(pricing.find("flat").unwrap().long_context.is_none());

        pricing.load_models_dev_json_for_tests(
            r#"{
            "whole": {"cost": {"input": 1, "output": 2, "tiers": [{
                "input": 3, "tier": {"type": "context", "size": 272000}
            }]}}
        }"#,
        );
        let whole = pricing.find("whole").unwrap().long_context.unwrap();
        assert_eq!(whole.mode, super::LongContextMode::WholeRequest);
        assert_eq!(whole.threshold, 272_000);
        assert_eq!(whole.rates.input, Some(3e-6));
    }

    #[test]
    fn ordered_candidates_keep_free_prices_and_per_candidate_match_kinds() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "free": {"litellm_provider":"openai","input_cost_per_token": 0, "output_cost_per_token": 0},
            "paid": {"input_cost_per_token": 1, "output_cost_per_token": 2},
            "provider/paid-version": {"input_cost_per_token": 9, "output_cost_per_token": 10}
        }"#,
        );
        let (matched, free) = pricing
            .resolve_at(
                None,
                &[
                    ("free", super::MatchKind::Fuzzy),
                    ("paid", super::MatchKind::Fuzzy),
                ],
                None,
            )
            .unwrap();
        assert_eq!(matched, "free");
        assert_eq!(free.input, 0.0);
        let (matched, paid) = pricing
            .resolve_at(
                None,
                &[
                    ("provider/paid", super::MatchKind::Exact),
                    ("paid", super::MatchKind::Fuzzy),
                ],
                None,
            )
            .unwrap();
        assert_eq!(matched, "paid");
        assert_eq!(paid.input, 1.0);
        let (matched, price) = pricing
            .resolve_at(
                None,
                &[
                    ("provider/paid", super::MatchKind::Fuzzy),
                    ("paid", super::MatchKind::Fuzzy),
                ],
                None,
            )
            .unwrap();
        assert_eq!(matched, "paid");
        assert_eq!(price.input, 1.0);
    }

    #[test]
    fn respelled_base_model_never_uses_a_longer_model_price() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let price = pricing.find("gpt-5-5").unwrap();
        assert_eq!(price.input, 5e-6);
        assert_eq!(price.output, 30e-6);
        assert_eq!(pricing.find("gpt-5.5-pro").unwrap().input, 30e-6);
    }

    #[test]
    fn respelled_adapter_ids_use_their_own_list_rates() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (model, input, output) in [
            ("gpt-5-1", 1.25, 10.0),
            ("gpt-5-1-codex", 1.25, 10.0),
            ("gpt-5-2", 1.75, 14.0),
            ("gpt-5-4", 2.5, 15.0),
            ("gpt-5-5", 5.0, 30.0),
            ("gpt-5-6", 4.0, 20.0),
            ("gpt-4-1", 2.0, 8.0),
            ("gemini-2-5-flash", 0.3, 2.5),
            ("gemini-3-5-flash", 1.5, 9.0),
            ("glm-4-7", 0.6, 2.2),
            ("deepseek/deepseek.v4.flash", 0.3, 1.2),
            ("deepseek/deepseek.v4.pro", 1.32, 3.96),
        ] {
            let rates = pricing
                .resolve_at(None, &[(model, super::MatchKind::Fuzzy)], None)
                .unwrap()
                .1;
            let direct = pricing.find(model).unwrap();
            assert_eq!(
                (rates.input, rates.output),
                (direct.input, direct.output),
                "{model}"
            );
            assert!((rates.input * 1e6 - input).abs() < 1e-9, "{model}");
            assert!((rates.output * 1e6 - output).abs() < 1e-9, "{model}");
        }
    }

    #[test]
    fn exact_candidate_kinds_keep_the_same_model_and_respelled_overrides() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::load_embedded();
        pricing.load_json(
            r#"{"gpt-5-5":{"input_cost_per_token":0.000005,"output_cost_per_token":0.00003}}"#,
        );
        for kind in [super::MatchKind::Exact, super::MatchKind::StrictExact] {
            let (_, price) = pricing
                .resolve_at(None, &[("gpt-5-5", kind)], None)
                .unwrap();
            assert_eq!(price.input, 5e-6);
            assert_eq!(price.output, 30e-6);
        }
        let key = "claude-sonnet-4-5".to_string();
        let value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(7e-6),
            ..Default::default()
        };
        let pricing = pricing.with_user_overrides([(&key, &value)]);
        for model in [
            "claude-sonnet-4-5",
            "claude-sonnet-4.5",
            "claude.sonnet.4.5",
        ] {
            assert!(pricing.has_user_override(model), "{model}");
            for kind in [super::MatchKind::Exact, super::MatchKind::Fuzzy] {
                assert_eq!(
                    pricing
                        .resolve_at(None, &[(model, kind)], None)
                        .unwrap()
                        .1
                        .input,
                    7e-6
                );
            }
            assert_eq!(pricing.find(model).unwrap().input, 7e-6);
        }
        let mut primary = PricingMap::default();
        primary
            .load_json(r#"{"sample-pro":{"input_cost_per_token":9,"output_cost_per_token":18}}"#);
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"sample":{"cost":{"input":1000000,"output":2000000}}}"#,
        );
        let pricing = layered(&primary, Some(&embedded));
        let (_, price) = pricing
            .resolve_at(None, &[("sample", super::MatchKind::Exact)], None)
            .unwrap();
        assert_eq!(price.input, 1.0);
        assert_eq!(price.output, 2.0);
    }

    #[test]
    fn strict_candidates_use_authoritative_identities_without_path_suffixes() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        primary.load_json(r#"{"sample":{"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":2},"grok-4-1-x":{"input_cost_per_token":3,"output_cost_per_token":4}}"#);
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"vendor/sample":{"cost":{"input":3000000,"output":4000000}}}"#,
        );
        let pricing = layered(&primary, Some(&embedded));
        assert_eq!(
            pricing
                .resolve_at(None, &[("vendor/sample", super::MatchKind::Exact)], None)
                .unwrap()
                .1
                .input,
            1.0
        );
        assert!(
            pricing
                .resolve_at(
                    None,
                    &[("vendor/sample", super::MatchKind::StrictExact)],
                    None
                )
                .is_none()
        );
        assert_eq!(
            pricing
                .resolve_at(None, &[("grok-4.1-x", super::MatchKind::StrictExact)], None)
                .unwrap()
                .1
                .input,
            3.0
        );
        assert_eq!(
            pricing
                .resolve_at(None, &[("grok-4.1-x", super::MatchKind::Exact)], None)
                .unwrap()
                .1
                .input,
            3.0
        );
    }

    #[test]
    fn timestamped_candidates_keep_exact_provider_rates_and_schedule_direct_names() {
        let pricing = direct_deepseek_pricing();
        let instant = timestamp("2026-08-17T01:00:00Z");
        let (matched, provider) = pricing
            .resolve_at(
                None,
                &[
                    ("openrouter/deepseek.v4.flash", super::MatchKind::Exact),
                    ("deepseek-v4-flash", super::MatchKind::Fuzzy),
                ],
                Some(instant),
            )
            .unwrap();
        assert_eq!(matched, "openrouter/deepseek.v4.flash");
        assert_eq!(provider.input, 9e-6);
        let (matched, direct) = pricing
            .resolve_at(
                None,
                &[
                    ("unknown/deepseek-v4-flash", super::MatchKind::Exact),
                    ("deepseek-v4-flash", super::MatchKind::Fuzzy),
                ],
                Some(instant),
            )
            .unwrap();
        assert_eq!(matched, "deepseek-v4-flash");
        assert_eq!(direct.input, 0.44e-6);
    }

    #[test]
    fn scheduled_alias_override_preserves_duration_and_speed_fields_from_the_selected_patch() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "deepseek-v4-flash",
            "custom-base",
        )]);
        let base = direct_deepseek_pricing();
        let model = "custom-base".to_string();
        let value = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(2.0),
            cache_creation_input_token_cost: Some(3.0),
            cache_read_input_token_cost: Some(4.0),
            input_cost_per_token_above_200k_tokens: Some(5.0),
            output_cost_per_token_above_200k_tokens: Some(6.0),
            cache_creation_input_token_cost_above_200k_tokens: Some(7.0),
            cache_read_input_token_cost_above_200k_tokens: Some(8.0),
            cache_creation_input_token_cost_above_1hr: Some(13.0),
            fast_multiplier: Some(3.0),
            flex_multiplier: Some(0.4),
            ..Default::default()
        };
        let pricing = base.with_user_overrides([(&model, &value)]);
        let scheduled = pricing
            .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
            .unwrap();
        assert_eq!(scheduled.input, 1.0);
        assert_eq!(scheduled.output, 2.0);
        assert_eq!(scheduled.cache_create, Some(3.0));
        assert_eq!(scheduled.cache_read, Some(4.0));
        let rates = scheduled.long_context.unwrap().rates;
        assert_eq!(rates.input, Some(5.0));
        assert_eq!(rates.output, Some(6.0));
        assert_eq!(rates.cache_create, Some(7.0));
        assert_eq!(rates.cache_read, Some(8.0));
        assert_eq!(scheduled.cache_create_1h, Some(13.0));
        assert_eq!(scheduled.fast_multiplier, 3.0);
        assert_eq!(scheduled.flex_multiplier, 0.4);
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
                pricing
                    .resolve_at(None, &[("gpt-fetched", MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
                    .is_some(),
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
        assert_eq!(kimi_k26.input, 0.95e-6);
        assert_eq!(kimi_k26.output, 4e-6);
        assert_eq!(kimi_k26.cache_read, Some(0.16e-6));
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.5"), Some(262_144));
        assert_eq!(pricing.context_limit("moonshot/kimi-k2.6"), Some(262_144));
    }

    #[test]
    fn offline_prices_models_outside_the_claude_and_moonshot_families() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let grok_build = pricing
            .find("grok-build-0.1")
            .expect("embedded models.dev should include xAI pricing");

        assert!((grok_build.input * 1e6 - 1.0).abs() < 1e-9);
        assert!((grok_build.output * 1e6 - 2.0).abs() < 1e-9);
        assert_eq!(pricing.context_limit("grok-build-0.1"), Some(256_000));
    }

    #[test]
    fn embedded_models_dev_omits_models_priced_per_asset() {
        crate::test_support::install_pricing_snapshots();
        let embedded = embedded_models_dev_pricing();

        for model in [
            "whisper-large-v3",
            "gemini-2.5-flash-image",
            "gemini-3-pro-image-preview",
        ] {
            assert!(
                embedded
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
                    .is_none(),
                "{model} prices assets rather than text tokens and must stay out of the snapshot"
            );
        }
        assert!(
            embedded
                .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
        assert!(
            embedded
                .resolve_at(None, &[("gemini-3-flash-preview", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
    }

    #[test]
    fn embedded_models_dev_prices_resold_models_from_their_author() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let kimi_k27_code = pricing.find("kimi-k2.7-code").unwrap();

        assert!((kimi_k27_code.input * 1e6 - 0.95).abs() < 1e-9);
        assert!((kimi_k27_code.output * 1e6 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn exact_only_fallback_beats_a_fuzzy_primary_match() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            models_dev
                .rows
                .entries
                .get("kimi-k2.7-code-highspeed")
                .is_some_and(|row| row.exact_only)
        );

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
            assert!(
                embedded
                    .rows
                    .entries
                    .get(model)
                    .is_some_and(|row| row.exact_only),
                "{model}"
            );
            assert!(pricing.find(model).is_some(), "{model}");
            assert!(pricing.context_limit(model).is_some(), "{model}");
        }

        let expected_fast = embedded
            .resolve_at(None, &[("claude-opus-5-fast", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        let actual_fast = pricing.find("claude-opus-5-fast").unwrap();
        assert_eq!(actual_fast.input, expected_fast.input);
        assert_eq!(actual_fast.output, expected_fast.output);
        assert_eq!(
            pricing.context_limit("claude-opus-5-fast"),
            embedded.context_limit("claude-opus-5-fast")
        );

        assert!(pricing.find("claude-opus-5-20260115").is_some());
    }

    #[test]
    fn a_separator_spelling_of_an_exact_only_id_is_priced_as_that_id() {
        crate::test_support::install_pricing_snapshots();
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

        assert!(pricing.find("claude-opus-5-eu-preview").is_none());
    }

    #[test]
    fn a_dotted_spelling_of_a_dash_spelled_exact_only_id_is_priced_as_that_id() {
        crate::test_support::install_pricing_snapshots();
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
            .resolve_at(None, &[("claude-opus-5@eu", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .expect("the embedded snapshot should contain the exact-only regional id");

        for spelling in ["claude-opus-5-eu", "claude-opus-5.eu"] {
            let resolved = pricing
                .resolve_at(None, &[(spelling, MatchKind::Exact)], None)
                .map(|(_, pricing)| pricing)
                .expect("separator-equivalent exact-only ids should resolve");
            assert_eq!(resolved.input, expected.input, "{spelling}");
            assert_eq!(resolved.output, expected.output, "{spelling}");
        }
        assert!(
            pricing
                .resolve_at(
                    None,
                    &[("claude-opus-5-eu-preview", MatchKind::Exact)],
                    None
                )
                .map(|(_, pricing)| pricing)
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
            .resolve_at(None, &[("claude-opus-5-eu", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .expect("the normalized primary entry should resolve");
        assert_eq!(resolved.input, 0.000009);
        assert_eq!(resolved.output, 0.000010);
    }

    #[test]
    fn exact_fallback_prefers_literal_keys_then_the_smallest_spelling() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        assert_eq!(pricing.load_json(r#"{
            "acme-alpha-1.5": {"litellm_provider":"openai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "acme.alpha@1.5": {"litellm_provider":"openai","input_cost_per_token":2e-6,"output_cost_per_token":2e-6}
        }"#), 2);
        for (request, key, rate) in [
            ("acme-alpha-1.5", "acme-alpha-1.5", 1e-6),
            ("acme.alpha@1.5", "acme.alpha@1.5", 2e-6),
            ("acme-alpha-1-5", "acme-alpha-1.5", 1e-6),
        ] {
            let candidates = [(request, MatchKind::Exact)];
            let found = pricing.explain(None, &candidates).unwrap();
            assert_eq!(found.row_key, key, "{request}");
            let (_, price) = pricing.resolve_at(None, &candidates, None).unwrap();
            assert_eq!((price.input, price.output), (rate, rate), "{request}");
        }
    }

    #[test]
    fn exact_fallback_lookup_sees_a_colliding_entry_added_after_a_lookup() {
        let mut pricing = PricingMap::default();
        pricing.supplement_pricing(
            "acme-alpha-1.5",
            Pricing {
                input: 1e-6,
                ..Pricing::empty()
            },
            None,
            "",
        );
        let resolved = pricing
            .resolve_at(None, &[("acme-alpha-1-5", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .expect("a single normalized match should resolve");
        assert_eq!(resolved.input, 1e-6);

        pricing.supplement_pricing(
            "acme.alpha@1.5",
            Pricing {
                input: 2e-6,
                ..Pricing::empty()
            },
            None,
            "",
        );

        assert_eq!(
            pricing
                .resolve_at(None, &[("acme-alpha-1-5", MatchKind::Exact)], None)
                .unwrap()
                .1
                .input,
            1e-6
        );
        assert_eq!(
            pricing
                .resolve_at(None, &[("acme.alpha@1.5", MatchKind::Exact)], None)
                .unwrap()
                .1
                .input,
            2e-6
        );
    }

    #[test]
    fn exact_fallback_lookup_sees_rates_patched_after_a_lookup() {
        crate::test_support::install_pricing_snapshots();
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
            .resolve_at(None, &[("zai/glm-test-5", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .expect("the normalized entry should resolve");
        assert!(before.cache_read.is_none());

        pricing.supplement_pricing(
            "zai/glm-test.5",
            Pricing {
                cache_read: Some(0.03),
                ..Pricing::empty()
            },
            None,
            "",
        );

        let after = pricing
            .resolve_at(None, &[("zai/glm-test-5", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .unwrap();
        assert_eq!(after.cache_read, Some(0.03));
    }

    #[test]
    fn configured_alias_of_an_exact_only_tier_beats_a_fuzzy_match_on_the_alias() {
        crate::test_support::install_pricing_snapshots();
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
        builtin.insert_test_limit("claude-opus-5", 123_456);
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
                .context_limit_with_live("claude-opus-5-fast", || None),
            None,
            "an exact-only id without a published limit must not inherit a fuzzy builtin limit"
        );
    }

    #[test]
    fn offline_prices_kimi_k3_from_embedded_models_dev() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let kimi_k3 = pricing.find("moonshot/kimi-k3").unwrap_or_else(|| {
            pricing
                .find("kimi-k3")
                .expect("embedded models.dev should include kimi-k3 pricing")
        });

        assert_eq!(kimi_k3.input, 3e-6);
        assert_eq!(kimi_k3.output, 15e-6);
        assert_eq!(kimi_k3.cache_read, Some(0.3e-6));
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
        crate::test_support::install_pricing_snapshots();
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

        pricing.supplement_pricing("zai/glm-test-model", zeroed, None, "");

        let entry = pricing
            .resolve_at(None, &[("zai/glm-test-model", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.input),
            Some(2e-6)
        );
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.output),
            Some(20e-6)
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_create),
            Some(2.5e-6)
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_read),
            Some(0.2e-6)
        );
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
            .find_entry("gpt-first", MatchKind::Fuzzy)
            .expect("the loaded body should price its model");
        assert_eq!(gpt_first.input, 0.000001);
        assert_eq!(gpt_first.output, 0.000002);
        assert_eq!(
            loaded.context_limit_entry("gpt-first", MatchKind::Fuzzy),
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
        let alias = pricing
            .resolve_at(None, &[("gpt-alias", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();

        assert_eq!(primary.input, 1e-6);
        assert_eq!(primary.output, 10e-6);
        assert_eq!(primary.cache_read, Some(0.1e-6));
        assert_eq!(pricing.context_limit("gpt-primary"), Some(123));
        assert!((fallback.input - 2e-6).abs() < f64::EPSILON);
        assert!((fallback.output - 8e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_create.unwrap() - 2.5e-6).abs() < f64::EPSILON);
        assert!((fallback.cache_read.unwrap() - 0.2e-6).abs() < f64::EPSILON);
        assert_eq!(
            fallback
                .long_context
                .and_then(|context| context.rates.input),
            None
        );
        assert_eq!(
            fallback
                .long_context
                .and_then(|context| context.rates.output),
            None
        );
        assert_eq!(fallback.fast_multiplier, 1.0);
        assert_eq!(pricing.context_limit("gpt-fallback"), Some(456));
        assert!((alias.input - 4e-6).abs() < f64::EPSILON);
        assert_eq!(pricing.context_limit("gpt-alias"), Some(654));
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
            pricing
                .resolve_at(None, &[("gpt-6-astra", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .fast_multiplier,
            2.0
        );
        assert_eq!(
            pricing
                .resolve_at(None, &[("gpt-6-astra", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .flex_multiplier,
            0.5
        );
    }

    #[test]
    fn live_models_dev_pricing_prefers_the_authoring_catalog_over_resellers() {
        crate::test_support::install_pricing_snapshots();
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

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let kimi = pricing
            .resolve_at(None, &[("kimi-k2.7-code", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((kimi.input * 1e6 - 0.95).abs() < 1e-9);
        assert!((kimi.output * 1e6 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_skips_models_priced_per_asset() {
        crate::test_support::install_pricing_snapshots();
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

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        assert!(
            pricing
                .resolve_at(None, &[("gemini-2.5-flash-image", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
        assert!(
            pricing
                .resolve_at(
                    None,
                    &[("some-unlisted-transcriber", MatchKind::Exact)],
                    None
                )
                .map(|(_, rates)| rates)
                .is_none()
        );
        assert!(
            pricing
                .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
    }

    #[test]
    fn live_models_dev_pricing_keys_asset_pricing_on_the_source_model_id() {
        crate::test_support::install_pricing_snapshots();
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
                .resolve_at(
                    None,
                    &[("models/gemini-2.5-flash-image", MatchKind::Exact)],
                    None
                )
                .map(|(_, rates)| rates)
                .is_none()
        );
    }

    #[test]
    fn live_models_dev_pricing_rejects_explicitly_empty_modalities() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .resolve_at(None, &[("empty-output-model", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
        assert!(
            pricing
                .resolve_at(None, &[("empty-input-model", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
        assert!(
            pricing
                .resolve_at(None, &[("no-modalities-model", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
    }

    #[test]
    fn live_models_dev_pricing_carries_the_reseller_ids_generation_carries() {
        crate::test_support::install_pricing_snapshots();
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
                .resolve_at(
                    None,
                    &[("accounts/fireworks/models/kimi-k2p6", MatchKind::Exact)],
                    None
                )
                .map(|(_, rates)| rates)
                .is_some()
        );
        assert!(
            pricing
                .resolve_at(None, &[("claude-opus-5-fast", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
        assert!(
            pricing
                .resolve_at(None, &[("claude-3-haiku-20240307", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
        assert!(
            pricing
                .rows
                .entries
                .get("claude-opus-5-fast")
                .is_some_and(|row| row.exact_only)
        );
        assert!(
            !pricing
                .rows
                .entries
                .get("claude-3-haiku-20240307")
                .is_some_and(|row| row.exact_only)
        );
    }

    #[test]
    fn live_models_dev_pricing_prefers_the_more_detailed_entry_within_a_tier() {
        crate::test_support::install_pricing_snapshots();
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

        assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
        let entry = pricing
            .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!(entry.cache_read.is_some());
        assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
        assert_eq!(
            pricing
                .rows
                .entries
                .get("kimi-k2.6-nitro")
                .and_then(|row| row.context_limit.as_ref()),
            Some(&128_000)
        );
    }

    #[test]
    fn live_models_dev_pricing_keeps_authored_token_models_a_catalog_mislabels() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .resolve_at(None, &[("claude-opus-5", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
    }

    #[test]
    fn live_models_dev_pricing_ranks_by_the_catalog_s_own_provider_id() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_orders_detail_the_way_generation_does() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!(entry.cache_read.is_some());
        assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_drops_the_context_limit_of_a_replaced_catalog() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
        assert_eq!(
            pricing
                .rows
                .entries
                .get("kimi-k2.6-nitro")
                .and_then(|row| row.context_limit.as_ref()),
            None
        );
    }

    #[test]
    fn live_models_dev_pricing_resolves_duplicate_ids_inside_one_catalog_stably() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(None, &[("shared-model", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_merges_spelling_duplicates_onto_one_entry() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(!pricing.rows.entries.contains_key("grok-9-5"));
        assert_eq!(
            pricing
                .explain(None, &[("grok-9-5", MatchKind::Exact)])
                .unwrap()
                .row_key,
            "grok-9.5"
        );
        let entry = pricing
            .resolve_at(None, &[("grok-9.5", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(
            entry.long_context.map(|context| context.threshold),
            Some(200_000)
        );
        let via_dash = pricing.find("grok-9-5").unwrap();
        assert_eq!(
            via_dash.long_context.map(|context| context.threshold),
            Some(200_000)
        );
    }

    #[test]
    fn live_models_dev_pricing_prefers_a_tiered_catalog_within_a_trust_tier() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(
                None,
                &[("some-reseller-only-model", MatchKind::Exact)],
                None,
            )
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(
            entry.long_context.map(|context| context.threshold),
            Some(200_000)
        );
    }

    #[test]
    fn live_models_dev_pricing_stores_a_model_with_an_empty_declared_id_by_its_key() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .resolve_at(None, &[("kimi-k9", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
        assert!(
            pricing
                .resolve_at(None, &[("", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
    }

    #[test]
    fn live_models_dev_pricing_orders_same_id_catalogs_by_their_map_key() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(
                None,
                &[("some-reseller-only-model", MatchKind::Exact)],
                None,
            )
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_breaks_exact_ties_by_source_key_across_same_id_catalogs() {
        crate::test_support::install_pricing_snapshots();
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
        let entry = pricing
            .resolve_at(
                None,
                &[("some-reseller-only-model", MatchKind::Exact)],
                None,
            )
            .map(|(_, rates)| rates)
            .unwrap();
        assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn live_models_dev_pricing_skips_flat_fee_catalogs() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .resolve_at(None, &[("kimi-for-coding", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
        assert!(
            pricing
                .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none()
        );
    }

    #[test]
    fn live_models_dev_pricing_keeps_a_tier_out_of_the_fuzzy_lookup() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .rows
                .entries
                .get("claude-opus-5@eu")
                .is_some_and(|row| row.exact_only)
        );
    }

    #[test]
    fn live_models_dev_pricing_keeps_an_unversioned_id_out_of_the_fuzzy_lookup() {
        crate::test_support::install_pricing_snapshots();
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
        assert!(
            pricing
                .resolve_at(None, &[("auto", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some()
        );
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
            let model = pricing
                .resolve_at(None, &[(name, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap();
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
            .rows
            .matches()
            .map(|found| found.key)
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
                .find_entry("claude-fable-5-1", MatchKind::Fuzzy)
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

        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let entry = pricing.find(model).unwrap();
            let litellm_source = litellm_snapshot
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates);
            let base_source = litellm_source
                .or_else(|| {
                    models_dev_snapshot
                        .resolve_at(None, &[(model, MatchKind::Exact)], None)
                        .map(|(_, rates)| rates)
                })
                .unwrap();
            let tier_source = litellm_source
                .filter(|source| {
                    source
                        .long_context
                        .and_then(|context| context.rates.input)
                        .is_some()
                        || source
                            .long_context
                            .and_then(|context| context.rates.output)
                            .is_some()
                        || source
                            .long_context
                            .and_then(|context| context.rates.cache_create)
                            .is_some()
                        || source
                            .long_context
                            .and_then(|context| context.rates.cache_read)
                            .is_some()
                })
                .or_else(|| {
                    models_dev_snapshot
                        .resolve_at(None, &[(model, MatchKind::Exact)], None)
                        .map(|(_, rates)| rates)
                })
                .unwrap();
            assert_eq!(entry.input, base_source.input, "{model}");
            assert_eq!(entry.output, base_source.output, "{model}");
            assert_eq!(entry.cache_create, base_source.cache_create, "{model}");
            assert_eq!(entry.cache_read, base_source.cache_read, "{model}");
            assert_eq!(
                entry.long_context.and_then(|context| context.rates.input),
                tier_source
                    .long_context
                    .and_then(|context| context.rates.input),
                "{model}"
            );
            assert_eq!(
                entry.long_context.and_then(|context| context.rates.output),
                tier_source
                    .long_context
                    .and_then(|context| context.rates.output),
                "{model}"
            );
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_create),
                tier_source
                    .long_context
                    .and_then(|context| context.rates.cache_create),
                "{model}"
            );
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_read),
                tier_source
                    .long_context
                    .and_then(|context| context.rates.cache_read),
                "{model}"
            );
            assert_eq!(
                entry.long_context.map(|context| context.threshold),
                tier_source.long_context.map(|context| context.threshold),
                "{model}"
            );
            assert_eq!(
                pricing.context_limit(model),
                litellm_snapshot
                    .rows
                    .entries
                    .get(model)
                    .and_then(|row| row.context_limit.as_ref())
                    .copied()
                    .or_else(|| models_dev_snapshot
                        .rows
                        .entries
                        .get(model)
                        .and_then(|row| row.context_limit.as_ref())
                        .copied())
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
        crate::test_support::install_pricing_snapshots();
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
        let loaded = pricing
            .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        let loaded_context_limit = pricing.context_limit("gpt-5.6-sol");

        let mut builtin = PricingMap::default();
        builtin.put_builtin_pricing();
        let builtin_rates = builtin
            .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_ne!(loaded.input, builtin_rates.input);
        assert_ne!(loaded.output, builtin_rates.output);

        pricing.put_builtin_pricing();
        let resolved = pricing
            .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(resolved.input, loaded.input);
        assert_eq!(resolved.output, loaded.output);
        assert_eq!(resolved.cache_create, loaded.cache_create);
        assert_eq!(resolved.cache_read, loaded.cache_read);
        assert_eq!(resolved.fast_multiplier, loaded.fast_multiplier);
        assert_eq!(pricing.context_limit("gpt-5.6-sol"), loaded_context_limit);

        let mut embedded_models_dev = PricingMap::default();
        embedded_models_dev.insert_test_limit("gpt-5.6-sol", 777_777);
        let mut live_models_dev = PricingMap::default();
        live_models_dev.insert_test_limit("gpt-5.6-sol", 888_888);
        assert_eq!(
            layered(&pricing, Some(&embedded_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
            Some(654_321)
        );
        assert_eq!(
            layered(&PricingMap::default(), Some(&embedded_models_dev))
                .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
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
        assert_eq!(
            alias.long_context.and_then(|context| context.rates.input),
            sol.long_context.and_then(|context| context.rates.input)
        );
        assert_eq!(
            alias.long_context.and_then(|context| context.rates.output),
            sol.long_context.and_then(|context| context.rates.output)
        );
        assert_eq!(
            pricing.context_limit("gpt-5.6"),
            pricing.context_limit("gpt-5.6-sol")
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.6")
                .unwrap()
                .long_context_split_threshold(),
            PricingMap::load_embedded()
                .find("gpt-5.6-sol")
                .unwrap()
                .long_context_split_threshold()
        );
    }

    #[test]
    fn embedded_gpt_5_6_exact_entry_wins_over_the_sol_alias() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let exact = pricing
            .resolve_at(None, &[("gpt-5.6", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        let resolved = pricing.find("gpt-5.6").unwrap();

        assert_eq!(resolved.input, exact.input);
        assert_eq!(resolved.output, exact.output);
        assert_eq!(resolved.cache_create, exact.cache_create);
        assert_eq!(resolved.cache_read, exact.cache_read);
        assert_eq!(
            resolved
                .long_context
                .and_then(|context| context.rates.input),
            exact.long_context.and_then(|context| context.rates.input)
        );
        assert_eq!(
            resolved
                .long_context
                .and_then(|context| context.rates.output),
            exact.long_context.and_then(|context| context.rates.output)
        );
        assert_eq!(
            resolved.long_context.map(|context| context.threshold),
            exact.long_context.map(|context| context.threshold)
        );
        assert_eq!(
            pricing.context_limit("gpt-5.6"),
            pricing
                .rows
                .entries
                .get("gpt-5.6")
                .and_then(|row| row.context_limit.as_ref())
                .copied()
        );
    }

    #[test]
    fn embedded_pricing_fills_gpt_long_context_tier_rates() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        let gpt_55 = pricing.find("gpt-5.5").unwrap();
        assert_eq!(
            gpt_55.long_context.and_then(|context| context.rates.input),
            Some(10e-6)
        );
        assert_eq!(
            gpt_55.long_context.and_then(|context| context.rates.output),
            Some(45e-6)
        );
        assert_eq!(
            gpt_55
                .long_context
                .and_then(|context| context.rates.cache_read),
            Some(1e-6)
        );
        assert_eq!(
            gpt_55.long_context.map(|context| context.threshold),
            Some(272_000)
        );

        let gpt_54 = pricing.find("gpt-5.4").unwrap();
        assert_eq!(
            gpt_54.long_context.and_then(|context| context.rates.input),
            Some(5e-6)
        );
        assert_eq!(
            gpt_54.long_context.and_then(|context| context.rates.output),
            Some(22.5e-6)
        );
        assert_eq!(
            gpt_54.long_context.map(|context| context.threshold),
            Some(272_000)
        );

        let mini = pricing.find("gpt-5.4-mini").unwrap();
        assert_eq!(
            mini.long_context.and_then(|context| context.rates.input),
            None
        );
        assert_eq!(mini.long_context.map(|context| context.threshold), None);
    }

    #[test]
    fn long_context_overlay_survives_litellm_refresh_and_defers_to_upstream() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::load_embedded();
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
        assert_eq!(
            gpt_55.long_context.and_then(|context| context.rates.input),
            Some(10e-6)
        );
        assert_eq!(
            gpt_55.long_context.map(|context| context.threshold),
            Some(272_000)
        );
        let dated = pricing
            .resolve_at(None, &[("gpt-5.5-2026-04-23", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(
            dated.long_context.and_then(|context| context.rates.input),
            Some(10e-6)
        );

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
        assert_eq!(
            gpt_55.long_context.and_then(|context| context.rates.input),
            Some(12e-6)
        );
        assert_eq!(
            gpt_55.long_context.unwrap().mode,
            super::LongContextMode::Marginal
        );
        assert_eq!(gpt_55.long_context.unwrap().threshold, 200_000);
    }

    #[test]
    fn embedded_models_dev_carries_grok_long_context_tiers() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();

        for model in ["grok-4.5", "grok-4.6"] {
            let entry = pricing.find(model).unwrap();
            assert!((entry.input * 1e6 - 2.0).abs() < 1e-9, "{model} base input");
            assert!(
                (entry
                    .long_context
                    .and_then(|context| context.rates.input)
                    .unwrap()
                    * 1e6
                    - 4.0)
                    .abs()
                    < 1e-9,
                "{model} long-context input"
            );
            assert!(
                (entry
                    .long_context
                    .and_then(|context| context.rates.output)
                    .unwrap()
                    * 1e6
                    - 12.0)
                    .abs()
                    < 1e-9,
                "{model} long-context output"
            );
            assert_eq!(
                entry.long_context.map(|context| context.threshold),
                Some(200_000),
                "{model}"
            );
        }
        assert_eq!(
            PricingMap::load_embedded()
                .find("grok-4.5")
                .unwrap()
                .long_context_split_threshold(),
            200_000
        );
    }

    #[test]
    fn long_context_split_threshold_is_per_model() {
        crate::test_support::install_pricing_snapshots();
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.6-sol")
                .unwrap()
                .long_context_split_threshold(),
            272_000
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.5")
                .unwrap()
                .long_context_split_threshold(),
            272_000
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.5-pro")
                .unwrap()
                .long_context_split_threshold(),
            272_000
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.5-2026-04-23")
                .unwrap()
                .long_context_split_threshold(),
            272_000
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5")
                .unwrap()
                .long_context_split_threshold(),
            200_000
        );
        assert_eq!(
            PricingMap::load_embedded()
                .find("gpt-5.4-mini")
                .unwrap()
                .long_context_split_threshold(),
            200_000
        );
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
            .find_entry("claude-opus-4-8", MatchKind::Fuzzy)
            .unwrap();
        let resolved = pricing.find("claude-opus-4-8").unwrap();

        assert_eq!(resolved.input, original.input);
        assert_eq!(
            pricing.context_limit("claude-opus-4-8"),
            pricing.context_limit_entry("claude-opus-4-8", MatchKind::Fuzzy)
        );
    }

    #[test]
    fn pricing_lookup_prefers_an_exact_alias_target_over_a_fuzzy_original_match() {
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        models_dev.mark_test_exact_only("gpt-5.6-luna");

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
        crate::test_support::install_pricing_snapshots();
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
                pricing
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
                    .unwrap()
                    .fast_multiplier,
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
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-sol", "gpt-6.1-sol"),
            ("gpt-6.1-terra", "gpt-6.1-sol"),
        ]);
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

        let cycle_a = pricing
            .resolve_at(None, &[("cycle-a", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!((cycle_a.input, cycle_a.output), (4e-6, 0.0));
        let cycle_b = pricing
            .resolve_at(None, &[("cycle-b", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!((cycle_b.input, cycle_b.output), (0.0, 9e-6));
        let into_cycle = pricing
            .resolve_at(None, &[("into-cycle", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
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
        crate::test_support::install_pricing_snapshots();
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
                pricing
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
                    .unwrap()
                    .flex_multiplier,
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
            PricingMap::load_embedded()
                .find("gpt-reserve")
                .unwrap()
                .long_context_split_threshold(),
            PricingMap::load_embedded()
                .find("gpt-5.6-luna")
                .unwrap()
                .long_context_split_threshold()
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
    fn same_last_segment_wins_over_a_longer_fuzzy_key() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{
            "short/model-2.5": {"input_cost_per_token": 3, "output_cost_per_token": 4},
            "longer/catalog/model-2.5-extension": {"input_cost_per_token": 9, "output_cost_per_token": 10}
        }"#);
        for name in ["model-2.5", "model-2-5", "short/model-2@5"] {
            let rate = pricing.find(name).unwrap();
            assert_eq!((rate.input, rate.output), (3.0, 4.0), "{name}");
        }
    }

    #[test]
    fn resolver_table_vendor_free_rows_only_answer_their_identity_or_segment() {
        crate::test_support::install_pricing_snapshots();
        let mut map = PricingMap::default();
        map.load_json(r#"{
            "plan-2.5": {"litellm_provider":"zai","input_cost_per_token": 1,"output_cost_per_token":2},
            "zai/plan-2.5-flash": {"litellm_provider":"zai","input_cost_per_token": 0,"output_cost_per_token":0},
            "host/plan-2.5-flash": {"litellm_provider":"reseller","input_cost_per_token": 0,"output_cost_per_token":0}
        }"#);
        for (case, name, expected) in [
            ("54_bare_free", "plan-2.5-flash", (0.0, 0.0)),
            (
                "55_foreign_prefix_free",
                "foreign/plan-2-5-flash",
                (0.0, 0.0),
            ),
            ("53_qualified_free", "zai/plan-2-5-flash", (0.0, 0.0)),
        ] {
            let rates = map.find(name).expect(case);
            assert_eq!((rates.input, rates.output), expected, "{case}");
        }
        assert!(
            map.find("plan-2.5-flash-20260101").is_none(),
            "free_tier_does_not_win_fuzzy"
        );
        let (_, hosted) = map
            .resolve_at(
                None,
                &[("host/plan-2.5-flash", MatchKind::StrictExact)],
                None,
            )
            .unwrap();
        assert_eq!((hosted.input, hosted.output), (0.0, 0.0));
    }

    #[test]
    fn resolver_table_segment_matches() {
        crate::test_support::install_pricing_snapshots();
        let map = PricingMap::load_embedded();
        for (case, model, expected) in [
            (
                "71_fireworks_short_flash",
                "deepseek-v4p1-flash",
                (0.3e-6, 1.2e-6),
            ),
            ("72_fireworks_short_glm", "glm-5p3", (1.4e-6, 4.4e-6)),
        ] {
            let entry = map.find(model).expect(case);
            assert!(
                (entry.input - expected.0).abs() < 1e-12,
                "{case}: {entry:?}"
            );
            assert!(
                (entry.output - expected.1).abs() < 1e-12,
                "{case}: {entry:?}"
            );
        }
    }

    #[test]
    fn builtin_price_aliases_require_an_explicit_table_entry() {
        assert_eq!(super::data::ALIASES.len(), 3);
    }

    #[test]
    fn resolver_row_23_bare_id_reaches_the_direct_vendor_row() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let found = pricing
            .explain(None, &[("deepseek-v3.2", MatchKind::StrictExact)])
            .unwrap();
        assert_eq!(found.step, super::ResolutionStep::AuthoritativeIdentity);
        assert_eq!(found.row_key, "deepseek/deepseek-v3.2");
    }

    #[test]
    fn conflicting_direct_vendor_rates_do_not_answer_by_vendor_id() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        assert_eq!(pricing.load_json(r#"{
            "openai/twin-1": {"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":1},
            "anthropic/twin-1": {"litellm_provider":"anthropic","input_cost_per_token":2,"output_cost_per_token":2}
        }"#), 2);
        assert!(
            pricing
                .resolve_at(None, &[("twin-1", MatchKind::Exact)], None)
                .is_none()
        );
        assert_eq!(pricing.find("openai/twin-1").unwrap().input, 1.0);
        assert_eq!(pricing.find("anthropic/twin-1").unwrap().input, 2.0);
    }

    #[test]
    fn overrides_with_the_same_identity_merge_fields() {
        let first = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(10e-6),
            ..Default::default()
        };
        let second = ccusage_cli::PricingOverride {
            output_cost_per_token: Some(20e-6),
            ..Default::default()
        };
        let pricing = PricingMap::default().with_user_overrides([
            (&"k-1.5".to_owned(), &first),
            (&"k-1-5".to_owned(), &second),
        ]);
        for model in ["k-1.5", "k-1-5"] {
            let p = pricing.find(model).unwrap();
            assert_eq!((p.input, p.output), (10e-6, 20e-6), "{model}");
        }
    }

    #[test]
    fn authoritative_rows_keep_the_embedded_tier_after_a_live_models_dev_load() {
        crate::test_support::install_pricing_snapshots();
        let mut primary = PricingMap::default();
        assert_eq!(primary.load_json(r#"{
            "gpt-5.5":{"litellm_provider":"openai","input_cost_per_token":6e-6,"output_cost_per_token":31e-6}
        }"#), 1);
        let mut live = PricingMap::default();
        assert_eq!(live.load_models_dev_json_for_tests(r#"{
            "live-only":{"cost":{"input":7,"output":9}},
            "gpt-5.5":{"cost":{"input":16,"output":31,"tiers":[{"input":20,"tier":{"type":"context","size":240000}}]}}
        }"#), Some(2));
        let pricing = layered(&primary, Some(embedded_models_dev_pricing()));
        assert_eq!(
            pricing
                .find_with_live("live-only", || Some(&live))
                .unwrap()
                .input,
            7e-6
        );
        let found = pricing.find("gpt-5.5").unwrap();
        assert_eq!(found.input, 6e-6);
        let tier = found.long_context.unwrap();
        assert_eq!(tier.threshold, 272_000);
        assert_eq!(tier.rates.input, Some(10e-6));
    }

    #[test]
    fn resolver_table() {
        use super::ResolutionStep::{
            AuthoritativeFuzzy, AuthoritativeIdentity, AuthoritativeSuffix, ModelsDevIdentity,
            ReportedOverride,
        };

        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
            "fixture-2.5": {"input_cost_per_token":2,"output_cost_per_token":3},
            "zai/desk-model-2.5": {"litellm_provider":"zai","input_cost_per_token":3,"output_cost_per_token":4},
            "catalogue/same-longer": {"input_cost_per_token":4,"output_cost_per_token":5},
            "standard/tiny-model-3": {"input_cost_per_token":5,"output_cost_per_token":6},
            "other/catalogue/tiny-model-3-preview": {"input_cost_per_token":6,"output_cost_per_token":7},
            "zai/free-niche-model": {"litellm_provider":"zai","input_cost_per_token":0,"output_cost_per_token":0},
            "deepseek/deepseek-v4-flash": {"litellm_provider":"deepseek","input_cost_per_token":1,"output_cost_per_token":2},
            "gpt-cache-fixture": {"input_cost_per_token":8,"output_cost_per_token":9},
            "override-base": {"litellm_provider":"zai","input_cost_per_token":9,"output_cost_per_token":10},
            "zai/override-base": {"litellm_provider":"zai","input_cost_per_token":10,"output_cost_per_token":11},
            "embedding-only": {"input_cost_per_token":11,"output_cost_per_token":0},
            "provider/embedding-with-output": {"input_cost_per_token":12,"output_cost_per_token":13}
        }"#,
        );
        let mut models_dev = PricingMap::default();
        models_dev
            .load_models_dev_json_for_tests(r#"{"provider/same":{"cost":{"input":7,"output":8}}}"#)
            .unwrap();
        let keys = ["reported-model".to_owned(), "override-base".to_owned()];
        let patch = ccusage_cli::PricingOverride {
            input_cost_per_token: Some(50.0),
            ..Default::default()
        };
        let pricing = layered(&pricing, Some(&models_dev))
            .with_user_overrides(keys.iter().map(|key| (key, &patch)));
        let fuzzy = MatchKind::Fuzzy;
        let strict = MatchKind::StrictExact;
        let cases = [
            (
                "R1_45_reported_override",
                "reported-model",
                &[("fixture-2.5", fuzzy)][..],
                ReportedOverride,
                (50.0, 0.0),
            ),
            (
                "R2_10_separator_identity",
                "fixture-2-5",
                &[("fixture-2-5", fuzzy)][..],
                AuthoritativeIdentity,
                (2.0, 3.0),
            ),
            (
                "R3_authoritative_path_suffix",
                "reseller/zai/desk-model-2-5",
                &[("reseller/zai/desk-model-2-5", fuzzy)][..],
                AuthoritativeSuffix,
                (3.0, 4.0),
            ),
            (
                "R4_73_later_identity_beats_fuzzy",
                "same",
                &[("same", fuzzy), ("provider/same", fuzzy)][..],
                ModelsDevIdentity,
                (7e-6, 8e-6),
            ),
            (
                "R5_71_fuzzy_segment",
                "tiny-model-3",
                &[("tiny-model-3", fuzzy)][..],
                AuthoritativeFuzzy,
                (5.0, 6.0),
            ),
            (
                "R6_55_free_final_segment",
                "foreign/free-niche-model",
                &[("foreign/free-niche-model", fuzzy)][..],
                AuthoritativeSuffix,
                (0.0, 0.0),
            ),
            (
                "R7_34_vendor_schedule",
                "deepseek-v4-flash",
                &[("deepseek-v4-flash", fuzzy)][..],
                AuthoritativeIdentity,
                (0.22e-6, 0.66e-6),
            ),
            (
                "R8_9_vendor_cache_default",
                "gpt-cache-fixture",
                &[("gpt-cache-fixture", fuzzy)][..],
                AuthoritativeIdentity,
                (8.0, 9.0),
            ),
            (
                "R9_48_override_inherits_same_model",
                "zai/override-base",
                &[("zai/override-base", fuzzy)][..],
                ReportedOverride,
                (50.0, 10.0),
            ),
            (
                "R10_94_zero_output_keeps_selected_row",
                "embedding-only",
                &[
                    ("embedding-only", strict),
                    ("provider/embedding-with-output", fuzzy),
                ][..],
                AuthoritativeIdentity,
                (11.0, 0.0),
            ),
        ];
        let instant = timestamp("2026-11-17T10:00:00Z");
        for (case, label, candidates, step, expected) in cases {
            let (_, rates) = pricing
                .resolve_at(Some(label), candidates, Some(instant))
                .expect(case);
            assert!(
                (rates.input - expected.0).abs() < 1e-12,
                "{case}: {rates:?}"
            );
            assert!(
                (rates.output - expected.1).abs() < 1e-12,
                "{case}: {rates:?}"
            );
            assert_eq!(
                pricing.explain(Some(label), candidates).expect(case).step,
                step,
                "{case}"
            );
        }
        assert_eq!(
            pricing
                .find("gpt-cache-fixture")
                .unwrap()
                .cache_creation_rate(),
            8.0,
            "R8_9_vendor_cache_default"
        );
    }

    #[test]
    fn preserves_a_fast_model_name_without_a_standard_sibling() {
        crate::test_support::install_pricing_snapshots();
        let entry = PricingMap::load_embedded()
            .find("qwen2.5-coder-7b-fast")
            .unwrap();
        assert_eq!((entry.input, entry.output), (0.00000003, 0.00000009));
    }

    #[test]
    fn context_limit_prefers_the_paid_host_listing_over_the_free_vendor_row() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (model, limit) in [
            ("@cf/zai-org/glm-4.7-flash", 131_072),
            ("cloudflare/@cf/zai-org/glm-4.7-flash", 131_072),
            ("z-ai/glm-4.7-flash", 202_752),
        ] {
            let rates = pricing.find(model).unwrap();
            assert!(rates.input > 0.0 && rates.output > 0.0, "{model}");
            assert_eq!(pricing.context_limit(model), Some(limit), "{model}");
        }
    }

    #[test]
    fn resolver_table_14_qualified_google_flash_uses_the_models_dev_rate() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let model = "google/gemini-2.5-flash";
        let entry = pricing.find(model).unwrap();
        assert_eq!((entry.input, entry.output), (0.0000003, 0.0000025));
        let resolved = pricing
            .explain(Some(model), &[(model, MatchKind::Fuzzy)])
            .unwrap();
        assert_eq!(resolved.step, super::ResolutionStep::ModelsDevIdentity);
    }

    #[test]
    fn fuzzy_match_requires_model_key_boundaries() {
        let mut pricing = PricingMap::default();
        pricing.insert_test_pricing(
            "claude-opus-4-7".to_string(),
            Pricing {
                input: 5e-6,
                output: 25e-6,
                cache_create: Some(6.25e-6),
                cache_read: Some(0.5e-6),
                ..Pricing::empty()
            },
        );
        pricing.insert_test_pricing(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: Some(18.75e-6),
                cache_read: Some(1.5e-6),
                ..Pricing::empty()
            },
        );

        assert!(pricing.find("claude-opus-4.70").is_none());
    }

    #[test]
    fn fuzzy_match_does_not_fall_back_across_numeric_model_versions() {
        let mut pricing = PricingMap::default();
        pricing.insert_test_pricing(
            "claude-opus-4".to_string(),
            Pricing {
                input: 15e-6,
                output: 75e-6,
                cache_create: Some(18.75e-6),
                cache_read: Some(1.5e-6),
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
        crate::test_support::install_pricing_snapshots();
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
        crate::test_support::install_pricing_snapshots();
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
        pricing.insert_test_pricing(
            "claude-sonnet-4".to_string(),
            Pricing {
                input: 1.0,
                output: 0.0,
                cache_create: Some(0.0),
                cache_read: Some(0.0),
                ..Pricing::empty()
            },
        );
        pricing.insert_test_pricing(
            "claude-sonnet-4-20250514".to_string(),
            Pricing {
                input: 2.0,
                output: 0.0,
                cache_create: Some(0.0),
                cache_read: Some(0.0),
                ..Pricing::empty()
            },
        );

        let matched = pricing.find("claude-sonnet-4-20250514-v1:0").unwrap();

        assert_eq!(matched.input, 2.0);
    }

    #[test]
    fn an_unlisted_id_that_extends_a_listed_id_with_a_model_word_is_unpriced() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for model in [
            "gpt-5.4-codex",
            "gpt-5.3-codex-mini",
            "gpt-5.3-codex-max",
            "gpt-6.2-sol-mini",
            "gemini-3.8-flash-lite",
            "gemini-4-argon-lite",
            "kimi-k3-code",
            "kimi-k3-pro",
            "glm-5.3-turbo",
            "glm-5.3-air",
            "glm-5.3-code",
            "llama-4-scout-2",
            "claude-sonnet-5-5-fast",
            "claude-fable-5-1-fast",
            "deepseek-v4-flash-0812",
        ] {
            assert_eq!(pricing.find(model), None, "{model}");
        }
    }

    #[test]
    fn a_model_word_in_front_of_a_listed_id_names_another_model() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for model in ["chat-gpt-5.4", "my-gpt-5.4-high", "gemini-3.5-flash-extra"] {
            assert_eq!(pricing.find(model), None, "{model}");
        }
    }

    #[test]
    fn a_word_or_version_after_a_live_key_does_not_borrow_its_row() {
        crate::test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::load_embedded();
        pricing.refresh_litellm(|load| {
            assert!(load(
                r#"{
                "mistral/mistral-small": {"litellm_provider":"mistral","input_cost_per_token":1e-7,"output_cost_per_token":3e-7},
                "mistral/mistral-medium": {"litellm_provider":"mistral","input_cost_per_token":1.5e-6,"output_cost_per_token":7.5e-6},
                "xai/grok-code-fast": {"litellm_provider":"xai","input_cost_per_token":1e-6,"output_cost_per_token":2e-6}
            }"#
            ));
            Ok(())
        });
        for model in [
            "mistral/mistral-small-5",
            "mistral/mistral-medium-4",
            "xai/grok-code-fast-2",
        ] {
            assert_eq!(pricing.find(model), None, "{model}");
        }
        let listed = pricing.find("mistral/mistral-small-4").unwrap();
        assert_eq!((listed.input, listed.output), (0.15e-6, 0.6e-6));
    }

    #[test]
    fn an_unlisted_id_whose_only_matches_are_longer_ids_is_unpriced() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for model in [
            "gpt-6.2",
            "openai/gpt-6.2",
            "gpt-6",
            "o4",
            "kimi-k2.8",
            "deepseek-v4.1",
            "qwen2.5-coder-7b",
            "opus",
        ] {
            assert_eq!(pricing.find(model), None, "{model}");
        }
    }

    #[test]
    fn price_neutral_suffixes_keep_the_listed_rate() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (model, listed) in [
            ("o4-mini-high", "o4-mini"),
            ("gpt-5.4-xhigh", "gpt-5.4"),
            ("gpt-5.5-2026-11-01", "gpt-5.5"),
            ("claude-opus-4-8[1m]", "claude-opus-4-8"),
            ("claude-opus-4-8-20261101-v1:0", "claude-opus-4-8"),
            ("vertex_ai/claude-opus-4-7@default", "claude-opus-4-7"),
            ("grok-4.5-latest", "grok-4.5"),
            ("gemini-3.5-flash-extra-low", "gemini-3.5-flash"),
            ("gemini-3.5-flash-extra-high", "gemini-3.5-flash"),
            ("host/gpt-5.4-high", "gpt-5.4"),
            ("host.gpt-5-4-high", "gpt-5.4"),
        ] {
            let found = pricing.find(model).unwrap_or_else(|| panic!("{model}"));
            let expected = pricing.find(listed).unwrap();
            assert_eq!(
                (found.input, found.output),
                (expected.input, expected.output),
                "{model}"
            );
        }
    }

    #[test]
    fn split_threshold_comes_from_the_resolved_pricing_entry() {
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "private-boundary",
            "custom-boundary",
        )]);
        let mut map = PricingMap::default();
        map.insert_test_pricing(
            "custom-boundary".into(),
            Pricing {
                input: 1e-6,
                output: 2e-6,
                long_context: Some(super::LongContext {
                    threshold: 123_456,
                    mode: super::LongContextMode::WholeRequest,
                    rates: crate::pricing::LongContextRates::default(),
                }),
                ..Pricing::empty()
            },
        );
        assert_eq!(
            map.find("private-boundary")
                .unwrap()
                .long_context_split_threshold(),
            123_456
        );
        assert_eq!(
            map.find("custom-boundary-high")
                .unwrap()
                .long_context_split_threshold(),
            123_456
        );
        let embedded = PricingMap::load_embedded();
        assert_eq!(
            embedded
                .find("gpt-5-4")
                .unwrap()
                .long_context_split_threshold(),
            272_000
        );
        assert_eq!(Pricing::empty().long_context_split_threshold(), 200_000);
    }

    #[test]
    fn recorded_threshold_migration_groups_use_the_resolved_model_boundary() {
        crate::test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        for (model, expected) in [
            ("GLM-5-1", 32_000),
            ("anthropic-claude-5-5-haiku", 100_000),
            ("spacexai/grok-4-5", 200_001),
            ("qwen-3-6-plus-high", 256_000),
            ("alibaba/qwen3-5-plus", 256_001),
            ("aihubmix/gpt-5-4-high", 272_000),
            ("minimax-m-3-0-high", 512_000),
        ] {
            assert_eq!(
                pricing.find(model).unwrap().long_context_split_threshold(),
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn aliased_tier_id_does_not_take_the_base_models_limit() {
        crate::test_support::install_pricing_snapshots();
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
            "claude-opus-5-turbo",
            "claude-opus-5-fast",
        )]);
        let mut secondary = PricingMap::default();
        assert_eq!(
            secondary.load_models_dev_json_for_tests(
                r#"{
            "claude-opus-5":{"cost":{"input":3,"output":4},"limit":{"context":1000000}},
            "claude-opus-5-fast":{"cost":{"input":6,"output":8},"exactOnly":true}
        }"#
            ),
            Some(2)
        );
        let pricing = layered(&PricingMap::default(), Some(&secondary));
        assert_eq!(
            pricing.context_limit_entry("claude-opus-5", MatchKind::Fuzzy),
            Some(1_000_000)
        );
        assert_eq!(
            pricing.context_limit_entry("claude-opus-5-turbo", MatchKind::Fuzzy),
            None
        );
        assert_eq!(
            pricing
                .find_with_live("claude-opus-5-turbo", || None)
                .unwrap()
                .input,
            6e-6
        );
    }

    #[test]
    fn context_limits_keep_their_pre_wave_exact_only_boundaries() {
        crate::test_support::install_pricing_snapshots();
        let map = PricingMap::load_embedded();
        assert_eq!(map.context_limit("qwen_ai_platform/glm-5.1"), Some(200_000));
        assert_eq!(map.context_limit("novita/zai-org/glm-5-turbo"), None);
    }

    #[test]
    fn live_exact_only_rows_without_limits_keep_embedded_tiers_out_of_fuzzy_limits() {
        crate::test_support::install_pricing_snapshots();
        let mut embedded = PricingMap::default();
        embedded.load_models_dev_json_for_tests(
            r#"{"sample-fast":{"exactOnly":true,"cost":{"input":1,"output":2},"limit":{"context":123456}}}"#,
        );
        let pricing = layered(&PricingMap::default(), Some(&embedded));
        let mut live = PricingMap::default();
        live.load_models_dev_json_for_tests(
            r#"{"sample-fast":{"exactOnly":true,"cost":{"input":3,"output":4}}}"#,
        );
        let pricing = pricing.with_live_layer("sample-fast-extra", Lookup::Context, || Some(&live));
        assert!(pricing.find("sample-fast-extra").is_none());
        assert_eq!(pricing.context_limit("sample-fast-extra"), None);
        assert_eq!(pricing.context_limit("sample-fast"), None);
    }

    #[test]
    fn cache_read_fallback_is_independent_of_cache_write_family() {
        crate::test_support::install_pricing_snapshots();
        for model in ["gpt-5.3-codex-xhigh", "auto/openai/gpt-oss-120b"] {
            let pricing = PricingMap::load_embedded()
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(pricing.cache_read, None, "{model}");
            assert_eq!(
                pricing.cache_read_rate_at(pricing.input),
                pricing.input * 0.1,
                "{model}"
            );
            assert_eq!(pricing.cache_creation_rate(), pricing.input, "{model}");
        }
    }

    #[test]
    fn model_vendor_precedence_is_independent_of_segment_order() {
        use super::{cache_defaults, data};
        for model in ["claude-proxy/openai.gpt-5", "openai.gpt-5/claude-proxy"] {
            assert_eq!(
                cache_defaults(model, Some("anthropic")),
                &data::WRITES_AT_INPUT_RATE
            );
        }
        assert_eq!(
            cache_defaults("proxy/anthropic.claude-sonnet-4", Some("openai")),
            &data::WRITES_SURCHARGED
        );
        assert_eq!(
            cache_defaults("proxy/openai-claude-sonnet", None),
            &data::WRITES_SURCHARGED
        );
    }

    #[test]
    fn hosted_openai_model_names_keep_openai_cache_write_defaults() {
        crate::test_support::install_pricing_snapshots();
        for (model, provider) in [
            ("bedrock_mantle/openai.gpt-5.4", "bedrock_mantle"),
            ("global.openai.gpt-5.4", "bedrock_converse"),
            ("us.openai.gpt-5.5", "bedrock_converse"),
            ("oci/openai.gpt-5", "oci"),
            ("snowflake/openai-gpt-5", "snowflake"),
            ("azure_ai/codex-mini", "azure_ai"),
        ] {
            let mut map = PricingMap::default();
            map.load_json(
                &serde_json::json!({ model: {
                    "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6,
                    "litellm_provider": provider,
                }})
                .to_string(),
            );
            let pricing = map
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(pricing.cache_creation_rate(), pricing.input, "{model}");
            assert_eq!(
                pricing.one_hour_cache_creation_rate(),
                pricing.input,
                "{model}"
            );
        }
    }

    #[test]
    fn respelled_override_applies_to_the_models_dev_exact_only_row() {
        crate::test_support::install_pricing_snapshots();
        use crate::cli::PricingOverride;
        let original = "kimi-k2.7-code-highspeed";
        let key = "kimi-k2-7-code-highspeed".to_string();
        let baseline = PricingMap::load_embedded()
            .resolve_at(None, &[(original, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        let value = PricingOverride {
            input_cost_per_token: Some(7e-6),
            ..PricingOverride::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&key, &value)]);
        for resolved in [
            pricing.find(original),
            pricing
                .resolve_at(None, &[(original, MatchKind::Exact)], None)
                .map(|(_, rates)| rates),
            pricing
                .resolve_at(None, &[(original, MatchKind::Exact)], None)
                .map(|(_, pricing)| pricing),
        ] {
            let resolved = resolved.unwrap();
            assert_eq!(resolved.input, 7e-6);
            assert_eq!(resolved.output, baseline.output);
            assert_eq!(resolved.cache_read, baseline.cache_read);
        }
        assert_eq!(
            pricing
                .resolve_at(None, &[(&key, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .input,
            7e-6
        );
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
            assert_eq!(entry.fast_multiplier, 2.0);
            assert_eq!(pricing.context_limit("custom-model"), Some(123_456));
        }

        #[test]
        fn override_only_openai_models_charge_cache_writes_at_input_rate() {
            let input_rate = 1e-6;
            for model in ["gpt-5.5-private", "[pi] gpt-5.5-private"] {
                let mut pricing = PricingMap::default();
                let overrides = build_overrides(model, |value| {
                    value.input_cost_per_token = Some(input_rate);
                });
                pricing.apply_overrides(overrides.iter());

                let entry = pricing.find(model).unwrap();
                assert_eq!(entry.cache_creation_rate(), input_rate, "{model}");
                assert_eq!(entry.one_hour_cache_creation_rate(), input_rate, "{model}");
            }
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
            assert_eq!(
                entry.long_context.and_then(|context| context.rates.input),
                sol.long_context.and_then(|context| context.rates.input)
            );
            assert_eq!(
                entry.long_context.and_then(|context| context.rates.output),
                sol.long_context.and_then(|context| context.rates.output)
            );
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_create),
                sol.long_context
                    .and_then(|context| context.rates.cache_create)
            );
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_read),
                sol.long_context
                    .and_then(|context| context.rates.cache_read)
            );
            assert_eq!(
                entry.long_context.map(|context| context.threshold),
                sol.long_context.map(|context| context.threshold)
            );
            assert_eq!(entry.fast_multiplier, sol.fast_multiplier);
            assert_eq!(pricing.context_limit("gpt-5.6"), Some(654_321));
        }

        #[test]
        fn partial_override_preserves_existing_fields() {
            let mut pricing = PricingMap::default();
            pricing.insert_test_pricing(
                "existing".to_string(),
                Pricing {
                    input: 10e-6,
                    output: 20e-6,
                    cache_create: Some(30e-6),
                    cache_read: Some(40e-6),
                    long_context: crate::pricing::LongContextRates {
                        input: Some(15e-6),
                        ..Default::default()
                    }
                    .marginal(),
                    fast_multiplier: 1.5,
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
            assert_eq!(
                entry.long_context.and_then(|context| context.rates.input),
                Some(15e-6)
            );
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
            pricing.insert_test_limit("model", 500_000);
            let overrides = build_overrides("model", |o| {
                o.input_cost_per_token = Some(1e-6);
            });
            pricing.apply_overrides(overrides.iter());
            assert_eq!(pricing.context_limit("model"), Some(500_000));
        }

        #[test]
        fn input_override_recomputes_absent_cache_rates_and_preserves_published_tiers() {
            let mut pricing = PricingMap::default();
            pricing.insert_test_pricing(
                "claude-model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    long_context: crate::pricing::LongContextRates {
                        cache_create: Some(4.6875e-6),
                        cache_read: Some(3.75e-7),
                        ..Default::default()
                    }
                    .marginal(),
                    ..Pricing::empty()
                },
            );

            let overrides = build_overrides("claude-model", |o| {
                o.input_cost_per_token = Some(2e-6);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("claude-model").unwrap();
            assert_eq!(entry.input, 2e-6);
            assert_eq!(entry.output, 15e-6);
            assert_eq!(entry.cache_create, None);
            assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
            assert_eq!(entry.cache_read, None);
            assert!((entry.cache_read_rate_at(entry.input) - 2e-7).abs() < 1e-15);
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_create),
                Some(4.6875e-6)
            );
            assert_eq!(
                entry
                    .long_context
                    .and_then(|context| context.rates.cache_read),
                Some(3.75e-7)
            );
        }

        #[test]
        fn input_override_does_not_scale_zero_cache() {
            let mut pricing = PricingMap::default();
            pricing.insert_test_pricing(
                "no-cache-model".to_string(),
                Pricing {
                    input: 5e-6,
                    output: 10e-6,
                    cache_create: Some(0.0),
                    cache_read: Some(0.0),
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
            pricing.insert_test_pricing(
                "model".to_string(),
                Pricing {
                    input: 3e-6,
                    output: 15e-6,
                    ..Pricing::empty()
                },
            );

            let overrides = build_overrides("model", |o| {
                o.input_cost_per_token = Some(2e-6);
                o.cache_read_input_token_cost = Some(5e-7);
            });
            pricing.apply_overrides(overrides.iter());

            let entry = pricing.find("model").unwrap();
            assert_eq!(entry.input, 2e-6);
            assert_eq!(entry.cache_read, Some(5e-7));
            assert_eq!(entry.cache_create, None);
            assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
        }
    }
}
