use std::{
    cmp::Ordering,
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

use crate::fast::FxHashMap;

use super::{
    CacheDefaults, LongContext, LongContextMode, LongContextRates, PricingMap, PricingRows, Row,
    RowMatch, RowPricing, Scope, Source, data, normalized_pricing_key,
    row::{Published, PublishedRates},
    rows::{FoldedIndexes, Index, folded_identity},
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

pub(super) fn decode_pricing_table(bytes: &'static [u8]) -> PricingTable {
    let mut reader = Reader::new(bytes);
    let table = PricingTable {
        rows: PricingMap::from_embedded_rows(reader.rows()),
        models_dev: PricingMap::from_embedded_rows(reader.rows()),
    };
    assert!(
        reader.catalogues.0.is_empty(),
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
const HOST_SHIFT: u8 = 5;
const HOST_MASK: u8 = 0b11 << HOST_SHIFT;
const _: () = assert!(data::HOSTS.len() <= (HOST_MASK >> HOST_SHIFT) as usize);
const _: () =
    assert!(HOST_MASK & (SOURCE_MASK | SCOPE_DIRECT | EXACT_ONLY | WHOLE_REQUEST_TIERS) == 0);

const NO_LONG_CONTEXT: u8 = 0;
const LONG_CONTEXT_MARGINAL: u8 = 1;
const LONG_CONTEXT_WHOLE_REQUEST: u8 = 2;

const PUBLISHED_KNOWN: u8 = 0;
const PUBLISHED_BY_KEY: u8 = 1;

const KEY: usize = 0;
const KEY_LEN: usize = 1;
const IDENTITY: usize = 2;
const VENDOR_LOOKUP_START: usize = 3;
const FLAGS: usize = 4;
const LIMITS: usize = 5;
const RATES: usize = 6;
type EncodedId = [u8; 4];
type Record = [u8; (RATES + 1) * size_of::<EncodedId>()];
const ROWS_PER_PAGE: usize = 64;
type RowPage = [OnceLock<Box<Row>>; ROWS_PER_PAGE];

fn push_offset(bytes: &mut Vec<u8>, offset: usize) {
    bytes.extend_from_slice(
        &u32::try_from(offset)
            .expect("pricing offset fits u32")
            .to_le_bytes(),
    );
}

fn push_stream(bytes: &mut Vec<u8>, stream: &[u8]) {
    varint(bytes, stream.len() as u64);
    bytes.extend_from_slice(stream);
}

enum Rate {
    Required(f64),
    Optional(Option<f64>),
}

// NOTE(2026-11-21, #2465): Inflating the indexed table still charges each report for the whole catalog. Borrow the streams directly from the binary and keep rates pooled to limit their size.
#[derive(Default)]
struct Writer {
    catalogues: Vec<Catalogue>,
    flags: Vec<u8>,
    limits: Vec<u8>,
    rates: Vec<Rate>,
}

struct Catalogue {
    keys: Vec<u8>,
    records: Vec<[usize; RATES + 1]>,
    sensitive: [Vec<usize>; 3],
    exact_only: Vec<usize>,
    folded: FoldedIndexes,
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
        let mut offsets = Vec::with_capacity(self.rates.len());
        for rate in &self.rates {
            let id = match rate {
                Rate::Required(value) => ids[&value.to_bits()],
                Rate::Optional(value) => value.map_or(0, |value| ids[&value.to_bits()] + 1),
            };
            offsets.push(rates.len());
            varint(&mut rates, id as u64);
        }

        let mut table = Vec::new();
        varint(&mut table, values.len() as u64);
        for (bits, _) in &values {
            table.extend_from_slice(&bits.to_le_bytes());
        }
        for stream in [&self.flags, &self.limits, &rates] {
            push_stream(&mut table, stream);
        }
        for Catalogue {
            keys,
            records,
            sensitive,
            exact_only,
            folded,
        } in self.catalogues
        {
            push_stream(&mut table, &keys);
            varint(&mut table, (records.len() * size_of::<Record>()) as u64);
            for mut record in records {
                record[RATES] = offsets[record[RATES]];
                for offset in record {
                    push_offset(&mut table, offset);
                }
            }
            for ids in sensitive.into_iter().chain([exact_only]).chain(folded.0) {
                varint(&mut table, (ids.len() * size_of::<EncodedId>()) as u64);
                for id in ids {
                    push_offset(&mut table, id);
                }
            }
        }
        table
    }

    fn rows(&mut self, rows: &PricingRows) {
        let mut sorted: Vec<_> = rows.entries.rows.iter().collect();
        sorted.sort_by(|left, right| left.key.cmp(&right.key));
        let mut keys = Vec::new();
        let mut records = Vec::new();
        let mut sensitive: [Vec<(&str, usize)>; 3] = Default::default();
        let identities: Vec<_> = sorted
            .iter()
            .map(|row| normalized_pricing_key(&row.key))
            .collect();
        let mut exact_only = Vec::new();
        for (id, (row, identity)) in sorted.iter().zip(&identities).enumerate() {
            let found = RowMatch::new(id, row);
            let key = keys.len();
            keys.extend_from_slice(row.key.as_bytes());
            let normalized = if identity.as_ref() == row.key.as_ref() {
                key
            } else {
                let offset = keys.len();
                keys.extend_from_slice(identity.as_bytes());
                offset
            };
            let vendor =
                super::rows::vendor_lookup_id(found).map(|vendor| row.key.len() - vendor.len());
            records.push([
                key,
                row.key.len(),
                normalized,
                vendor.unwrap_or(row.key.len()),
                self.flags.len(),
                self.limits.len(),
                self.rates.len(),
            ]);
            sensitive[Index::for_key(found) as usize].push((identity.as_ref(), id));
            if let Some(start) = vendor {
                sensitive[Index::VendorId as usize].push((&identity[start..], id));
            }
            if row.exact_only {
                exact_only.push((identity.as_ref(), id));
            }
            self.row(row);
        }
        let folded = FoldedIndexes::build(
            sorted
                .iter()
                .enumerate()
                .map(|(id, row)| RowMatch::new(id, row)),
        );
        let sorted_ids = |mut keys: Vec<(&str, usize)>| {
            keys.sort_unstable();
            keys.into_iter().map(|(_, id)| id).collect()
        };
        self.catalogues.push(Catalogue {
            keys,
            records,
            sensitive: sensitive.map(sorted_ids),
            exact_only: sorted_ids(exact_only),
            folded,
        });
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
        let whole_request = u8::from(row.whole_request_tiers) * WHOLE_REQUEST_TIERS;
        let host = row.host.map_or(0, |host| (host + 1) << HOST_SHIFT);
        self.flags
            .push(source | scope | exact_only | whole_request | host);
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

#[derive(Clone, Copy, Debug)]
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
    data: RowData<'a>,
    catalogues: Stream<'a>,
}

impl Reader<'static> {
    fn new(bytes: &'static [u8]) -> Self {
        let mut table = Stream(bytes);
        let len = table.usize();
        let values = table.take(len * 8);
        let data = RowData {
            values,
            flags: table.stream(),
            limits: table.stream(),
            rates: table.stream(),
        };
        Self {
            data,
            catalogues: table,
        }
    }

    fn rows(&mut self) -> EmbeddedRows {
        let keys = std::str::from_utf8(self.catalogues.stream().0)
            .expect("embedded pricing keys are UTF-8");
        let (records, remainder) = self.catalogues.stream().0.as_chunks();
        assert!(
            remainder.is_empty(),
            "embedded pricing records are truncated"
        );
        let mut ids = || {
            let (ids, remainder) = self.catalogues.stream().0.as_chunks();
            assert!(remainder.is_empty(), "embedded pricing index is truncated");
            ids
        };
        EmbeddedRows {
            data: self.data,
            keys,
            records,
            sensitive: std::array::from_fn(|_| ids()),
            exact_only: ids(),
            folded: std::array::from_fn(|_| ids()),
            decoded: OnceLock::new(),
        }
    }
}

pub(super) fn decode_id(id: &EncodedId) -> usize {
    u32::from_le_bytes(*id) as usize
}

#[derive(Debug)]
pub(super) struct EmbeddedRows {
    data: RowData<'static>,
    keys: &'static str,
    records: &'static [Record],
    sensitive: [&'static [EncodedId]; 3],
    exact_only: &'static [EncodedId],
    folded: [&'static [EncodedId]; 3],
    decoded: OnceLock<Box<[OnceLock<Box<RowPage>>]>>,
}

impl EmbeddedRows {
    pub(super) fn len(&self) -> usize {
        self.records.len()
    }

    fn offset(record: &Record, index: usize) -> usize {
        decode_id(&record.as_chunks::<4>().0[index])
    }

    fn read_key(&self, record: &Record, field: usize) -> &str {
        let start = Self::offset(record, field);
        &self.keys[start..start + Self::offset(record, KEY_LEN)]
    }

    pub(super) fn key(&self, id: usize) -> &str {
        self.read_key(&self.records[id], KEY)
    }

    pub(super) fn identity(&self, id: usize) -> &str {
        self.read_key(&self.records[id], IDENTITY)
    }

    pub(super) fn vendor_id(&self, id: usize) -> Option<&str> {
        let mut flags = Stream(&self.data.flags.0[Self::offset(&self.records[id], FLAGS)..]);
        flags.u8();
        let start = flags.usize().checked_sub(1)?;
        Some(&self.key(id)[start..])
    }

    pub(super) fn id(&self, key: &str) -> Option<usize> {
        self.records
            .binary_search_by(|record| self.read_key(record, KEY).cmp(key))
            .ok()
    }

    pub(super) fn exact_only(&self, id: usize) -> bool {
        self.data.flags.0[Self::offset(&self.records[id], FLAGS)] & EXACT_ONLY != 0
    }

    pub(super) fn exact_only_ids(&self, identity: &str) -> &[EncodedId] {
        matching_ids(self.exact_only, |id| self.identity(id).cmp(identity))
    }

    pub(super) fn indexed_ids(&self, identity: &str, index: Index, fold: bool) -> &[EncodedId] {
        let ids = if fold { self.folded } else { self.sensitive };
        matching_ids(ids[index as usize], |id| {
            let mut key = self.identity(id);
            if matches!(index, Index::VendorId) {
                key = &key[Self::offset(&self.records[id], VENDOR_LOOKUP_START)..];
            }
            if fold {
                folded_identity(key).cmp(identity.bytes())
            } else {
                key.cmp(identity)
            }
        })
    }

    pub(super) fn row(&self, id: usize) -> Option<&Row> {
        let record = self.records.get(id)?;
        let pages = self.decoded.get_or_init(|| {
            std::iter::repeat_with(OnceLock::new)
                .take(self.len().div_ceil(ROWS_PER_PAGE))
                .collect()
        });
        let page = pages[id / ROWS_PER_PAGE]
            .get_or_init(|| Box::new([const { OnceLock::new() }; ROWS_PER_PAGE]));
        Some(page[id % ROWS_PER_PAGE].get_or_init(|| Box::new(self.decode_row(record))))
    }

    pub(super) fn materialize(&self) -> super::Entries {
        let mut entries = super::Entries::default();
        entries.rows.reserve_exact(self.len());
        for record in self.records {
            let row = self.decode_row(record);
            entries.append_row(row);
        }
        entries
    }

    fn decode_row(&self, record: &Record) -> Row {
        let mut data = RowData {
            values: self.data.values,
            flags: Stream(&self.data.flags.0[Self::offset(record, FLAGS)..]),
            limits: Stream(&self.data.limits.0[Self::offset(record, LIMITS)..]),
            rates: Stream(&self.data.rates.0[Self::offset(record, RATES)..]),
        };
        data.row(Arc::from(self.read_key(record, KEY)))
    }

    #[cfg(test)]
    pub(super) fn decode_cache_counts(&self) -> (usize, usize) {
        self.decoded
            .get()
            .into_iter()
            .flat_map(|pages| pages.iter())
            .filter_map(OnceLock::get)
            .fold((0, 0), |(slots, rows), page| {
                let decoded_rows = page.iter().filter(|cell| cell.get().is_some()).count();
                (slots + page.len(), rows + decoded_rows)
            })
    }
}

fn matching_ids(ids: &[EncodedId], compare: impl Fn(usize) -> Ordering) -> &[EncodedId] {
    let start = ids.partition_point(|id| compare(decode_id(id)).is_lt());
    let end = start
        + ids[start..]
            .iter()
            .take_while(|id| compare(decode_id(id)).is_eq())
            .count();
    &ids[start..end]
}

#[derive(Clone, Copy, Debug)]
struct RowData<'a> {
    values: &'a [u8],
    flags: Stream<'a>,
    limits: Stream<'a>,
    rates: Stream<'a>,
}

impl RowData<'_> {
    fn row(&mut self, key: Arc<str>) -> Row {
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
        let host = ((flags & HOST_MASK) >> HOST_SHIFT).checked_sub(1);
        let context_limit = self.limits.u64().checked_sub(1);
        Row {
            key,
            pricing: self.pricing(),
            source,
            schedule,
            context_limit,
            exact_only: flags & EXACT_ONLY != 0,
            whole_request_tiers: flags & WHOLE_REQUEST_TIERS != 0,
            scope,
            vendor_start,
            host,
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
