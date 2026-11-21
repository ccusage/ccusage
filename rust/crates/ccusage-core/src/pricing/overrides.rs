use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::{
    Candidate, LongContext, LongContextRates, Lookup, Name, Pricing, PricingMap, PricingOverride,
    PricingRows, Row, RowMatch, Scope, Source, SpeedMultipliers, normalized_pricing_key,
    row::cache_defaults, with_names,
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
    authoritative: OnceLock<Option<Row>>,
    row: OnceLock<Option<Row>>,
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
        let slot = self.rows.get(id)?;
        slot.row
            .get()
            .and_then(Option::as_ref)
            .or_else(|| slot.authoritative.get().and_then(Option::as_ref))
            .map(|row| RowMatch::new(id, row))
    }

    pub(super) fn label_id(&self, label: &str, slot: &Candidate<'_>) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        let name = slot.name(label);
        self.id_identity(name.identity())
            .or_else(|| self.id_identity(slot.configured(label)?.identity()))
            .or_else(|| {
                name.suffixes()
                    .find_map(|suffix| self.id_identity(suffix.identity()))
            })
    }

    pub(super) fn shadow(&self, found: RowMatch<'_>) -> Option<usize> {
        self.id(found.key)
            .or_else(|| self.id(found.row.vendor_id()?))
    }

    pub(super) fn names(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rows
            .iter()
            .map(|slot| (slot.key.as_ref(), slot.identity.as_ref()))
    }

    fn published_tier(&self, id: usize) -> Option<LongContext> {
        let mut base = self.rows.get(id)?.base.as_ref();
        while let Some(Base::Override(dependency)) = base {
            base = self.rows.get(*dependency)?.base.as_ref();
        }
        match base? {
            Base::Authority(row) => row.rates().long_context,
            Base::Override(_) => None,
        }
    }

    fn has_authoritative_base(&self, id: usize) -> Option<bool> {
        let mut base = self.rows.get(id)?.base.as_ref();
        while let Some(Base::Override(dependency)) = base {
            base = self.rows.get(*dependency)?.base.as_ref();
        }
        Some(base.is_some())
    }

    fn in_cycle(&self, id: usize) -> bool {
        let mut current = id;
        for _ in 0..self.rows.len() {
            let Some(Base::Override(next)) =
                self.rows.get(current).and_then(|slot| slot.base.as_ref())
            else {
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
        self.apply_override_list(values.into_iter().collect());
    }

    fn apply_override_list(&mut self, mut values: Vec<(&String, &PricingOverride)>) {
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
                        authoritative: OnceLock::new(),
                        row: OnceLock::new(),
                    });
                    id
                }
            };
            if let Some(slot) = overrides.rows.get_mut(id) {
                slot.patches.push(value.clone());
            }
        }
        for (id, slot) in overrides.rows.iter_mut().enumerate() {
            slot.base = self.override_base(id, &slot.key, &overrides.by_identity);
        }
        let in_cycle: Vec<_> = (0..overrides.rows.len())
            .filter(|id| overrides.in_cycle(*id))
            .collect();
        for id in in_cycle {
            if let Some(slot) = overrides.rows.get_mut(id) {
                slot.base = self.override_base(id, &slot.key, &FxHashMap::default());
            }
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
        needs_fuzzy_flag: bool,
    ) -> Option<RowMatch<'a>> {
        let slot = self.overrides.rows.get(id)?;
        if let Some(row) = slot.row.get() {
            return row.as_ref().map(|row| RowMatch::new(id, row));
        }
        let select_catalogue = needs_fuzzy_flag || !self.overrides.has_authoritative_base(id)?;
        let cell = if select_catalogue {
            &slot.row
        } else {
            &slot.authoritative
        };
        let row = cell.get_or_init(|| {
            let secondary = if select_catalogue { models_dev() } else { None };
            let exact_only = secondary
                .is_some_and(|rows| rows.contains_exact_only_identity(&slot.key, &slot.identity));
            if let Some(authority) = slot.authoritative.get().and_then(Option::as_ref) {
                return Some(Row {
                    exact_only,
                    ..authority.clone()
                });
            }
            let (mut pricing, mut context_limit, schedule) = match &slot.base {
                Some(Base::Authority(row)) => (row.rates(), row.context_limit, row.schedule),
                Some(Base::Override(id)) => {
                    let row = self.resolve_override(*id, models_dev, false)?.row;
                    (row.rates(), row.context_limit, row.schedule)
                }
                None => {
                    let base = secondary.and_then(|rows| {
                        with_names(&slot.key, |name| {
                            rows.exact(Name::new(name, &OnceLock::new()), Lookup::Pricing)
                        })
                    });
                    match base {
                        Some(found) => (
                            Pricing {
                                long_context: None,
                                ..found.row.rates()
                            },
                            found.row.context_limit,
                            None,
                        ),
                        None => {
                            let speed = SpeedMultipliers::get();
                            let alias = crate::model_aliases::resolve_model_name(&slot.key);
                            let model = slot
                                .key
                                .strip_prefix('[')
                                .and_then(|label| label.split_once("] "))
                                .map_or(slot.key.as_ref(), |(_, model)| model);
                            (
                                speed
                                    .for_model(&slot.key)
                                    .or(speed.for_model(&alias))
                                    .applied_to(Pricing {
                                        cache_defaults: cache_defaults(model, None),
                                        ..Pricing::empty()
                                    }),
                                None,
                                None,
                            )
                        }
                    }
                }
            };
            if matches!(slot.base, Some(Base::Override(_))) && pricing.long_context.is_none() {
                pricing.long_context = self.overrides.published_tier(id).map(|tier| LongContext {
                    rates: LongContextRates::default(),
                    ..tier
                });
            }
            let mut explicit_fields = pricing.explicit_long_rate_fields();
            for patch in &slot.patches {
                pricing.apply_override(patch);
                explicit_fields |= pricing.explicit_fields;
                context_limit = patch.max_input_tokens.or(context_limit);
            }
            pricing.explicit_fields = explicit_fields;
            pricing.replace_tier_with_explicit_rates();
            pricing.long_context = pricing
                .long_context
                .filter(|tier| tier.rates.marginal().is_some());
            Some(Row {
                key: Arc::clone(&slot.key),
                source: Source::Override,
                scope: Scope::Direct,
                vendor_start: super::vendor_start(&slot.key, &slot.key),
                exact_only,
                schedule,
                context_limit,
                pricing: pricing.into(),
            })
        });
        row.as_ref().map(|row| RowMatch::new(id, row))
    }
}
