use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

use crate::fast::FxHashMap;

use super::{
    CacheDefaults, LongContext, LongContextMode, LongContextRates, PricingMap, PricingRows, Row,
    RowPricing, Scope, Source, data,
    row::{Published, PublishedRates},
    speed::PublishedSpeed,
};

/// Builds the embedded catalogue from the LiteLLM and models.dev snapshots and
/// encodes it for [`EmbeddedSnapshots::pricing_table`](super::EmbeddedSnapshots).
/// Install the LiteLLM source and models.dev catalog rules first.
pub fn encode_pricing_table(litellm_json: &str, models_dev_json: &str) -> Vec<u8> {
    let mut models_dev = PricingMap::default();
    models_dev
        .load_models_dev_json_missing(models_dev_json)
        .expect("embedded models-dev-pricing.json must parse");
    let mut map = PricingMap::default();
    assert!(
        map.load_json(litellm_json) > 0,
        "embedded LiteLLM pricing must load"
    );
    map.complete_layers(&models_dev);
    let mut writer = Writer::default();
    writer.rows(&map.rows);
    writer.rows(&models_dev.rows);
    writer.finish()
}

pub(super) struct PricingTable {
    pub(super) rows: PricingMap,
    pub(super) models_dev: PricingMap,
}

pub(super) fn decode_pricing_table(bytes: &[u8]) -> PricingTable {
    let mut reader = Reader::new(bytes);
    let table = PricingTable {
        rows: reader.rows(),
        models_dev: reader.rows(),
    };
    assert!(
        reader.keys.0.is_empty()
            && reader.flags.0.is_empty()
            && reader.limits.0.is_empty()
            && reader.rates.0.is_empty(),
        "embedded pricing table has trailing bytes"
    );
    table
}

const SOURCE_LITELLM: u8 = 0;
const SOURCE_CURATED: u8 = 1;
const SOURCE_MODELS_DEV: u8 = 2;
const SOURCE_MASK: u8 = 0b11;
const SCOPE_DIRECT: u8 = 1 << 2;
const EXACT_ONLY: u8 = 1 << 3;
const WHOLE_REQUEST_TIERS: u8 = 1 << 4;

const NO_LONG_CONTEXT: u8 = 0;
const LONG_CONTEXT_MARGINAL: u8 = 1;
const LONG_CONTEXT_WHOLE_REQUEST: u8 = 2;

const PUBLISHED_KNOWN: u8 = 0;
const PUBLISHED_BY_KEY: u8 = 1;

enum Rate {
    Required(f64),
    Optional(Option<f64>),
}

// NOTE(2026-11-20, #2388): The deflated table replaces the JSON snapshots in the binary, and a row-major layout with inline rates compresses larger than they do. Keep one stream per field kind and rates as indexes into a pool ordered by frequency.
#[derive(Default)]
struct Writer {
    keys: Vec<u8>,
    flags: Vec<u8>,
    limits: Vec<u8>,
    rates: Vec<Rate>,
}

impl Writer {
    fn finish(self) -> Vec<u8> {
        let mut counts = FxHashMap::<u64, usize>::default();
        for rate in &self.rates {
            if let Rate::Required(value) | Rate::Optional(Some(value)) = rate {
                *counts.entry(value.to_bits()).or_default() += 1;
            }
        }
        let mut values: Vec<_> = counts.into_iter().collect();
        values.sort_by(|(left, left_count), (right, right_count)| {
            right_count.cmp(left_count).then(left.cmp(right))
        });
        let ids: FxHashMap<u64, usize> = values
            .iter()
            .enumerate()
            .map(|(id, (bits, _))| (*bits, id))
            .collect();
        let mut rates = Vec::new();
        for rate in &self.rates {
            let id = match rate {
                Rate::Required(value) => ids[&value.to_bits()],
                Rate::Optional(value) => value.map_or(0, |value| ids[&value.to_bits()] + 1),
            };
            varint(&mut rates, id as u64);
        }

        let mut table = Vec::new();
        varint(&mut table, values.len() as u64);
        for (bits, _) in &values {
            table.extend_from_slice(&bits.to_le_bytes());
        }
        for stream in [&self.keys, &self.flags, &self.limits] {
            varint(&mut table, stream.len() as u64);
            table.extend_from_slice(stream);
        }
        table.extend_from_slice(&rates);
        table
    }

    fn rows(&mut self, rows: &PricingRows) {
        let mut sorted: Vec<_> = rows.entries.rows.iter().collect();
        sorted.sort_by(|left, right| left.key.cmp(&right.key));
        varint(&mut self.keys, sorted.len() as u64);
        let mut previous: &str = "";
        for row in sorted {
            self.key(previous, &row.key);
            self.row(row);
            previous = &row.key;
        }
    }

