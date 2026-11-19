use std::sync::{Arc, OnceLock, RwLock};

use rustc_hash::FxHashMap;

use super::{
    Lookup, MatchKind, ResolutionStep, RowMatch, Source, normalized_pricing_key, pricing_alias,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct CachedHit {
    pub(super) id: usize,
    pub(super) step: ResolutionStep,
    pub(super) overridden: bool,
}

impl CachedHit {
    pub(super) fn new(found: RowMatch<'_>, step: ResolutionStep) -> Self {
        Self {
            id: found.id,
            step,
            overridden: found.row.source == Source::Override,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Pass {
    Find,
    Label,
    S2,
    S34,
    S56,
}

impl Pass {
    pub(super) fn allows(self, kind: MatchKind) -> bool {
        match self {
            Self::S2 => true,
            Self::S34 => kind != MatchKind::StrictExact,
            Self::S56 => kind == MatchKind::Fuzzy,
            Self::Find | Self::Label => false,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Name<'a> {
    text: &'a str,
    offset: usize,
    normalized: &'a OnceLock<Option<Box<str>>>,
}

impl<'a> Name<'a> {
    pub(super) fn new(text: &'a str, normalized: &'a OnceLock<Option<Box<str>>>) -> Self {
        Self {
            text,
            normalized,
            offset: 0,
        }
    }

    pub(super) fn raw(self) -> &'a str {
        &self.text[self.offset..]
    }

    pub(super) fn identity(self) -> &'a str {
        let normalized = self
            .normalized
            .get_or_init(|| match normalized_pricing_key(self.text) {
                std::borrow::Cow::Borrowed(_) => None,
                std::borrow::Cow::Owned(id) => Some(id.into_boxed_str()),
            });
        &normalized.as_deref().unwrap_or(self.text)[self.offset..]
    }

    pub(super) fn suffixes(self) -> impl Iterator<Item = Self> {
        self.raw().match_indices('/').map(move |(offset, _)| Self {
            offset: self.offset + offset + 1,
            ..self
        })
    }
}

#[derive(Debug, Default)]
pub(super) struct NameSlot {
    normalized: OnceLock<Option<Box<str>>>,
    builtin_normalized: OnceLock<Option<Box<str>>>,
    configured: OnceLock<Option<Box<str>>>,
    configured_normalized: OnceLock<Option<Box<str>>>,
    configured_builtin_normalized: OnceLock<Option<Box<str>>>,
    prices: [OnceLock<Option<CachedHit>>; 5],
    limits: [OnceLock<Option<CachedHit>>; 5],
}

impl NameSlot {
    pub(super) fn name<'a>(&'a self, text: &'a str) -> Name<'a> {
        Name::new(text, &self.normalized)
    }

    pub(super) fn configured(&self, text: &str) -> Option<Name<'_>> {
        let resolved = self.configured.get_or_init(|| {
            let alias = crate::model_aliases::resolve_model_name(text);
            (alias != text).then(|| alias.into_owned().into_boxed_str())
        });
        Some(Name::new(resolved.as_deref()?, &self.configured_normalized))
    }

    pub(super) fn names<T>(
        &self,
        text: &str,
        mut select: impl FnMut(Name<'_>) -> Option<T>,
    ) -> Option<T> {
        select(self.name(text))
            .or_else(|| select(Name::new(pricing_alias(text)?, &self.builtin_normalized)))
            .or_else(|| {
                let alias = self.configured(text)?;
                select(alias).or_else(|| {
                    select(Name::new(
                        pricing_alias(alias.raw())?,
                        &self.configured_builtin_normalized,
                    ))
                })
            })
    }

    pub(super) fn result(&self, lookup: Lookup, pass: Pass) -> &OnceLock<Option<CachedHit>> {
        let slots = match lookup {
            Lookup::Pricing => &self.prices,
            Lookup::Context => &self.limits,
        };
        &slots[pass as usize]
    }
}

#[derive(Debug, Default)]
pub(super) struct NameCache(RwLock<FxHashMap<Box<str>, Arc<NameSlot>>>);

impl NameCache {
    pub(super) fn get_or_init(
        &self,
        name: &str,
        lookup: Lookup,
        pass: Pass,
        init: impl FnOnce(&NameSlot) -> Option<CachedHit>,
    ) -> Option<CachedHit> {
        let guard = self.0.read().unwrap_or_else(|error| error.into_inner());
        let slot = if let Some(slot) = guard.get(name) {
            if let Some(&result) = slot.result(lookup, pass).get() {
                return result;
            }
            Arc::clone(slot)
        } else {
            drop(guard);
            let mut guard = self.0.write().unwrap_or_else(|error| error.into_inner());
            match guard.get(name) {
                Some(slot) => Arc::clone(slot),
                None => {
                    let slot = Arc::new(NameSlot::default());
                    guard.insert(name.into(), Arc::clone(&slot));
                    slot
                }
            }
        };
        *slot.result(lookup, pass).get_or_init(|| init(&slot))
    }
}

#[cfg(test)]
mod tests {
    use crate::pricing::PricingMap;

    use super::{Lookup, MatchKind, Pass};

    #[test]
    fn reuses_resolution_slots_across_ten_thousand_requests() {
        crate::test_support::install_pricing_snapshots();
        for multi in [false, true] {
            let mut table = PricingMap::default();
            let rows: serde_json::Map<String, serde_json::Value> = (0..20)
                .map(|i| {
                    (
                        format!("catalogue/model{i}-1.2"),
                        serde_json::json!({"input_cost_per_token": 3, "output_cost_per_token": 4}),
                    )
                })
                .collect();
            table.load_json(&serde_json::Value::Object(rows).to_string());

            let names: Vec<_> = (0..20)
                .map(|i| {
                    (
                        format!("model{i}-1-2"),
                        format!("provider/unknown-{i}"),
                        format!("unknown-{i}"),
                    )
                })
                .collect();
            for (label, provider, other) in names.iter().cycle().take(10_000) {
                let single = [(label.as_str(), MatchKind::Fuzzy)];
                let multiple = [
                    (provider.as_str(), MatchKind::StrictExact),
                    (other.as_str(), MatchKind::Fuzzy),
                    (label.as_str(), MatchKind::Fuzzy),
                ];
                let candidates = if multi { &multiple[..] } else { &single[..] };
                let rates = table.resolve_at(Some(label), candidates, None).unwrap().1;
                assert_eq!((rates.input, rates.output), (3.0, 4.0));
            }

            let cache = table.find_cache.get().unwrap().0.read().unwrap();
            assert_eq!(cache.len(), if multi { 60 } else { 20 });
            assert_eq!(
                cache
                    .values()
                    .filter(|slot| slot.normalized.get().is_some())
                    .count(),
                cache.len()
            );
            assert_eq!(
                cache
                    .values()
                    .filter(|slot| slot.result(Lookup::Pricing, Pass::S56).get().is_some())
                    .count(),
                if multi { 40 } else { 20 }
            );
        }
    }

    #[test]
    fn literal_hits_leave_the_identity_indexes_uninitialised() {
        crate::test_support::install_pricing_snapshots();
        let mut table = PricingMap::default();
        table.load_json(
            r#"{"provider/literal":{"input_cost_per_token":3,"output_cost_per_token":4}}"#,
        );
        for _ in 0..10_000 {
            assert_eq!(table.find("provider/literal").unwrap().input, 3.0);
        }
        assert!(table.rows.indexes.get().is_none());
        assert!(
            table.find_cache.get().unwrap().0.read().unwrap()["provider/literal"]
                .normalized
                .get()
                .is_none()
        );
    }
}
