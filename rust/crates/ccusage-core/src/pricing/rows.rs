use std::{borrow::Cow, sync::OnceLock};

use crate::fast::FxHashMap;

use super::{
    Entries, Name, Row, Scope, is_pricing_key_separator, last_segment, normalized_pricing_key,
};

#[derive(Clone, Copy)]
pub(super) enum Lookup {
    Pricing,
    Context,
}

impl Lookup {
    pub(super) fn includes(self, row: &Row) -> bool {
        match self {
            Self::Pricing => true,
            Self::Context => row.context_limit.is_some(),
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
    pub(super) indexes: OnceLock<RowIndexes>,
    pub(super) folded_indexes: OnceLock<FoldedIndexes>,
    pub(super) exact_only_by_identity: OnceLock<FxHashMap<Box<str>, Option<usize>>>,
    pub(super) normalized_keys: OnceLock<Box<[Option<Box<str>>]>>,
}

type IdentityIndex = FxHashMap<Box<str>, smallvec::SmallVec<[usize; 1]>>;

#[derive(Clone, Debug, Default)]
pub(super) struct RowIndexes {
    by_identity: IdentityIndex,
    by_vendor_id: IdentityIndex,
    bare_hosted: IdentityIndex,
}

#[derive(Clone, Copy)]
enum Index {
    Identity,
    VendorId,
    BareHosted,
}

impl Index {
    fn for_key(found: RowMatch<'_>) -> Self {
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
        Self(keys.map(|mut keys| {
            keys.sort_unstable_by(|(left, _), (right, _)| {
                folded_identity(left).cmp(folded_identity(right))
            });
            keys.into_iter().map(|(_, id)| id).collect()
        }))
    }
}

fn vendor_lookup_id(found: RowMatch<'_>) -> Option<&str> {
    let row = found.row;
    let id = if row.scope == Scope::Direct && row.is_free() {
        Some(last_segment(found.key))
    } else {
        row.vendor_id()
    };
    id.filter(|id| *id != found.key)
}

fn has_uppercase(key: &str) -> bool {
    key.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn fold_case(identity: &str) -> Cow<'_, str> {
    if has_uppercase(identity) {
        Cow::Owned(identity.to_ascii_lowercase())
    } else {
        Cow::Borrowed(identity)
    }
}

fn folded_identity(key: &str) -> impl Iterator<Item = u8> + '_ {
    key.bytes().map(|byte| {
        if is_pricing_key_separator(byte) {
            b'-'
        } else {
            byte.to_ascii_lowercase()
        }
    })
}

impl PricingRows {
    fn literal(&self, key: &str, lookup: Lookup) -> Option<RowMatch<'_>> {
        let id = self.entries.id(key)?;
        let found = self.at(id)?;
        lookup.includes(found.row).then_some(found)
    }

    pub(super) fn at(&self, id: usize) -> Option<RowMatch<'_>> {
        self.entries.rows.get(id).map(|row| RowMatch::new(id, row))
    }

    pub(super) fn matches(&self) -> impl Iterator<Item = RowMatch<'_>> {
        self.entries
            .rows
            .iter()
            .enumerate()
            .map(|(id, row)| RowMatch::new(id, row))
    }

    pub(super) fn normalized_matches(&self) -> impl Iterator<Item = (RowMatch<'_>, &str)> {
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

    fn indexed_rows<'a>(
        &'a self,
        identity: &str,
        index: Index,
        fold: bool,
    ) -> impl Iterator<Item = RowMatch<'a>> {
        let keys = index.sensitive(self.indexes()).get(identity);
        let folded = if fold {
            self.folded_ids(identity, index)
        } else {
            &[]
        };
        keys.into_iter()
            .flatten()
            .chain(folded)
            .filter_map(|id| self.at(*id))
    }

    fn folded_ids(&self, identity: &str, index: Index) -> &[usize] {
        let indexes = self
            .folded_indexes
            .get_or_init(|| FoldedIndexes::build(self.matches()));
        let ids = &indexes.0[index as usize];
        let compare = |id: &usize| {
            let found = RowMatch::new(*id, &self.entries.rows[*id]);
            folded_identity(index.key(found)).cmp(identity.bytes())
        };
        let start = ids.partition_point(|id| compare(id).is_lt());
        let end = start
            + ids[start..]
                .iter()
                .take_while(|id| compare(id).is_eq())
                .count();
        &ids[start..end]
    }

    pub(super) fn authoritative<'a>(
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
            self.indexed_rows(&identity, index, fold)
                .filter_map(|raw| Some((raw.key, visible(raw)?)))
        };
        let by_key = |select| {
            matches(select)
                .min_by_key(|(key, _)| (fold && !key.eq_ignore_ascii_case(name.raw()), *key))
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

    pub(super) fn exact(&self, name: Name<'_>, lookup: Lookup, fold: bool) -> Option<RowMatch<'_>> {
        if !fold && let Some(literal) = self.literal(name.raw(), lookup) {
            return Some(literal);
        }
        let identity = if fold {
            fold_case(name.identity())
        } else {
            Cow::Borrowed(name.identity())
        };
        self.indexed_rows(&identity, Index::Identity, fold)
            .filter(|found| lookup.includes(found.row))
            .min_by_key(|found| {
                (
                    fold && !found.key.eq_ignore_ascii_case(name.raw()),
                    found.key,
                )
            })
    }

    fn indexes(&self) -> &RowIndexes {
        self.indexes.get_or_init(|| {
            let mut indexes = RowIndexes::default();
            // NOTE(2026-11-21): Normalizing swaps ASCII bytes one for one, so a vendor id, which is a suffix of its key, normalizes to the same suffix of the normalized key. Reusing the cached normalized keys keeps every key to a single normalization.
            for (found, identity) in self.normalized_matches() {
                let index = if found.row.is_bare_hosted(found.key) {
                    &mut indexes.bare_hosted
                } else {
                    &mut indexes.by_identity
                };
                index.entry(identity.into()).or_default().push(found.id);
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
        self.entries.get(key).is_some_and(|row| row.exact_only)
            || self.exact_only_by_identity().contains_key(identity)
    }

    pub(super) fn exact_only_id(&self, key: &str) -> Option<&str> {
        let id = self
            .exact_only_by_identity()
            .get(normalized_pricing_key(key).as_ref())?;
        self.at((*id)?).map(|found| found.key)
    }
}