    fn key(&mut self, previous: &str, key: &str) {
        let shared = previous
            .bytes()
            .zip(key.bytes())
            .take_while(|(left, right)| left == right)
            .count();
        let shared = (0..=shared)
            .rev()
            .find(|len| key.is_char_boundary(*len))
            .unwrap_or(0);
        let suffix = &key[shared..];
        varint(&mut self.keys, shared as u64);
        varint(&mut self.keys, suffix.len() as u64);
        self.keys.extend_from_slice(suffix.as_bytes());
    }

    fn row(&mut self, row: &Row) {
        let source = match row.source {
            Source::LiteLlm => SOURCE_LITELLM,
            Source::Curated => SOURCE_CURATED,
            Source::ModelsDev => SOURCE_MODELS_DEV,
            Source::Override => unreachable!("embedded pricing has no user overrides"),
        };
        let scope = if row.scope == Scope::Direct {
            SCOPE_DIRECT
        } else {
            0
        };
        let exact_only = if row.exact_only { EXACT_ONLY } else { 0 };
        self.flags.push(
            source | scope | exact_only | (u8::from(row.whole_request_tiers) * WHOLE_REQUEST_TIERS),
        );
        varint(
            &mut self.flags,
            row.vendor_start.map_or(0, |start| start.get() as u64),
        );
        let schedule = row.schedule.map_or(0, |schedule| {
            data::SCHEDULES
                .iter()
                .position(|(_, known)| std::ptr::eq(known, schedule))
                .expect("a row schedule comes from data::SCHEDULES")
                + 1
        });
        varint(&mut self.flags, schedule as u64);
        let context_limit = row.context_limit.map_or(0, |limit| {
            limit.checked_add(1).expect("context limit fits the table")
        });
        varint(&mut self.limits, context_limit);
        self.pricing(&row.pricing);
    }

    fn pricing(&mut self, pricing: &RowPricing) {
        self.rates.extend([
            Rate::Required(pricing.input),
            Rate::Required(pricing.output),
            Rate::Optional(pricing.cache_create),
            Rate::Optional(pricing.cache_read),
            Rate::Optional(pricing.cache_create_1h),
        ]);
        match pricing.long_context.as_deref() {
            None => self.flags.push(NO_LONG_CONTEXT),
            Some(context) => {
                self.flags.push(match context.mode {
                    LongContextMode::Marginal => LONG_CONTEXT_MARGINAL,
                    LongContextMode::WholeRequest => LONG_CONTEXT_WHOLE_REQUEST,
                });
                varint(&mut self.limits, context.threshold);
                let rates = context.rates;
                self.rates.extend(
                    [
                        rates.input,
                        rates.output,
                        rates.cache_create,
                        rates.cache_create_1h,
                        rates.cache_read,
                    ]
                    .map(Rate::Optional),
                );
            }
        }
        varint(&mut self.flags, u64::from(pricing.explicit_fields));
        let cache_defaults = match &pricing.published {
            Published::Known(rates) => {
                self.flags.push(PUBLISHED_KNOWN);
                self.rates.extend(
                    [
                        rates.fast_multiplier,
                        rates.ultrafast_multiplier,
                        rates.flex_multiplier,
                    ]
                    .map(Rate::Required),
                );
                rates.cache_defaults
            }
            Published::ByKey {
                speed,
                cache_fallback,
                ..
            } => {
                self.flags.push(PUBLISHED_BY_KEY);
                self.rates
                    .extend([speed.fast, speed.ultrafast, speed.flex].map(Rate::Optional));
                *cache_fallback
            }
        };
        let cache_defaults = data::CACHE_DEFAULTS_BY_SEGMENT_PREFIX
            .iter()
            .position(|(_, known)| *known == cache_defaults)
            .expect("data::CACHE_DEFAULTS_BY_SEGMENT_PREFIX lists every row's cache defaults");
        varint(&mut self.flags, cache_defaults as u64);
    }
}

fn varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push((value as u8) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

#[derive(Clone, Copy)]
struct Stream<'a>(&'a [u8]);

impl<'a> Stream<'a> {
    fn take(&mut self, len: usize) -> &'a [u8] {
        let (bytes, rest) = self
            .0
            .split_at_checked(len)
            .expect("embedded pricing table is truncated");
        self.0 = rest;
        bytes
    }

    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    fn usize(&mut self) -> usize {
        usize::try_from(self.u64()).expect("embedded pricing index fits usize")
    }

    fn u64(&mut self) -> u64 {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let byte = self.u8();
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn stream(&mut self) -> Self {
        let len = self.usize();
        Self(self.take(len))
    }
}

