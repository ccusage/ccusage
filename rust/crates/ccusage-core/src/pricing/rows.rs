use std::{borrow::Cow, sync::OnceLock};

use crate::fast::FxHashMap;

use super::{Entries, Name, Row, Scope, last_segment, normalized_pricing_key};

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
    pub(super) folded_indexes: OnceLock<RowIndexes>,
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

impl RowIndexes {
    fn insert(&mut self, found: RowMatch<'_>, identity: &str) {
        let row = found.row;
        let key = found.key;
        let index = if row.is_bare_hosted(key) {
            &mut self.bare_hosted
        } else {
            &mut self.by_identity
        };
        index.entry(identity.into()).or_default().push(found.id);
        let vendor_id = if row.scope == Scope::Direct && row.is_free() {
            Some(last_segment(key))
        } else {
            row.vendor_id()
        };
        if let Some(id) = vendor_id.filter(|id| *id != key) {
            self.by_vendor_id
                .entry(identity[identity.len() - id.len()..].into())
                .or_default()
                .push(found.id);
        }
    }
}

fn fold_case(identity: &str) -> Cow<'_, str> {
    if identity.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(identity.to_ascii_lowercase())
    } else {
        Cow::Borrowed(identity)
    }
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

    fn indexed_rows<'a, const FOLD: bool>(
        &'a self,
        identity: &str,
        select: impl Fn(&'a RowIndexes) -> &'a IdentityIndex,
    ) -> impl Iterator<Item = RowMatch<'a>> {
        let keys = select(self.indexes()).get(identity);
        let folded = if FOLD {
            select(self.folded_indexes()).get(identity)
        } else {
            None
        };
        keys.into_iter()
            .chain(folded)
            .flatten()
            .filter_map(|id| self.at(*id))
    }

    pub(super) fn authoritative<'a, const FOLD: bool>(
        &'a self,
        name: Name<'_>,
        hosted: bool,
        visible: impl Fn(RowMatch<'a>) -> Option<RowMatch<'a>>,
    ) -> Option<RowMatch<'a>> {
        let literal = if FOLD {
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
        let identity = if FOLD {
            fold_case(name.identity())
        } else {
            Cow::Borrowed(name.identity())
        };
        let matches = |select: fn(&RowIndexes) -> &IdentityIndex| {
            self.indexed_rows::<FOLD>(&identity, select)
                .filter_map(|raw| Some((raw.key, visible(raw)?)))
        };
        let by_key = |select| {
            matches(select)
                .min_by_key(|(key, _)| (FOLD && !key.eq_ignore_ascii_case(name.raw()), *key))
                .map(|(_, found)| found)
        };
        by_key(|indexes| &indexes.by_identity)
            .or_else(|| {
                let mut rows = matches(|indexes| &indexes.by_vendor_id);
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
                        .or_else(|| by_key(|indexes| &indexes.bare_hosted))
                } else {
                    None
                }
            })
    }

    pub(super) fn exact<const FOLD: bool>(
        &self,
        name: Name<'_>,
        lookup: Lookup,
    ) -> Option<RowMatch<'_>> {
        if !FOLD && let Some(literal) = self.literal(name.raw(), lookup) {
            return Some(literal);
        }
        let identity = if FOLD {
            fold_case(name.identity())
        } else {
            Cow::Borrowed(name.identity())
        };
        self.indexed_rows::<FOLD>(&identity, |indexes| &indexes.by_identity)
            .filter(|found| lookup.includes(found.row))
            .min_by_key(|found| {
                (
                    FOLD && !found.key.eq_ignore_ascii_case(name.raw()),
                    found.key,
                )
            })
    }

    fn indexes(&self) -> &RowIndexes {
        self.indexes.get_or_init(|| {
            let mut indexes = RowIndexes::default();
            // NOTE(2026-11-21): Normalizing swaps ASCII bytes one for one, so a vendor id, which is a suffix of its key, normalizes to the same suffix of the normalized key. Reusing the cached normalized keys keeps every key to a single normalization.
            for (found, identity) in self.normalized_matches() {
                indexes.insert(found, identity);
            }
            indexes
        })
    }

    fn folded_indexes(&self) -> &RowIndexes {
        self.folded_indexes.get_or_init(|| {
            let mut indexes = RowIndexes::default();
            // NOTE(2026-11-21, #2230): Lowercase identities already live in the case-sensitive indexes. Index only changed keys here to avoid duplicating the catalogue on the fallback path.
            for (found, identity) in self.normalized_matches() {
                if let Cow::Owned(identity) = fold_case(identity) {
                    indexes.insert(found, &identity);
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
