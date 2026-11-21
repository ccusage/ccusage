use std::borrow::Cow;
use std::cell::OnceCell;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock, RwLock};

use rustc_hash::{FxHashMap, FxHasher};

#[cfg(feature = "resolver-trace")]
use super::ResolutionStep;
use super::{
    Lookup, MatchKind, Row, RowMatch, Source, alternate_spellings, normalized_pricing_key,
};

pub(super) enum HitKind {
    ReportedOverride,
    AuthoritativeIdentity,
    AuthoritativeSuffix,
    ModelsDevIdentity,
    AuthoritativeFuzzy,
    ModelsDevFuzzy,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CachedHit(usize);

impl CachedHit {
    const OVERRIDDEN: usize = 1;
    const PRESENT: usize = 2;
    const STEP_SHIFT: usize = 2;
    const ALIASED: usize = 1 << 5;
    const ESTIMATED: usize = 1 << 6;
    const FOLDED: usize = 1 << 7;
    const ROW_SHIFT: usize = 8;
    const STEP_MASK: usize = 7;
    #[cfg(feature = "resolver-trace")]
    const STEPS: [ResolutionStep; 6] = [
        ResolutionStep::ReportedOverride,
        ResolutionStep::AuthoritativeIdentity,
        ResolutionStep::AuthoritativeSuffix,
        ResolutionStep::ModelsDevIdentity,
        ResolutionStep::AuthoritativeFuzzy,
        ResolutionStep::ModelsDevFuzzy,
    ];

    pub(super) fn new(found: RowMatch<'_>, kind: HitKind) -> Self {
        let overridden = if found.row.source == Source::Override {
            Self::OVERRIDDEN
        } else {
            0
        };
        Self(
            (found.id << Self::ROW_SHIFT)
                | ((kind as usize) << Self::STEP_SHIFT)
                | Self::PRESENT
                | overridden,
        )
    }

    pub(super) fn through_alias(self, aliased: bool) -> Self {
        if aliased {
            Self(self.0 | Self::ALIASED)
        } else {
            self
        }
    }

    pub(super) fn aliased(self) -> bool {
        self.0 & Self::ALIASED != 0
    }

    pub(super) fn as_estimate(self) -> Self {
        Self(self.0 | Self::ESTIMATED)
    }

    pub(super) fn estimated(self) -> bool {
        self.0 & Self::ESTIMATED != 0
    }

    pub(super) fn through_case_fallback(self) -> Self {
        Self(self.0 | Self::FOLDED)
    }

    pub(super) fn case_fallback(self) -> bool {
        self.0 & Self::FOLDED != 0
    }

    pub(super) fn same_row(self, other: Self) -> bool {
        self.id() == other.id()
            && self.overridden() == other.overridden()
            && self.is_models_dev() == other.is_models_dev()
    }

    pub(super) fn id(self) -> usize {
        self.0 >> Self::ROW_SHIFT
    }

    fn step_index(self) -> usize {
        (self.0 >> Self::STEP_SHIFT) & Self::STEP_MASK
    }

    #[cfg(feature = "resolver-trace")]
    pub(super) fn step(self) -> Option<ResolutionStep> {
        Self::STEPS.get(self.step_index()).copied()
    }

    pub(super) fn scheduled(self) -> bool {
        self.step_index() <= HitKind::AuthoritativeIdentity as usize
    }

    pub(super) fn is_models_dev(self) -> bool {
        let step = self.step_index();
        step == HitKind::ModelsDevIdentity as usize || step == HitKind::ModelsDevFuzzy as usize
    }

    pub(super) fn overridden(self) -> bool {
        self.0 & Self::OVERRIDDEN != 0
    }
}

const _: () = assert!(size_of::<CachedHit>() <= 8);
// NOTE(2026-11-21, #2228): A row index is below `isize::MAX / size_of::<Row>()`, the most rows a `Vec<Row>` can hold, so shifting it left by `ROW_SHIFT` must still fit in a `usize`.
const _: () = assert!(isize::MAX as usize / size_of::<Row>() <= usize::MAX >> CachedHit::ROW_SHIFT);

#[derive(Clone, Copy, Debug, Default)]
struct ResultCell(usize);

impl ResultCell {
    const UNSET: usize = 0;
    const MISS: usize = 1;

