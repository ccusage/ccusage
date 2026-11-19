use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::{
    Lookup, Name, Pricing, PricingMap, PricingOverride, PricingRows, Row, RowMatch, Scope, Source,
    SpeedMultipliers, normalized_pricing_key, with_names,
};

#[derive(Debug, Default)]
pub(super) struct Overrides {
    by_identity: FxHashMap<Arc<str>, usize>,
    rows: Vec<OverrideRow>,
}

#[derive(Debug)]
struct OverrideRow {
    key: Arc<str>,
    identity: Arc<str>,
    base: Option<Base>,
    patches: SmallVec<[PricingOverride; 1]>,
    row: OnceLock<Row>,
}

#[derive(Debug)]
enum Base {
    Authority(Box<Row>),
    Override(usize),
}

impl Overrides {
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn id(&self, name: &str) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        self.id_identity(normalized_pricing_key(name).as_ref())
    }

    pub(super) fn id_identity(&self, identity: &str) -> Option<usize> {
        self.by_identity.get(identity).copied()
    }

    pub(super) fn resolved(&self, id: usize) -> Option<RowMatch<'_>> {
        self.rows
            .get(id)?
            .row
            .get()
            .map(|row| RowMatch::new(id, row))
    }

    pub(super) fn label_id(&self, label: &str) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        self.id(label)
            .or_else(|| self.id(crate::model_aliases::resolve_model_name(label).as_ref()))
    }

    pub(super) fn shadow(&self, found: RowMatch<'_>) -> Option<usize> {
        self.id(found.key)
            .or_else(|| self.id(found.row.vendor_id.as_deref()?))
    }

    pub(super) fn names(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rows
            .iter()
            .map(|slot| (slot.key.as_ref(), slot.identity.as_ref()))
    }

    fn in_cycle(&self, id: usize) -> bool {
        let mut current = id;
        for _ in 0..self.rows.len() {
            let Some(Base::Override(next)) = self.rows[current].base.as_ref() else {
                return false;
            };
            if *next == id {
                return true;
            }
            current = *next;
        }
        false
    }
}

impl PricingMap {
    pub(super) fn apply_overrides<'a, I>(&mut self, values: I)
    where
        I: IntoIterator<Item = (&'a String, &'a PricingOverride)>,
    {
        let mut values: Vec<_> = values.into_iter().collect();
        if values.is_empty() {
            return;
        }
        values.sort_unstable_by_key(|(key, _)| *key);
        let mut overrides = Overrides::default();
        for (key, value) in values {
            let identity = normalized_pricing_key(key);
            let id = match overrides.by_identity.get(identity.as_ref()) {
                Some(&id) => id,
                None => {
                    let id = overrides.rows.len();
                    let identity: Arc<str> = Arc::from(identity.as_ref());
                    overrides.by_identity.insert(Arc::clone(&identity), id);
                    overrides.rows.push(OverrideRow {
                        key: key.as_str().into(),
                        identity,
                        base: None,
                        patches: SmallVec::new(),
                        row: OnceLock::new(),
                    });
                    id
                }
            };
            overrides.rows[id].patches.push(value.clone());
        }
        for (id, slot) in overrides.rows.iter_mut().enumerate() {
            slot.base = self.override_base(id, &slot.key, &overrides.by_identity);
        }
        let in_cycle: Vec<_> = (0..overrides.rows.len())
            .filter(|id| overrides.in_cycle(*id))
            .collect();
        for id in in_cycle {
            let slot = &mut overrides.rows[id];
            slot.base = self.override_base(id, &slot.key, &FxHashMap::default());
        }
        self.overrides = Arc::new(overrides);
        self.clear_find_cache();
    }

    fn override_base(
        &self,
        id: usize,
        key: &str,
        overrides: &FxHashMap<Arc<str>, usize>,
    ) -> Option<Base> {
        with_names(key, |name| {
            if name != key
                && let Some(&dependency) = overrides.get(normalized_pricing_key(name).as_ref())
                && dependency != id
            {
                return Some(Base::Override(dependency));
            }
            self.rows
                .authoritative(Name::new(name, &OnceLock::new()), false, Some)
                .map(|found| Base::Authority(Box::new(found.row.clone())))
        })
    }

    pub(super) fn resolve_override<'a>(
        &'a self,
        id: usize,
        models_dev: &impl Fn() -> Option<&'a PricingRows>,
    ) -> RowMatch<'a> {
        let slot = &self.overrides.rows[id];
        let row = slot.row.get_or_init(|| {
            let (mut pricing, mut context_limit, schedule) = match &slot.base {
                Some(Base::Authority(row)) => (row.pricing, row.context_limit, row.schedule),
                Some(Base::Override(id)) => {
                    let row = self.resolve_override(*id, models_dev).row;
                    (row.pricing, row.context_limit, row.schedule)
                }
                None => {
                    let base = models_dev().and_then(|rows| {
                        with_names(&slot.key, |name| {
                            rows.exact(Name::new(name, &OnceLock::new()), Lookup::Pricing)
                        })
                    });
                    match base {
                        Some(found) => (
                            Pricing {
                                long_context: None,
                                ..found.row.pricing
                            },
                            found.row.context_limit,
                            None,
                        ),
                        None => {
                            let speed = SpeedMultipliers::get();
                            let alias = crate::model_aliases::resolve_model_name(&slot.key);
                            (
                                speed
                                    .for_model(&slot.key)
                                    .or(speed.for_model(&alias))
                                    .applied_to(Pricing::empty()),
                                None,
                                None,
                            )
                        }
                    }
                }
            };
            let mut explicit_fields = 0;
            for patch in &slot.patches {
                pricing.apply_override(patch);
                explicit_fields |= pricing.explicit_fields;
                context_limit = patch.max_input_tokens.or(context_limit);
            }
            pricing.explicit_fields = explicit_fields;
            Row {
                key: Arc::clone(&slot.key),
                source: Source::Override,
                scope: Scope::Direct,
                vendor_id: Some(slot.key.as_ref().into()),
                exact_only: OnceLock::new(),
                schedule,
                context_limit,
                pricing,
            }
        });
        RowMatch::new(id, row)
    }
}
