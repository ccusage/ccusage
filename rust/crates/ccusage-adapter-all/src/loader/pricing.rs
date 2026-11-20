use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::RunSource;
use crate::{PricingMap, cli::SharedArgs};

struct CachedPricingMap {
    overrides: BTreeMap<String, crate::cli::PricingOverride>,
    pricing: Arc<PricingMap>,
}

fn cached_pricing_map(
    shared: &SharedArgs,
    base: &Arc<PricingMap>,
    cached: &mut Vec<CachedPricingMap>,
) -> Arc<PricingMap> {
    if let Some(entry) = cached
        .iter()
        .find(|entry| entry.overrides == shared.pricing_overrides)
    {
        return Arc::clone(&entry.pricing);
    }

    let pricing = if shared.pricing_overrides.is_empty() {
        Arc::clone(base)
    } else {
        Arc::new(base.with_user_overrides(shared.pricing_overrides.iter()))
    };
    cached.push(CachedPricingMap {
        overrides: shared.pricing_overrides.clone(),
        pricing: Arc::clone(&pricing),
    });
    pricing
}

pub(super) fn load_pricing(sources: &[RunSource<'_>]) -> BTreeMap<&'static str, Arc<PricingMap>> {
    let needs_pricing = |source: &&RunSource<'_>| {
        source.source().uses_shared_pricing() && source.shared.mode != crate::cli::CostMode::Display
    };
    let modes = sources
        .iter()
        .filter(needs_pricing)
        .map(|source| source.shared.offline)
        .collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();
    for offline in modes {
        let base = Arc::new(PricingMap::load_with_overrides(
            offline,
            crate::log_level() != Some(0),
            std::iter::empty(),
        ));
        let mut cached = Vec::new();
        for source in sources
            .iter()
            .filter(needs_pricing)
            .filter(|source| source.shared.offline == offline)
        {
            result.insert(
                source.source().id(),
                cached_pricing_map(source.shared, &base, &mut cached),
            );
        }
    }
    result
}