    fn get(self) -> Option<Option<CachedHit>> {
        match self.0 {
            Self::UNSET => None,
            Self::MISS => Some(None),
            hit => Some(Some(CachedHit(hit))),
        }
    }

    fn insert(&mut self, result: Option<CachedHit>) -> Option<CachedHit> {
        if let Some(existing) = self.get() {
            return existing;
        }
        self.0 = result.map_or(Self::MISS, |hit| hit.0);
        result
    }
}

#[derive(Clone, Copy)]
pub(super) enum Pass {
    Label,
    S2,
    S34,
    S3f,
    S56,
}

impl Pass {
    pub(super) const IDENTITY: [Self; 3] = [Self::S2, Self::S34, Self::S3f];

    pub(super) fn searches(self, lookup: Lookup) -> bool {
        !matches!((lookup, self), (Lookup::Context | Lookup::Paid, Self::S3f))
    }

    pub(super) fn allows(self, kind: MatchKind) -> bool {
        match self {
            Self::S2 => true,
            Self::S34 | Self::S3f => kind != MatchKind::StrictExact,
            Self::S56 => matches!(kind, MatchKind::Fuzzy | MatchKind::Fallback),
            Self::Label => false,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Name<'a> {
    text: &'a str,
    identity: Identity<'a>,
}

#[derive(Clone, Copy)]
enum Identity<'a> {
    Standalone(&'a OnceLock<Option<Box<str>>>),
    Candidate(&'a Candidate<'a>),
    Resolved(Option<&'a str>),
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
        }
    }

    pub(super) fn raw(self) -> &'a str {
        self.text
    }

    pub(super) fn identity(self) -> &'a str {
        let identity = match self.identity {
            Identity::Standalone(cell) => cell.get_or_init(|| normalized(self.text)).as_deref(),
            Identity::Candidate(slot) => slot
                .forms(self.text)
                .and_then(|forms| forms.normalized.as_deref()),
            Identity::Resolved(identity) => identity,
        };
        identity.unwrap_or(self.text)
    }

    pub(super) fn suffixes(self) -> impl Iterator<Item = Self> {
        self.raw()
            .match_indices('/')
            .scan(None, move |identity, (offset, _)| {
                let raw = self.raw().get(offset + 1..)?;
                let (_, suffix) = identity
                    .unwrap_or_else(|| self.identity())
                    .split_once('/')?;
                *identity = Some(suffix);
                Some(Self {
                    text: raw,
                    identity: Identity::Resolved(Some(suffix)),
                })
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
            identity: Identity::Resolved(self.normalized.as_deref()),
        }
    }
}

#[derive(Debug)]
struct NameForms {
    normalized: Option<Box<str>>,
    spellings: Box<[Alias<Cow<'static, str>>]>,
    configured: Option<Alias<Box<str>>>,
    configured_spellings: Box<[Alias<Cow<'static, str>>]>,
}

#[derive(Debug, Default)]
struct NameSlot {
    forms: Option<Option<Arc<NameForms>>>,
    prices: [ResultCell; 5],
    limits: Option<Box<[ResultCell; 5]>>,
    paid: Option<Box<[ResultCell; 5]>>,
}

const _: () = assert!(size_of::<NameSlot>() <= 96);

impl NameSlot {
    fn result(&self, lookup: Lookup, pass: Pass) -> Option<Option<CachedHit>> {
        let slots = match lookup {
            Lookup::Pricing => &self.prices,
            Lookup::Context => self.limits.as_deref()?,
            Lookup::Paid => self.paid.as_deref()?,
        };
        slots[pass as usize].get()
    }

    fn insert(
        &mut self,
        lookup: Lookup,
        pass: Pass,
        result: Option<CachedHit>,
    ) -> Option<CachedHit> {
        let slots = match lookup {
            Lookup::Pricing => &mut self.prices,
            Lookup::Context => self.limits.get_or_insert_with(Box::default),
            Lookup::Paid => self.paid.get_or_insert_with(Box::default),
        };
        slots[pass as usize].insert(result)
    }
}

impl NameForms {
    fn new(text: &str) -> Option<Arc<Self>> {
        let normalized = normalized(text);
        let spellings = |text: &str| alternate_spellings(text).map(Alias::new).collect();
        let own: Box<[_]> = spellings(text);
        let alias = crate::model_aliases::resolve_model_name(text);
        let configured = (alias != text).then(|| Alias::new(alias.into_owned().into_boxed_str()));
        if normalized.is_none() && own.is_empty() && configured.is_none() {
            return None;
        }
        let configured_spellings = configured
            .as_ref()
            .map_or_else(Box::default, |alias| spellings(&alias.text));
        Some(Arc::new(Self {
            normalized,
            spellings: own,
            configured,
            configured_spellings,
        }))
    }
}

#[derive(Default)]
pub(super) struct Candidate<'a> {
    cache: Option<&'a NameCache>,
    forms: OnceCell<Option<Arc<NameForms>>>,
}

impl Candidate<'_> {
    fn forms(&self, text: &str) -> Option<&NameForms> {
        self.forms
            .get_or_init(|| match self.cache {
                Some(cache) => cache.forms(text),
                None => NameForms::new(text),
            })
            .as_deref()
    }

