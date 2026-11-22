use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

use smallvec::SmallVec;

use crate::fast::FxHashMap;

use super::{
    Entries, Name, Row, Scope, is_pricing_key_separator, last_segment, normalized_pricing_key,
    secondary_index::SecondaryIndex,
    table::{EmbeddedRows, decode_id},
};

#[derive(Clone, Copy)]
pub(super) enum Lookup {
    Pricing,
    DirectPricing,
    Context,
    Paid,
}

impl Lookup {
    pub(super) fn includes(self, row: &Row) -> bool {
        match self {
            Self::Pricing => true,
            Self::DirectPricing => !row.is_bedrock(),
            Self::Context => row.context_limit.is_some(),
            Self::Paid => !row.is_free(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct RowMatch<'a> {
    pub(super) key: &'a str,
    pub(super) row: &'a Row,
    pub(super) id: usize,
}

impl<'a> RowMatch<'a> {
    pub(super) fn new(id: usize, row: &'a Row) -> Self {
        Self {
            key: &row.key,
            row,
            id,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct PricingRows {
    pub(super) entries: Entries,
    embedded: Option<EmbeddedSource>,
    pub(super) indexes: OnceLock<RowIndexes>,
    pub(super) folded_indexes: OnceLock<FoldedIndexes>,
    exact_only_by_identity: OnceLock<FxHashMap<Box<str>, Option<usize>>>,
}

#[derive(Clone, Debug)]
enum EmbeddedSource {
    Ready(Arc<EmbeddedRows>),
    Deferred {
        load: fn() -> &'static EmbeddedRows,
        index: SecondaryIndex,
    },
}

impl EmbeddedSource {
    fn rows(&self) -> &EmbeddedRows {
        match self {
            Self::Ready(rows) => rows,
            Self::Deferred { load, .. } => load(),
        }
    }
}

type IdentityIndex = FxHashMap<Box<str>, smallvec::SmallVec<[usize; 1]>>;

#[derive(Clone, Debug, Default)]
pub(super) struct RowIndexes {
    by_identity: IdentityIndex,
    by_vendor_id: IdentityIndex,
    bare_hosted: IdentityIndex,
}

#[derive(Clone, Copy)]
pub(super) enum Index {
    Identity,
    VendorId,
    BareHosted,
}

impl Index {
    pub(super) fn for_key(found: RowMatch<'_>) -> Self {
        if found.row.is_bare_hosted(found.key) {
            Self::BareHosted
        } else {
            Self::Identity
        }
    }

    fn sensitive(self, indexes: &RowIndexes) -> &IdentityIndex {
        match self {
            Self::Identity => &indexes.by_identity,
            Self::VendorId => &indexes.by_vendor_id,
            Self::BareHosted => &indexes.bare_hosted,
        }
    }

    fn key(self, found: RowMatch<'_>) -> &str {
        match self {
            Self::VendorId => vendor_lookup_id(found).expect("vendor index contains vendor IDs"),
            Self::Identity | Self::BareHosted => found.key,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FoldedIndexes(pub(super) [Vec<usize>; 3]);

impl FoldedIndexes {
    pub(super) fn build<'a>(rows: impl Iterator<Item = RowMatch<'a>>) -> Self {
        let mut keys: [Vec<(&str, usize)>; 3] = Default::default();
        for found in rows {
            if has_uppercase(found.key) {
                keys[Index::for_key(found) as usize].push((found.key, found.id));
            }
            if let Some(id) = vendor_lookup_id(found).filter(|id| has_uppercase(id)) {
                keys[Index::VendorId as usize].push((id, found.id));
            }
        }
        Self(keys.map(sort_folded_ids))
    }
}

pub(super) fn sort_folded_ids(mut keys: Vec<(&str, usize)>) -> Vec<usize> {
    keys.sort_unstable_by(|(left, _), (right, _)| {
        folded_identity(left).cmp(folded_identity(right))
    });
    keys.into_iter().map(|(_, id)| id).collect()
}

pub(super) fn matching_folded_ids<'a, 'k>(
    ids: &'a [usize],
    identity: &str,
    key: impl Fn(usize) -> &'k str,
) -> &'a [usize] {
    let compare = |id: &usize| folded_identity(key(*id)).cmp(identity.bytes());
    let start = ids.partition_point(|id| compare(id).is_lt());
    let end = start
        + ids[start..]
            .iter()
            .take_while(|id| compare(id).is_eq())
            .count();
    &ids[start..end]
}

pub(super) fn vendor_lookup_id(found: RowMatch<'_>) -> Option<&str> {
    let row = found.row;
    let id = if row.scope == Scope::Direct && row.is_free() {
        Some(last_segment(found.key))
    } else {
        row.vendor_id()
    };
    id.filter(|id| *id != found.key)
}

pub(super) fn has_uppercase(key: &str) -> bool {
    key.bytes().any(|byte| byte.is_ascii_uppercase())
}

pub(super) fn fold_case(identity: &str) -> Cow<'_, str> {
    if has_uppercase(identity) {
        Cow::Owned(identity.to_ascii_lowercase())
    } else {
        Cow::Borrowed(identity)
    }
}

pub(super) fn folded_identity(key: &str) -> impl Iterator<Item = u8> + '_ {
    key.bytes().map(|byte| {
        if is_pricing_key_separator(byte) {
            b'-'
        } else {
            byte.to_ascii_lowercase()
        }
    })
}

pub(super) fn identity_order<'k>(name: Name<'_>, key: &'k str, fold: bool) -> (bool, &'k str) {
    (fold && !key.eq_ignore_ascii_case(name.raw()), key)
}

impl PricingRows {
    pub(super) fn from_embedded(rows: EmbeddedRows) -> Self {
        Self {
            embedded: Some(EmbeddedSource::Ready(Arc::new(rows))),
            ..Self::default()
        }
    }

    pub(super) fn from_deferred(
        load: fn() -> &'static EmbeddedRows,
        index: SecondaryIndex,
    ) -> Self {
        Self {
            embedded: Some(EmbeddedSource::Deferred { load, index }),
            ..Self::default()
        }
    }

    fn embedded(&self) -> Option<&EmbeddedRows> {
        self.embedded.as_ref().map(EmbeddedSource::rows)
    }

    fn secondary_index(&self) -> Option<SecondaryIndex> {
        match self.embedded.as_ref()? {
            EmbeddedSource::Deferred { index, .. } => Some(*index),
            EmbeddedSource::Ready(_) => None,
        }
    }

    fn may_contain(&self, key: &str) -> bool {
        self.secondary_index()
            .is_none_or(|index| index.may_contain(key))
    }

    pub(super) fn may_match_fuzzy(&self, key: &str) -> bool {
        self.secondary_index()
            .is_none_or(|index| index.may_match_fuzzy(key))
    }

    pub(super) fn make_mutable(&mut self) {
        if let Some(rows) = self.embedded.take() {
            self.entries = rows.rows().materialize();
        }
        self.indexes.take();
        self.folded_indexes.take();
        self.exact_only_by_identity.take();
    }

    #[cfg(test)]
    pub(super) fn decode_cache_counts(&self) -> (usize, usize) {
        self.embedded()
            .expect("cache counts require embedded rows")
            .decode_cache_counts()
    }

    fn id(&self, key: &str) -> Option<usize> {
        match self.embedded() {
            Some(rows) => rows.id(key),
            None => self.entries.id(key),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<&Row> {
        if !self.may_contain(key) {
            return None;
        }
        self.row(self.id(key)?)
    }

    fn literal(&self, key: &str, lookup: Lookup) -> Option<RowMatch<'_>> {
        let id = self.id(key)?;
        let found = self.at(id)?;
        lookup.includes(found.row).then_some(found)
    }

    pub(super) fn at(&self, id: usize) -> Option<RowMatch<'_>> {
        self.row(id).map(|row| RowMatch::new(id, row))
    }

    fn row(&self, id: usize) -> Option<&Row> {
        match self.embedded() {
            Some(rows) => rows.row(id),
            None => self.entries.rows.get(id),
        }
    }

    pub(super) fn vendor_id(&self, id: usize) -> Option<&str> {
        match self.embedded() {
            Some(rows) => rows.vendor_id(id),
            None => self.entries.rows[id].vendor_id(),
        }
    }

    fn len(&self) -> usize {
        self.embedded()
            .map_or(self.entries.rows.len(), EmbeddedRows::len)
    }

    pub(super) fn matches(&self) -> impl Iterator<Item = RowMatch<'_>> {
        (0..self.len()).filter_map(|id| self.at(id))
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = (usize, &str)> {
        let embedded = self.embedded();
        let len = embedded.map_or(self.entries.rows.len(), EmbeddedRows::len);
        (0..len).map(move |id| {
            let key = match embedded {
                Some(rows) => rows.key(id),
                None => &self.entries.rows[id].key,
            };
            (id, key)
        })
    }

    #[cfg(test)]
    pub(super) fn indexed_rows<'a>(
        &'a self,
        identity: &str,
        index: Index,
        fold: bool,
    ) -> impl Iterator<Item = RowMatch<'a>> {
        self.search_index::<false>(identity, index, fold)
    }

    fn search_index<'a, const INHERITANCE: bool>(
        &'a self,
        identity: &str,
        index: Index,
        fold: bool,
    ) -> impl Iterator<Item = RowMatch<'a>> {
        let mut uncached = SmallVec::<[usize; 1]>::new();
        let (sensitive, folded) = match self.embedded() {
            Some(rows) => (
                RowIds::Embedded(rows.indexed_ids(identity, index, false)),
                RowIds::Embedded(if fold {
                    rows.indexed_ids(identity, index, true)
                } else {
                    &[]
                }),
            ),
            None => {
                let keys = index.sensitive(self.indexes());
                let folded = if !fold {
                    &[]
                } else if INHERITANCE && self.folded_indexes.get().is_none() {
                    uncached.extend(
                        keys.iter()
                            .filter_map(|(key, ids)| {
                                (key.len() == identity.len()
                                    && key.as_ref() != identity
                                    && folded_identity(key).eq(identity.bytes()))
                                .then_some(ids)
                            })
                            .flatten()
                            .copied(),
                    );
                    &[]
                } else {
                    self.folded_ids(identity, index)
                };
                (
                    RowIds::Live(keys.get(identity).map_or(&[], |ids| ids.as_slice())),
                    RowIds::Live(folded),
                )
            }
        };
        sensitive
            .iter()
            .chain(folded.iter())
            .chain(uncached)
            .filter_map(|id| self.at(id))
    }

