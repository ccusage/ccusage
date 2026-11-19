use std::borrow::Cow;
use std::sync::{
    Arc, OnceLock, RwLock,
    atomic::{AtomicUsize, Ordering},
};

use rustc_hash::FxHashMap;

use super::{
    Lookup, MatchKind, ResolutionStep, Row, RowMatch, Source, normalized_pricing_key, pricing_alias,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct CachedHit(usize);

impl CachedHit {
    const OVERRIDDEN: usize = 1;
    const PRESENT: usize = 2;
    const STEP_SHIFT: usize = 2;
    const ROW_SHIFT: usize = 5;
    const STEP_MASK: usize = 7;
    const STEPS: [ResolutionStep; 6] = [
        ResolutionStep::ReportedOverride,
        ResolutionStep::AuthoritativeIdentity,
        ResolutionStep::AuthoritativeSuffix,
        ResolutionStep::ModelsDevIdentity,
        ResolutionStep::AuthoritativeFuzzy,
        ResolutionStep::ModelsDevFuzzy,
    ];

    pub(super) fn new(found: RowMatch<'_>, step: ResolutionStep) -> Self {
        let overridden = if found.row.source == Source::Override {
            Self::OVERRIDDEN
        } else {
            0
        };
        Self(
            (found.id << Self::ROW_SHIFT)
                | ((step as usize) << Self::STEP_SHIFT)
                | Self::PRESENT
                | overridden,
        )
    }

    pub(super) fn id(self) -> usize {
        self.0 >> Self::ROW_SHIFT
    }

    pub(super) fn step(self) -> Option<ResolutionStep> {
        Self::STEPS
            .get((self.0 >> Self::STEP_SHIFT) & Self::STEP_MASK)
            .copied()
    }

    pub(super) fn overridden(self) -> bool {
        self.0 & Self::OVERRIDDEN != 0
    }
}

const _: () = assert!(size_of::<CachedHit>() <= 8);
const _: () = assert!(size_of::<Row>() >= 1 << CachedHit::ROW_SHIFT);

#[derive(Debug, Default)]
pub(super) struct ResultCell(AtomicUsize);

impl ResultCell {
    const UNSET: usize = 0;
    const MISS: usize = 1;

    fn unpack(value: usize) -> Option<Option<CachedHit>> {
        match value {
            Self::UNSET => None,
            Self::MISS => Some(None),
            hit => Some(Some(CachedHit(hit))),
        }
    }

    pub(super) fn get(&self) -> Option<Option<CachedHit>> {
        Self::unpack(self.0.load(Ordering::Acquire))
    }

    pub(super) fn get_or_init(
        &self,
        init: impl FnOnce() -> Option<CachedHit>,
    ) -> Option<CachedHit> {
        if let Some(hit) = self.get() {
            return hit;
        }
        let hit = init();
        let value = hit.map_or(Self::MISS, |hit| hit.0);
        match self
            .0
            .compare_exchange(Self::UNSET, value, Ordering::Release, Ordering::Acquire)
        {
            Ok(_) => hit,
            Err(existing) => Self::unpack(existing).flatten(),
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
    identity: Identity<'a>,
}

#[derive(Clone, Copy)]
enum Identity<'a> {
    Standalone(&'a OnceLock<Option<Box<str>>>),
    Candidate(&'a NameSlot),
    Alias(Option<&'a str>),
}

fn normalized(text: &str) -> Option<Box<str>> {
    match normalized_pricing_key(text) {
        Cow::Borrowed(_) => None,
        Cow::Owned(id) => Some(id.into_boxed_str()),
    }
}

impl<'a> Name<'a> {
    pub(super) fn new(text: &'a str, normalized: &'a OnceLock<Option<Box<str>>>) -> Self {
        Self {
            text,
            identity: Identity::Standalone(normalized),
            offset: 0,
        }
    }

    pub(super) fn raw(self) -> &'a str {
        &self.text[self.offset..]
    }

    pub(super) fn identity(self) -> &'a str {
        let identity = match self.identity {
            Identity::Standalone(cell) => cell.get_or_init(|| normalized(self.text)).as_deref(),
            Identity::Candidate(slot) => slot
                .forms(self.text)
                .and_then(|forms| forms.normalized.as_deref()),
            Identity::Alias(identity) => identity,
        };
        &identity.unwrap_or(self.text)[self.offset..]
    }

    pub(super) fn suffixes(self) -> impl Iterator<Item = Self> {
        self.raw().match_indices('/').map(move |(offset, _)| Self {
            offset: self.offset + offset + 1,
            ..self
        })
    }
}

#[derive(Debug)]
struct Alias<T> {
    text: T,
    normalized: Option<Box<str>>,
}

impl<T: AsRef<str>> Alias<T> {
    fn new(text: T) -> Self {
        Self {
            normalized: normalized(text.as_ref()),
            text,
        }
    }

    fn name(&self) -> Name<'_> {
        Name {
            text: self.text.as_ref(),
            offset: 0,
            identity: Identity::Alias(self.normalized.as_deref()),
        }
    }
}

#[derive(Debug)]
struct NameForms {
    normalized: Option<Box<str>>,
    builtin: Option<Alias<&'static str>>,
    configured: Option<Alias<Box<str>>>,
    configured_builtin: Option<Alias<&'static str>>,
}

#[derive(Debug, Default)]
pub(super) struct NameSlot {
    forms: OnceLock<Option<Box<NameForms>>>,
    prices: [ResultCell; 5],
    limits: OnceLock<Box<[ResultCell; 5]>>,
}

const _: () = assert!(size_of::<NameSlot>() <= 96);

impl NameSlot {
    fn forms(&self, text: &str) -> Option<&NameForms> {
        self.forms
            .get_or_init(|| {
                let normalized = normalized(text);
                let builtin = pricing_alias(text).map(Alias::new);
                let alias = crate::model_aliases::resolve_model_name(text);
                let configured =
                    (alias != text).then(|| Alias::new(alias.into_owned().into_boxed_str()));
                if normalized.is_none() && builtin.is_none() && configured.is_none() {
                    return None;
                }
                let configured_builtin = configured
                    .as_ref()
                    .and_then(|alias| pricing_alias(&alias.text))
                    .map(Alias::new);
                Some(Box::new(NameForms {
                    normalized,
                    builtin,
                    configured,
                    configured_builtin,
                }))
            })
            .as_deref()
    }

    pub(super) fn name<'a>(&'a self, text: &'a str) -> Name<'a> {
        Name {
            text,
            offset: 0,
            identity: Identity::Candidate(self),
        }
    }

    pub(super) fn configured(&self, text: &str) -> Option<Name<'_>> {
        Some(self.forms(text)?.configured.as_ref()?.name())
    }

    pub(super) fn names<T>(
        &self,
        text: &str,
        mut select: impl FnMut(Name<'_>) -> Option<T>,
    ) -> Option<T> {
        select(self.name(text)).or_else(|| {
            let forms = self.forms(text)?;
            [
                forms.builtin.as_ref().map(Alias::name),
                forms.configured.as_ref().map(Alias::name),
                forms.configured_builtin.as_ref().map(Alias::name),
            ]
            .into_iter()
            .flatten()
            .find_map(select)
        })
    }

    pub(super) fn result(&self, lookup: Lookup, pass: Pass) -> &ResultCell {
        let slots = match lookup {
            Lookup::Pricing => &self.prices,
            Lookup::Context => self.limits.get_or_init(Box::default),
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
        let slot = {
            let guard = self.0.read().unwrap_or_else(|error| error.into_inner());
            if let Some(slot) = guard.get(name) {
                if let Some(result) = slot.result(lookup, pass).get() {
                    return result;
                }
                Some(Arc::clone(slot))
            } else {
                None
            }
        };
        let slot = slot.unwrap_or_else(|| {
            let mut guard = self.0.write().unwrap_or_else(|error| error.into_inner());
            if let Some(slot) = guard.get(name) {
                return Arc::clone(slot);
            }
            let slot = Arc::new(NameSlot::default());
            guard.insert(name.into(), Arc::clone(&slot));
            slot
        });
        slot.result(lookup, pass).get_or_init(|| init(&slot))
    }
}

#[cfg(test)]
mod tests {
    use crate::pricing::PricingMap;

    use super::{Lookup, MatchKind, Pass};

    #[test]
    fn initializing_an_existing_slot_does_not_block_a_new_name() {
        use std::{sync::mpsc, thread, time::Duration};

        let cache = super::NameCache::default();
        assert!(
            cache
                .get_or_init("existing", Lookup::Pricing, Pass::Label, |_| None)
                .is_none()
        );
        let (entered, wait_for_init) = mpsc::channel();
        let (release, wait_for_release) = mpsc::channel();
        let (inserted, wait_for_insert) = mpsc::channel();
        thread::scope(|scope| {
            let cache = &cache;
            scope.spawn(move || {
                cache.get_or_init("existing", Lookup::Pricing, Pass::S2, |_| {
                    entered.send(()).unwrap();
                    wait_for_release.recv().unwrap();
                    None
                });
            });
            wait_for_init.recv_timeout(Duration::from_secs(5)).unwrap();
            scope.spawn(move || {
                cache.get_or_init("new", Lookup::Pricing, Pass::S2, |_| None);
                inserted.send(()).unwrap();
            });
            let result = wait_for_insert.recv_timeout(Duration::from_secs(2));
            release.send(()).unwrap();
            result.unwrap();
        });
    }

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
            assert!(cache.values().all(|slot| {
                slot.limits.get().is_none() && matches!(slot.forms.get(), Some(None))
            }));
            assert_eq!(
                cache
                    .values()
                    .filter(|slot| slot.forms.get().is_some())
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
                .forms
                .get()
                .is_none()
        );
    }
}