    pub(super) fn name<'a>(&'a self, text: &'a str) -> Name<'a> {
        Name {
            text,
            identity: Identity::Candidate(self),
        }
    }

    pub(super) fn configured(&self, text: &str) -> Option<Name<'_>> {
        Some(self.forms(text)?.configured.as_ref()?.name())
    }

    pub(super) fn own_names<T>(
        &self,
        text: &str,
        mut select: impl FnMut(Name<'_>) -> Option<T>,
    ) -> Option<T> {
        select(self.name(text)).or_else(|| {
            let forms = self.forms(text)?;
            forms.spellings.iter().map(Alias::name).find_map(select)
        })
    }

    pub(super) fn names<T>(
        &self,
        text: &str,
        select: impl FnMut(Name<'_>) -> Option<T>,
    ) -> Option<T> {
        self.names_marked(text, select).map(|(found, _)| found)
    }

    pub(super) fn names_marked<T>(
        &self,
        text: &str,
        mut select: impl FnMut(Name<'_>) -> Option<T>,
    ) -> Option<(T, bool)> {
        if let Some(found) = self.own_names(text, &mut select) {
            return Some((found, false));
        }
        let forms = self.forms(text)?;
        forms
            .configured
            .iter()
            .map(Alias::name)
            .chain(forms.configured_spellings.iter().map(Alias::name))
            .find_map(select)
            .map(|found| (found, true))
    }
}

#[derive(Debug, Default)]
pub(super) struct NameCache(RwLock<FxHashMap<Box<str>, NameSlot>>);

impl NameCache {
    pub(super) fn candidate(&self) -> Candidate<'_> {
        Candidate {
            cache: Some(self),
            forms: OnceCell::new(),
        }
    }

    fn with_slot<T>(&self, text: &str, update: impl FnOnce(&mut NameSlot) -> T) -> T {
        let mut guard = self.0.write().unwrap_or_else(|error| error.into_inner());
        if let Some(slot) = guard.get_mut(text) {
            return update(slot);
        }
        let mut slot = NameSlot::default();
        let result = update(&mut slot);
        guard.insert(text.into(), slot);
        result
    }

    fn forms(&self, text: &str) -> Option<Arc<NameForms>> {
        {
            let guard = self.0.read().unwrap_or_else(|error| error.into_inner());
            if let Some(forms) = guard.get(text).and_then(|slot| slot.forms.as_ref()) {
                return forms.clone();
            }
        }
        let forms = NameForms::new(text);
        self.with_slot(text, |slot| slot.forms.get_or_insert(forms).clone())
    }

    fn get(&self, name: &str, lookup: Lookup, pass: Pass) -> Option<Option<CachedHit>> {
        let guard = self.0.read().unwrap_or_else(|error| error.into_inner());
        guard.get(name)?.result(lookup, pass)
    }

    pub(super) fn get_or_init(
        &self,
        name: &str,
        lookup: Lookup,
        pass: Pass,
        init: impl FnOnce(&Candidate<'_>) -> Option<CachedHit>,
    ) -> Option<CachedHit> {
        if let Some(hit) = self.get(name, lookup, pass) {
            return hit;
        }
        let result = init(&self.candidate());
        self.with_slot(name, |slot| slot.insert(lookup, pass, result))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RequestHit {
    pub(super) candidate: Option<usize>,
    pub(super) row: CachedHit,
}

#[derive(Debug)]
struct RequestSlot {
    reported: Option<Box<str>>,
    candidates: Box<[(Box<str>, MatchKind)]>,
    hit: Arc<OnceLock<Option<RequestHit>>>,
}

impl RequestSlot {
    fn matches(&self, reported: Option<&str>, candidates: &[(&str, MatchKind)]) -> bool {
        self.reported.as_deref() == reported
            && self.candidates.len() == candidates.len()
            && self
                .candidates
                .iter()
                .zip(candidates)
                .all(|((name, kind), candidate)| (name.as_ref(), *kind) == *candidate)
    }
}

#[derive(Debug, Default)]
pub(super) struct RequestCache(RwLock<FxHashMap<u64, Vec<RequestSlot>>>);

impl RequestCache {
    pub(super) fn get_or_init(
        &self,
        reported: Option<&str>,
        candidates: &[(&str, MatchKind)],
        init: impl FnOnce() -> Option<RequestHit>,
    ) -> Option<RequestHit> {
        let mut hasher = FxHasher::default();
        reported.hash(&mut hasher);
        candidates.hash(&mut hasher);
        let hash = hasher.finish();
        let matches = |slot: &&RequestSlot| slot.matches(reported, candidates);
        let pending = {
            let table = self.0.read().unwrap_or_else(|error| error.into_inner());
            if let Some(slot) = table
                .get(&hash)
                .and_then(|slots| slots.iter().find(matches))
            {
                if let Some(hit) = slot.hit.get() {
                    return *hit;
                }
                Some(Arc::clone(&slot.hit))
            } else {
                None
            }
        };
        let slot = pending.unwrap_or_else(|| {
            let mut table = self.0.write().unwrap_or_else(|error| error.into_inner());
            let slots = table.entry(hash).or_insert_with(|| Vec::with_capacity(1));
            if let Some(slot) = slots.iter().find(matches) {
                return Arc::clone(&slot.hit);
            }
            let hit = Arc::new(OnceLock::new());
            slots.push(RequestSlot {
                reported: reported.map(Into::into),
                candidates: candidates
                    .iter()
                    .map(|(name, kind)| ((*name).into(), *kind))
                    .collect(),
                hit: Arc::clone(&hit),
            });
            hit
        });
        *slot.get_or_init(init)
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
            crate::parallel::spawn_scoped_worker(scope, move || {
                cache.get_or_init("existing", Lookup::Pricing, Pass::S2, |_| {
                    entered.send(()).unwrap();
                    wait_for_release.recv().unwrap();
                    None
                });
            });
            wait_for_init.recv_timeout(Duration::from_secs(5)).unwrap();
            crate::parallel::spawn_scoped_worker(scope, move || {
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

            let requests = table.requests.get().unwrap().0.read().unwrap();
            assert_eq!(requests.values().map(Vec::len).sum::<usize>(), 20);
            assert!(
                requests
                    .values()
                    .flatten()
                    .all(|slot| slot.hit.get().is_some())
            );

            let cache = table.find_cache.get().unwrap().0.read().unwrap();
            assert_eq!(cache.len(), if multi { 60 } else { 20 });
            assert!(
                cache
                    .values()
                    .all(|slot| { slot.limits.is_none() && matches!(slot.forms, Some(None)) })
            );
            assert_eq!(
                cache.values().filter(|slot| slot.forms.is_some()).count(),
                cache.len()
            );
            assert_eq!(
                cache
                    .values()
                    .filter(|slot| slot.result(Lookup::Pricing, Pass::S56).is_some())
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
        assert!(table.rows.folded_indexes.get().is_none());
        assert!(
            table.find_cache.get().unwrap().0.read().unwrap()["provider/literal"]
                .forms
                .is_none()
        );
    }
}
