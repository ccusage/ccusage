use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

use rustc_hash::FxHashMap;

use super::{
    CacheDefaults, LongContext, Pricing, PricingSchedule, SpeedMultipliers, data,
    normalized_pricing_key, speed::PublishedSpeed,
};

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
    pub(super) host: Option<u8>,
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
            host: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RowPricing {
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_create: Option<f64>,
    pub(super) cache_read: Option<f64>,
    pub(super) cache_create_1h: Option<f64>,
    pub(super) long_context: Option<Arc<LongContext>>,
    pub(super) published: Published,
    pub(super) explicit_fields: u16,
}

#[derive(Clone, Debug)]
pub(super) enum Published {
    Known(PublishedRates),
    ByKey {
        speed: PublishedSpeed,
        cache_fallback: &'static CacheDefaults,
        resolved: OnceLock<PublishedRates>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PublishedRates {
    pub(super) fast_multiplier: f64,
    pub(super) ultrafast_multiplier: f64,
    pub(super) flex_multiplier: f64,
    pub(super) cache_defaults: &'static CacheDefaults,
}

impl From<Pricing> for RowPricing {
    fn from(rates: Pricing) -> Self {
        Self::with_published(
            rates,
            Published::Known(PublishedRates {
                fast_multiplier: rates.fast_multiplier,
                ultrafast_multiplier: rates.ultrafast_multiplier,
                flex_multiplier: rates.flex_multiplier,
                cache_defaults: rates.cache_defaults,
            }),
        )
    }
}

impl RowPricing {
    // NOTE(2026-11-20, #2342): A catalogue holds thousands of rows and a report prices only a few. Derive speed multipliers and cache defaults from the key when a row is first priced, not while loading.
    pub(super) fn by_key(
        rates: Pricing,
        speed: PublishedSpeed,
        cache_fallback: &'static CacheDefaults,
    ) -> Self {
        Self::with_published(
            rates,
            Published::ByKey {
                speed,
                cache_fallback,
                resolved: OnceLock::new(),
            },
        )
    }

    fn with_published(rates: Pricing, published: Published) -> Self {
        Self {
            input: rates.input,
            output: rates.output,
            cache_create: rates.cache_create,
            cache_read: rates.cache_read,
            cache_create_1h: rates.cache_create_1h,
            long_context: rates.long_context.map(Arc::new),
            published,
            explicit_fields: rates.explicit_fields,
        }
    }

    fn rates(&self, key: &str) -> Pricing {
        let published = match &self.published {
            Published::Known(published) => *published,
            Published::ByKey {
                speed,
                cache_fallback,
                resolved,
            } => *resolved.get_or_init(|| {
                let speed = speed.or(SpeedMultipliers::get().for_model(key));
                PublishedRates {
                    fast_multiplier: speed.fast.unwrap_or(1.0),
                    ultrafast_multiplier: speed.ultrafast.unwrap_or(1.0),
                    flex_multiplier: speed.flex.unwrap_or(1.0),
                    cache_defaults: model_cache_defaults(key).unwrap_or(cache_fallback),
                }
            }),
        };
        Pricing {
            input: self.input,
            output: self.output,
            cache_create: self.cache_create,
            cache_read: self.cache_read,
            cache_create_1h: self.cache_create_1h,
            long_context: self.long_context.as_deref().copied(),
            fast_multiplier: published.fast_multiplier,
            ultrafast_multiplier: published.ultrafast_multiplier,
            flex_multiplier: published.flex_multiplier,
            cache_defaults: published.cache_defaults,
            explicit_fields: self.explicit_fields,
            flat_long_context_in_fast_mode: false,
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
    pub(super) fn rates(&self) -> Pricing {
        self.pricing.rates(&self.key)
    }

    pub(super) fn is_free(&self) -> bool {
        self.source != Source::Override && self.pricing.input == 0.0 && self.pricing.output == 0.0
    }

    pub(super) fn vendor_id(&self) -> Option<&str> {
        self.key.get(self.vendor_start?.get() - 1..)
    }

    pub(super) fn is_bare_hosted(&self, key: &str) -> bool {
        self.scope == Scope::Hosted && !key.contains('/')
    }

    pub(super) fn is_bedrock(&self) -> bool {
        self.host == Some(data::BEDROCK_HOST)
            || (self.scope == Scope::Hosted
                && self.key.split_once('/').is_some_and(|(prefix, _)| {
                    prefix == "bedrock_mantle"
                        || host_listing(prefix).is_some_and(|(host, _)| host == data::BEDROCK_HOST)
                }))
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

pub(super) fn host_listing(segment: &str) -> Option<(u8, &'static data::HostListing)> {
    find_host_listing(segment, |listing| listing.spellings)
}

fn find_host_listing(
    segment: &str,
    spellings: impl Fn(&data::HostListing) -> &'static [&'static str],
) -> Option<(u8, &'static data::HostListing)> {
    data::HOSTS
        .iter()
        .zip(0..)
        .find(|(listing, _)| {
            spellings(listing).iter().any(|spelling| {
                segment.len() == spelling.len()
                    && segment.bytes().zip(spelling.bytes()).all(|(left, right)| {
                        left.eq_ignore_ascii_case(&right)
                            || matches!((left, right), (b'-', b'_') | (b'_', b'-'))
                    })
            })
        })
        .map(|(listing, index)| (index, listing))
}

pub(super) fn is_host_segment(segment: &str) -> bool {
    !super::embedded::litellm_sources()
        .is_some_and(|sources| sources.direct_providers.iter().any(|id| id == segment))
}

pub(super) fn host_of_model_id(model: &str) -> Option<(u8, &'static data::HostListing)> {
    let (word, _) = model.split_once('.')?;
    find_host_listing(word, |listing| listing.model_id_words)
}

pub(super) fn bare_host(key: &str, provider: &str) -> Option<u8> {
    if key.contains('/') {
        return None;
    }
    data::HOSTS
        .iter()
        .zip(0..)
        .find(|(listing, _)| listing.bare_providers.contains(&provider))
        .map(|(_, index)| index)
}

pub(super) fn vendor_start(key: &str, vendor_id: &str) -> Option<NonZeroUsize> {
    NonZeroUsize::new(key.len() - vendor_id.len() + 1)
}

pub(super) fn cache_defaults(model: &str, provider: Option<&str>) -> &'static CacheDefaults {
    model_cache_defaults(model).unwrap_or(provider_cache_defaults(provider))
}

fn model_cache_defaults(model: &str) -> Option<&'static CacheDefaults> {
    use data::{CACHE_DEFAULTS_BY_SEGMENT, CACHE_DEFAULTS_BY_SEGMENT_PREFIX};
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
}

pub(super) fn provider_cache_defaults(provider: Option<&str>) -> &'static CacheDefaults {
    data::CACHE_DEFAULTS_BY_PROVIDER
        .iter()
        .find(|(key, _)| Some(*key) == provider)
        .map(|(_, defaults)| *defaults)
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

    pub(super) fn append(&mut self, key: String, mut row: Row) -> &mut Row {
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
