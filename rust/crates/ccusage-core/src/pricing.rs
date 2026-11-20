use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

use ccusage_cli::PricingOverride;

use crate::{
    MILLIS_PER_DAY, MILLIS_PER_HOUR, TimestampMs,
    fast::{FxHashMap, FxHashSet},
};

mod builtin_pricing;
mod cache;
use cache::{CachedHit, Candidate, HitKind, Name, NameCache, Pass, RequestCache, RequestHit};
mod data;
mod embedded;
use embedded::build_time_pricing_json;
pub use embedded::{EmbeddedSnapshots, set_embedded_snapshots};
mod fetch;
pub use fetch::{JsonFetcher, set_json_fetcher};
use fetch::{fetch_pricing_json, models_dev_pricing};
mod litellm;
mod models_dev;
use models_dev::embedded_models_dev_pricing;
mod overrides;
use overrides::Overrides;
mod row;
pub use row::Source;
use row::{Entries, Row, Scope, cache_defaults, row_scope, schedule_for, vendor_id, vendor_start};
mod speed;
use speed::SpeedMultipliers;

use builtin_pricing::BUILTIN_PRICING;

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
    Fallback,
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
        let has_fallback = candidates
            .iter()
            .any(|&(_, kind)| kind == MatchKind::Fallback);
        for fallback in [false, true] {
            if fallback && !has_fallback {
                break;
            }
            for pass in [Pass::S2, Pass::S34, Pass::S3f, Pass::S56] {
                if matches!((lookup, pass), (Lookup::Context, Pass::S3f)) {
                    continue;
                }
                for (index, &(model, kind)) in candidates.iter().enumerate() {
                    if (kind == MatchKind::Fallback) == fallback
                        && pass.allows(kind)
                        && let Some(row) = matching(model, pass)
                    {
                        return Some(RequestHit {
                            candidate: Some(index),
                            row,
                        });
                    }
                }
            }
        }
        None
    }

    fn models_dev_names<'a, 'n>(
        &'a self,
        name: Name<'n>,
        lookup: Lookup,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> impl Iterator<Item = Name<'n>> {
        // NOTE(2026-11-20, #2238): An authoritative row for an id behind the path outranks the models.dev row for it. After S3 only a vendor's free row can be left there, and it must still answer through S3f.
        let unclaimed = move |suffix: &Name<'_>| {
            self.authoritative(*suffix, lookup, false, models_dev, |_| true)
                .is_none()
        };
        std::iter::once(name).chain(name.suffixes().take_while(unclaimed))
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
                    slot.names(model, |name| {
                        self.models_dev_names(name, lookup, models_dev)
                            .find_map(|name| rows.exact(name, lookup))
                    })
                    .map(|found| CachedHit::new(found, HitKind::ModelsDevIdentity))
                })
            }
            Pass::S56 => {
                let secondary = models_dev();
                if let Some(rows) = secondary
                    && slot
                        .names(model, |name| {
                            self.models_dev_names(name, lookup, models_dev)
                                .any(|name| {
                                    rows.contains_exact_only_identity(name.raw(), name.identity())
                                })
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
            lookup.includes(found.row)
                && (!found.row.is_free()
                    || (matches!(lookup, Lookup::Context) && found.row.scope == Scope::Direct))
                && if found.row.exact_only {
                    names_pricing_key(key, identity, name, normalized)
                } else {
                    pricing_key_matches(key, identity, name, normalized)
                }
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

fn names_pricing_key(
    candidate: &str,
    normalized_candidate: &str,
    model: &str,
    normalized_model: &str,
) -> bool {
    contains_pricing_key(model, model, candidate)
        || contains_pricing_key(normalized_model, model, normalized_candidate)
}

// NOTE(2026-11-20, #2236): Either id may carry only a host or provider qualifier in front of the other, so a word in front, as in `chat-gpt-5.4` for `gpt-5.4`, makes it another model. The qualifier is read from the raw id because the identity spells `openai.` as `openai-`. Only the logged id may add a price-neutral suffix: a catalogue lists a dated, `[1m]` or effort-level id because it prices that variant itself.
fn names_same_model((model, raw_model): (&str, &str), (key, raw_key): (&str, &str)) -> bool {
    if model.len() >= key.len() {
        contains_pricing_key(model, raw_model, key)
    } else {
        key.strip_suffix(model).is_some_and(|qualifier| {
            raw_key
                .as_bytes()
                .get(qualifier.len() - 1)
                .copied()
                .is_some_and(is_qualifier_end)
        })
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

#[cfg(test)]
mod tests;
