mod context_limits;
mod curated;
mod embedded;
mod exact_only;
mod fuzzy;
mod litellm;
mod long_context;
mod models_dev;
mod names;
mod overrides;
mod resolver;
mod scenarios;
mod schedule;
mod speed;

use super::{
    BillingRoute, Lookup, MatchKind, Pricing, PricingMap, Row, build_time_pricing_json,
    embedded::build_time_models_dev_json, embedded_models_dev_pricing, model_without_date_suffix,
};
use ccusage_test_support::fs_fixture;
use std::sync::atomic::{AtomicUsize, Ordering};

impl PricingMap {
    pub(crate) fn find_at(&self, model: &str, timestamp: crate::TimestampMs) -> Option<Pricing> {
        self.resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], Some(timestamp))
            .map(|(_, rates)| rates)
    }

    pub(crate) fn load_models_dev_json_for_tests(&mut self, json: &str) -> Option<usize> {
        self.load_models_dev_json_missing(json)
    }

    fn insert_test_pricing(&mut self, key: String, pricing: Pricing) {
        let row = self
            .rows_mut()
            .entries
            .get_or_insert(key, Row::default)
            .unwrap();
        row.scope = super::Scope::Direct;
        row.pricing = pricing.into();
    }

    fn insert_test_limit(&mut self, key: &str, limit: u64) {
        let row = self
            .rows_mut()
            .entries
            .get_or_insert(key.to_string(), Row::default)
            .unwrap();
        row.scope = super::Scope::Direct;
        row.context_limit = Some(limit);
    }

    fn mark_test_exact_only(&mut self, key: &str) {
        self.rows_mut()
            .entries
            .get_or_insert(key.to_string(), Row::default)
            .unwrap()
            .exact_only = true;
    }

    fn find_entry(&self, model: &str, kind: MatchKind) -> Option<Pricing> {
        self.resolve_at(Some(model), &[(model, kind)], None)
            .map(|(_, rates)| rates)
    }

    fn context_limit_entry(&self, model: &str, kind: MatchKind) -> Option<u64> {
        self.resolve_context_limit(model, kind, || None)
    }

    fn find_with_live<'a>(
        &self,
        model: &str,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> Option<Pricing> {
        self.resolve_row_with(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            Lookup::Pricing,
            live,
        )
        .map(|(_, found, _)| found.row.rates())
    }

    fn context_limit_with_live<'a>(
        &self,
        model: &str,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> Option<u64> {
        self.resolve_context_limit(model, MatchKind::Fuzzy, live)
    }

    fn with_live_layer<'a>(
        &self,
        model: &str,
        lookup: Lookup,
        live: impl FnOnce() -> Option<&'a Self>,
    ) -> &Self {
        self.resolve_row_with(Some(model), &[(model, MatchKind::Fuzzy)], lookup, live);
        self
    }

    fn len(&self) -> usize {
        let mut keys = rustc_hash::FxHashSet::default();
        keys.extend(self.rows.matches().map(|found| found.key));
        if let Some(models_dev) = self.available_models_dev() {
            keys.extend(models_dev.matches().map(|found| found.key));
        }
        keys.len()
    }

    fn models_dev_fallback_enabled(&self) -> bool {
        self.enable_models_dev_fallback
    }
}

fn online(primary: &PricingMap) -> PricingMap {
    let mut map = primary.with_user_overrides(std::iter::empty());
    map.enable_models_dev_fallback = true;
    map
}

fn layered(primary: &PricingMap, embedded_models_dev: Option<&PricingMap>) -> PricingMap {
    let empty = PricingMap::default();
    let mut map = online(primary);
    map.complete_layers(embedded_models_dev.unwrap_or(&empty));
    map
}

fn timestamp(value: &str) -> crate::TimestampMs {
    crate::parse_ts_timestamp(value).unwrap()
}