    fn folded_ids(&self, identity: &str, index: Index) -> &[usize] {
        let indexes = self
            .folded_indexes
            .get_or_init(|| FoldedIndexes::build(self.matches()));
        matching_folded_ids(&indexes.0[index as usize], identity, |id| {
            index.key(RowMatch::new(id, &self.entries.rows[id]))
        })
    }

    pub(super) fn authoritative<'a>(
        &'a self,
        name: Name<'_>,
        hosted: bool,
        fold: bool,
        visible: impl Fn(RowMatch<'a>) -> Option<RowMatch<'a>>,
    ) -> Option<RowMatch<'a>> {
        self.authoritative_on_index::<false>(name, hosted, fold, visible)
    }

    pub(super) fn authoritative_base(&self, name: Name<'_>, fold: bool) -> Option<RowMatch<'_>> {
        if !self.may_contain(name.raw()) {
            return None;
        }
        self.authoritative_on_index::<true>(name, false, fold, Some)
    }

    fn authoritative_on_index<'a, const INHERITANCE: bool>(
        &'a self,
        name: Name<'_>,
        hosted: bool,
        fold: bool,
        visible: impl Fn(RowMatch<'a>) -> Option<RowMatch<'a>>,
    ) -> Option<RowMatch<'a>> {
        let literal = if fold {
            None
        } else {
            self.literal(name.raw(), Lookup::Pricing)
        };
        if let Some(found) = literal
            && !found.row.is_bare_hosted(found.key)
            && let Some(found) = visible(found)
        {
            return Some(found);
        }
        let identity = if fold {
            fold_case(name.identity())
        } else {
            Cow::Borrowed(name.identity())
        };
        let matches = |index| {
            self.search_index::<INHERITANCE>(&identity, index, fold)
                .filter_map(|raw| Some((raw.key, visible(raw)?)))
        };
        let by_key = |select| {
            matches(select)
                .min_by_key(|(key, _)| identity_order(name, key, fold))
                .map(|(_, found)| found)
        };
        by_key(Index::Identity)
            .or_else(|| {
                let mut rows = matches(Index::VendorId);
                let (key, first) = rows.next()?;
                let mut best = (key, first);
                for (key, found) in rows {
                    if found.row.rates() != first.row.rates() {
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
                        .or_else(|| by_key(Index::BareHosted))
                } else {
                    None
                }
            })
    }

    pub(super) fn host_row(
        &self,
        name: Name<'_>,
        host: u8,
        lookup: Lookup,
        fold: bool,
    ) -> Option<RowMatch<'_>> {
        self.exact_on_index::<false>(name, lookup, fold, Some(host))
    }

    pub(super) fn exact(&self, name: Name<'_>, lookup: Lookup, fold: bool) -> Option<RowMatch<'_>> {
        self.exact_on_index::<false>(name, lookup, fold, None)
    }

    pub(super) fn exact_base(&self, name: Name<'_>, fold: bool) -> Option<RowMatch<'_>> {
        self.exact_on_index::<true>(name, Lookup::Pricing, fold, None)
    }

    #[inline(never)]
    fn exact_on_index<const INHERITANCE: bool>(
        &self,
        name: Name<'_>,
        lookup: Lookup,
        fold: bool,
        host: Option<u8>,
    ) -> Option<RowMatch<'_>> {
        if !self.may_contain(name.raw()) {
            return None;
        }
        let on_host = |found: &RowMatch<'_>| host.is_none() || found.row.host == host;
        if !fold && let Some(literal) = self.literal(name.raw(), lookup).filter(on_host) {
            return Some(literal);
        }
        let identity = if fold {
            fold_case(name.identity())
        } else {
            Cow::Borrowed(name.identity())
        };
        let index = if host.is_some() {
            Index::BareHosted
        } else {
            Index::Identity
        };
        self.search_index::<INHERITANCE>(&identity, index, fold)
            .filter(|found| lookup.includes(found.row) && on_host(found))
            .min_by_key(|found| identity_order(name, found.key, fold))
    }

    fn indexes(&self) -> &RowIndexes {
        self.indexes.get_or_init(|| {
            let mut indexes = RowIndexes::default();
            // NOTE(2026-11-21): Normalizing swaps ASCII bytes one for one, so a vendor id, which is a suffix of its key, normalizes to the same suffix of the normalized key. Normalize the full key once while building both live indexes.
            for found in self.matches() {
                let identity = normalized_pricing_key(found.key);
                let index = if found.row.is_bare_hosted(found.key) {
                    &mut indexes.bare_hosted
                } else {
                    &mut indexes.by_identity
                };
                index
                    .entry(identity.as_ref().into())
                    .or_default()
                    .push(found.id);
                if let Some(id) = vendor_lookup_id(found) {
                    indexes
                        .by_vendor_id
                        .entry(identity[identity.len() - id.len()..].into())
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

    pub(super) fn contains_exact_only_identity(&self, key: &str, identity: &str) -> bool {
        if !self.may_contain(identity) {
            return false;
        }
        if let Some(rows) = self.embedded() {
            return rows.id(key).is_some_and(|id| rows.exact_only(id))
                || !rows.exact_only_ids(identity).is_empty();
        }
        self.entries.get(key).is_some_and(|row| row.exact_only)
            || self.exact_only_by_identity().contains_key(identity)
    }

    pub(super) fn exact_only_id(&self, key: &str) -> Option<&str> {
        if !self.may_contain(key) {
            return None;
        }
        let identity = normalized_pricing_key(key);
        if let Some(rows) = self.embedded() {
            let [id] = rows.exact_only_ids(&identity) else {
                return None;
            };
            return Some(rows.key(decode_id(id)));
        }
        let id = self.exact_only_by_identity().get(identity.as_ref())?;
        self.at((*id)?).map(|found| found.key)
    }
}

enum RowIds<'a> {
    Live(&'a [usize]),
    Embedded(&'a [[u8; 2]]),
}

impl RowIds<'_> {
    fn iter(self) -> impl Iterator<Item = usize> {
        let (live, embedded) = match self {
            Self::Live(ids) => (ids, [].as_slice()),
            Self::Embedded(ids) => ([].as_slice(), ids),
        };
        live.iter().copied().chain(embedded.iter().map(decode_id))
    }
}
