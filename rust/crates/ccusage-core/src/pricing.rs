use std::sync::{Arc, OnceLock};

use ccusage_cli::PricingOverride;

use crate::fast::FxHashSet;

mod builtin_pricing;
mod cache;
use cache::{CachedHit, Candidate, HitKind, Name, NameCache, Pass, RequestCache, RequestHit};
mod data;
mod embedded;
#[cfg(test)]
use embedded::build_time_pricing_json;
use embedded::embedded_pricing_table;
pub use embedded::{EmbeddedSnapshots, set_embedded_snapshots};
mod fetch;
pub use fetch::{JsonFetcher, set_json_fetcher};
use fetch::{fetch_pricing_json, models_dev_pricing};
mod keys;
use keys::{
    dash_pricing_key_separators, is_pricing_key_separator, last_segment, matches_model_suffix,
    model_without_date_suffix, names_pricing_key, normalized_pricing_key, pricing_alias,
    pricing_key_matches, unlabeled_model, with_names,
};
mod litellm;
mod models_dev;
use models_dev::embedded_models_dev_pricing;
mod overrides;
use overrides::Overrides;
mod rates;
use rates::{CacheDefaults, PricingSchedule, ScheduledRates, Successor, rates};
pub use rates::{
    DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS, LongContext, LongContextMode, LongContextRates, Pricing,
};
mod resolve;
pub use resolve::{
    AgentModel, BillingRoute, MatchKind, PricedIdentity, PricingHandle, direct_billing_vendor,
};
#[cfg(feature = "resolver-trace")]
pub use resolve::{Explanation, ResolutionStep};
mod row;
pub use row::Source;
use row::{
    Entries, Row, RowPricing, Scope, provider_cache_defaults, row_scope, schedule_for, vendor_id,
    vendor_start,
};
mod rows;
use rows::{Lookup, PricingRows, RowMatch};
mod speed;
use speed::SpeedMultipliers;
mod table;
pub use table::encode_pricing_table;

use builtin_pricing::{BUILTIN_PRICING, CURATED_LIST_PRICES};

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
        let table = embedded_pricing_table();
        Self {
            rows: Arc::clone(&table.rows.rows),
            models_dev: Some(Arc::clone(&table.models_dev.rows)),
            ..Self::default()
        }
    }

    fn complete_layers(&mut self, embedded_models_dev: &Self) {
        self.put_builtin_pricing();
        self.put_curated_list_prices();
        self.curate_long_context();
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

    fn clear_find_cache(&mut self) {
        self.find_cache.take();
        self.requests.take();
        self.override_shadows.take();
    }

    fn supplement_row(
        &mut self,
        model: &str,
        pricing: Pricing,
        context_limit: Option<u64>,
        provider: &str,
    ) -> Option<&mut Row> {
        let pricing = SpeedMultipliers::get().for_model(model).applied_to(pricing);
        self.clear_find_cache();
        let row = self
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
            })?;
        row.context_limit = row.context_limit.or(context_limit);
        Some(row)
    }

    fn supplement_pricing(
        &mut self,
        model: &str,
        pricing: Pricing,
        context_limit: Option<u64>,
        provider: &str,
    ) {
        let Some(row) = self.supplement_row(model, pricing, context_limit, provider) else {
            return;
        };
        row.pricing.cache_read = row.pricing.cache_read.or(pricing.cache_read);
        row.pricing.cache_create = row.pricing.cache_create.or(pricing.cache_create);
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

    fn put_curated_list_prices(&mut self) {
        for supplement in CURATED_LIST_PRICES {
            let curated = supplement.pricing;
            let Some(row) = self.supplement_row(
                supplement.model,
                curated,
                supplement.context_limit,
                supplement.provider,
            ) else {
                continue;
            };
            row.pricing.input = curated.input;
            row.pricing.output = curated.output;
            row.pricing.cache_read = curated.cache_read;
            row.pricing.cache_create = row.pricing.cache_create.or(curated.cache_create);
            row.source = Source::Curated;
            row.scope = Scope::Direct;
        }
    }
}

fn curated_long_context(key: &str) -> Option<&'static data::CuratedLongContext> {
    let segment = last_segment(key);
    let model = &segment[segment.find("claude-")?..];
    let model = model.split_once(':').map_or(model, |(model, _)| model);
    let model = model
        .rsplit_once("-v")
        .filter(|(_, version)| !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()))
        .map_or(model, |(model, _)| model);
    let model = normalized_pricing_key(model);
    let model = data::FAMILY_PRICE_NEUTRAL_SUFFIX_WORDS
        .iter()
        .filter(|(family, _)| model.starts_with(family))
        .flat_map(|(_, words)| words.iter())
        .find_map(|word| {
            model
                .strip_suffix(word)
                .and_then(|model| model.strip_suffix('-'))
        })
        .unwrap_or(&model);
    let model = model_without_date_suffix(model);
    data::ANTHROPIC_LONG_CONTEXT
        .iter()
        .find_map(|(id, curated)| (*id == model).then_some(curated))
}

impl PricingMap {
    fn curate_long_context(&mut self) {
        for row in &mut self.rows_mut().entries.rows {
            let Some(curated) = curated_long_context(&row.key) else {
                continue;
            };
            if (row.pricing.input - curated.input).abs() < f64::EPSILON
                && (row.pricing.output - curated.output).abs() < f64::EPSILON
            {
                row.pricing.long_context = curated.long_context.map(Arc::new);
                if let Some(schedule) = curated.schedule.and_then(schedule_for) {
                    row.schedule = Some(schedule);
                }
            }
        }
        self.clear_find_cache();
    }
}

struct Supplement {
    provider: &'static str,
    model: &'static str,
    pricing: Pricing,
    context_limit: Option<u64>,
}

#[cfg(test)]
mod tests;
