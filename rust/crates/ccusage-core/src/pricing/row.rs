use std::{num::NonZeroUsize, sync::Arc};

use rustc_hash::FxHashMap;

use super::{CacheDefaults, LongContext, Pricing, PricingSchedule, data, normalized_pricing_key};

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub(super) key: Arc<str>,
    pub(super) pricing: RowPricing,
    pub(super) source: Source,
    pub(super) schedule: Option<&'static PricingSchedule>,
    pub(super) context_limit: Option<u64>,
    pub(super) exact_only: bool,
    pub(super) scope: Scope,
    pub(super) vendor_start: Option<NonZeroUsize>,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            key: Arc::from(""),
            pricing: Pricing::empty().into(),
            source: Source::LiteLlm,
            schedule: None,
            context_limit: None,
            exact_only: false,
            scope: Scope::Hosted,
            vendor_start: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowPricing {
    input: f64,
    output: f64,
    pub(super) cache_create: Option<f64>,
    pub(super) cache_read: Option<f64>,
    cache_create_1h: Option<f64>,
    pub(super) long_context: Option<Arc<LongContext>>,
    fast_multiplier: f64,
    ultrafast_multiplier: f64,
    flex_multiplier: f64,
    cache_defaults: &'static CacheDefaults,
    explicit_fields: u16,
}

impl From<Pricing> for RowPricing {
    fn from(rates: Pricing) -> Self {
        Self {
            input: rates.input,
            output: rates.output,
            cache_create: rates.cache_create,
            cache_read: rates.cache_read,
            cache_create_1h: rates.cache_create_1h,
            long_context: rates.long_context.map(Arc::new),
            fast_multiplier: rates.fast_multiplier,
            ultrafast_multiplier: rates.ultrafast_multiplier,
            flex_multiplier: rates.flex_multiplier,
            cache_defaults: rates.cache_defaults,
            explicit_fields: rates.explicit_fields,
        }
    }
}

impl RowPricing {
    pub(super) fn rates(&self) -> Pricing {
        Pricing {
            input: self.input,
            output: self.output,
            cache_create: self.cache_create,
            cache_read: self.cache_read,
            cache_create_1h: self.cache_create_1h,
            long_context: self.long_context.as_deref().copied(),
            fast_multiplier: self.fast_multiplier,
            ultrafast_multiplier: self.ultrafast_multiplier,
            flex_multiplier: self.flex_multiplier,
            cache_defaults: self.cache_defaults,
            explicit_fields: self.explicit_fields,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Override,
    LiteLlm,
    Curated,
    ModelsDev,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Direct,
    Hosted,
}

impl Row {
    pub(super) fn is_free(&self) -> bool {
        self.source != Source::Override && self.pricing.input == 0.0 && self.pricing.output == 0.0
    }

    pub(super) fn vendor_id(&self) -> Option<&str> {
        self.key.get(self.vendor_start?.get() - 1..)
    }

    pub(super) fn is_bare_hosted(&self, key: &str) -> bool {
        self.scope == Scope::Hosted && !key.contains('/')
    }
}

pub(super) fn vendor_id<'a>(key: &'a str, provider: &str) -> &'a str {
    key.strip_prefix(provider)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .unwrap_or(key)
}

pub(super) fn row_scope<'a>(
    key: &'a str,
    provider: &str,
    direct: &[String],
) -> (Scope, Option<&'a str>) {
    if direct.iter().any(|id| id == provider) {
        (Scope::Direct, Some(vendor_id(key, provider)))
    } else {
        (Scope::Hosted, None)
    }
}

pub(super) fn vendor_start(key: &str, vendor_id: &str) -> Option<NonZeroUsize> {
    NonZeroUsize::new(key.len() - vendor_id.len() + 1)
}

pub(super) fn cache_defaults(model: &str, provider: Option<&str>) -> &'static CacheDefaults {
    use data::{
        CACHE_DEFAULTS_BY_PROVIDER, CACHE_DEFAULTS_BY_SEGMENT, CACHE_DEFAULTS_BY_SEGMENT_PREFIX,
    };
    model
        .split('/')
        .flat_map(|part| {
            std::iter::once(part)
                .chain(part.match_indices('.').map(|(index, _)| &part[index + 1..]))
        })
        .filter_map(|suffix| {
            let suffix = normalized_pricing_key(suffix);
            let suffix = suffix.as_bytes();
            CACHE_DEFAULTS_BY_SEGMENT_PREFIX
                .iter()
                .find(|(prefix, _)| {
                    suffix
                        .get(..prefix.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
                })
                .or_else(|| {
                    CACHE_DEFAULTS_BY_SEGMENT
                        .iter()
                        .find(|(key, _)| suffix.eq_ignore_ascii_case(key.as_bytes()))
                })
                .map(|(key, defaults)| (key.len(), *defaults))
        })
        .min_by(|(left_len, left), (right_len, right)| {
            left.write_x
                .total_cmp(&right.write_x)
                .then_with(|| right_len.cmp(left_len))
        })
        .map(|(_, defaults)| defaults)
        .or_else(|| {
            CACHE_DEFAULTS_BY_PROVIDER
                .iter()
                .find(|(key, _)| Some(*key) == provider)
                .map(|(_, defaults)| *defaults)
        })
        // NOTE(2026-11-20, #2258): No vendor documents rates for an unidentified model. Agents report cache writes mostly through Anthropic-compatible APIs, so Anthropic's multipliers are the estimate.
        .unwrap_or(&data::WRITES_SURCHARGED)
}

pub(super) fn schedule_for(model: &str) -> Option<&'static PricingSchedule> {
    let identity = normalized_pricing_key(model);
    data::SCHEDULES
        .iter()
        .find_map(|(id, rates)| (*id == identity).then_some(rates))
}

#[derive(Clone, Debug, Default)]
pub(super) struct Entries {
    pub(super) rows: Vec<Row>,
    literal: FxHashMap<Arc<str>, usize>,
}

impl Entries {
    pub(super) fn contains_key(&self, key: &str) -> bool {
        self.literal.contains_key(key)
    }

    pub(super) fn id(&self, key: &str) -> Option<usize> {
        self.literal.get(key).copied()
    }

    pub(super) fn get(&self, key: &str) -> Option<&Row> {
        self.rows.get(self.id(key)?)
    }

    pub(super) fn insert(&mut self, key: String, mut row: Row) {
        if let Some(existing) = self.id(&key).and_then(|id| self.rows.get_mut(id)) {
            row.key = Arc::clone(&existing.key);
            *existing = row;
        } else {
            self.append(key, row);
        }
    }

    pub(super) fn get_or_insert(
        &mut self,
        key: String,
        create: impl FnOnce() -> Row,
    ) -> Option<&mut Row> {
        match self.id(&key) {
            Some(id) => self.rows.get_mut(id),
            None => Some(self.append(key, create())),
        }
    }

    fn append(&mut self, key: String, mut row: Row) -> &mut Row {
        let id = self.rows.len();
        row.key = Arc::from(key);
        self.literal.insert(Arc::clone(&row.key), id);
        self.rows.push_mut(row)
    }

    pub(super) fn remove(&mut self, key: &str) {
        if let Some(id) = self.literal.remove(key) {
            self.rows.swap_remove(id);
            if let Some(row) = self.rows.get(id)
                && let Some(index) = self.literal.get_mut(&row.key)
            {
                *index = id;
            }
        }
    }
}