struct Reader<'a> {
    values: &'a [u8],
    keys: Stream<'a>,
    flags: Stream<'a>,
    limits: Stream<'a>,
    rates: Stream<'a>,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let mut table = Stream(bytes);
        let len = table.usize();
        let values = table.take(len * 8);
        Self {
            values,
            keys: table.stream(),
            flags: table.stream(),
            limits: table.stream(),
            rates: table,
        }
    }

    fn rows(&mut self) -> PricingMap {
        let mut map = PricingMap::default();
        let entries = &mut map.rows_mut().entries;
        for _ in 0..self.keys.usize() {
            let shared = self.keys.usize();
            let suffix = self.keys.usize();
            let suffix = std::str::from_utf8(self.keys.take(suffix))
                .expect("embedded pricing keys are UTF-8");
            let previous = entries.rows.last().map_or("", |row| &row.key[..shared]);
            let key = [previous, suffix].concat();
            let row = self.row();
            entries.append(key, row);
        }
        map
    }

    fn row(&mut self) -> Row {
        let flags = self.flags.u8();
        let source = match flags & SOURCE_MASK {
            SOURCE_LITELLM => Some(Source::LiteLlm),
            SOURCE_CURATED => Some(Source::Curated),
            SOURCE_MODELS_DEV => Some(Source::ModelsDev),
            _ => None,
        }
        .expect("embedded pricing table has a known row source");
        let scope = if flags & SCOPE_DIRECT == 0 {
            Scope::Hosted
        } else {
            Scope::Direct
        };
        let vendor_start = NonZeroUsize::new(self.flags.usize());
        let schedule = self
            .flags
            .usize()
            .checked_sub(1)
            .map(|index| &data::SCHEDULES[index].1);
        let context_limit = self.limits.u64().checked_sub(1);
        Row {
            key: Arc::default(),
            pricing: self.pricing(),
            source,
            schedule,
            context_limit,
            exact_only: flags & EXACT_ONLY != 0,
            whole_request_tiers: flags & WHOLE_REQUEST_TIERS != 0,
            scope,
            vendor_start,
        }
    }

    fn pricing(&mut self) -> RowPricing {
        let input = self.rate();
        let output = self.rate();
        let cache_create = self.optional_rate();
        let cache_read = self.optional_rate();
        let cache_create_1h = self.optional_rate();
        let mode = match self.flags.u8() {
            NO_LONG_CONTEXT => Some(None),
            LONG_CONTEXT_MARGINAL => Some(Some(LongContextMode::Marginal)),
            LONG_CONTEXT_WHOLE_REQUEST => Some(Some(LongContextMode::WholeRequest)),
            _ => None,
        }
        .expect("embedded pricing table has a known long-context mode");
        let long_context = mode.map(|mode| {
            Arc::new(LongContext {
                threshold: self.limits.u64(),
                mode,
                rates: LongContextRates {
                    input: self.optional_rate(),
                    output: self.optional_rate(),
                    cache_create: self.optional_rate(),
                    cache_create_1h: self.optional_rate(),
                    cache_read: self.optional_rate(),
                },
            })
        });
        let explicit_fields =
            u16::try_from(self.flags.u64()).expect("embedded pricing explicit fields fit u16");
        let published = match self.flags.u8() {
            PUBLISHED_KNOWN => Some(Published::Known(PublishedRates {
                fast_multiplier: self.rate(),
                ultrafast_multiplier: self.rate(),
                flex_multiplier: self.rate(),
                cache_defaults: self.cache_defaults(),
            })),
            PUBLISHED_BY_KEY => Some(Published::ByKey {
                speed: PublishedSpeed {
                    fast: self.optional_rate(),
                    ultrafast: self.optional_rate(),
                    flex: self.optional_rate(),
                },
                cache_fallback: self.cache_defaults(),
                resolved: OnceLock::new(),
            }),
            _ => None,
        }
        .expect("embedded pricing table has a known published-rate kind");
        RowPricing {
            input,
            output,
            cache_create,
            cache_read,
            cache_create_1h,
            long_context,
            published,
            explicit_fields,
        }
    }

    fn cache_defaults(&mut self) -> &'static CacheDefaults {
        data::CACHE_DEFAULTS_BY_SEGMENT_PREFIX[self.flags.usize()].1
    }

    fn rate(&mut self) -> f64 {
        let id = self.rates.usize();
        self.value(id)
    }

    fn optional_rate(&mut self) -> Option<f64> {
        let id = self.rates.usize().checked_sub(1)?;
        Some(self.value(id))
    }

    fn value(&self, id: usize) -> f64 {
        let bits = self.values[id * 8..][..8]
            .try_into()
            .expect("a pooled rate is 8 bytes");
        f64::from_bits(u64::from_le_bytes(bits))
    }
}
