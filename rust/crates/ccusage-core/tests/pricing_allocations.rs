use std::alloc::System;
use std::hint::black_box;

use ccusage_core::PricingMap;
use ccusage_core::pricing::MatchKind;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn repeated_pricing_neither_allocates_nor_reallocates() {
    for multiple_candidates in [false, true] {
        let mut pricing = PricingMap::default();
        let rows: serde_json::Map<String, serde_json::Value> = (0..20)
            .map(|id| {
                (
                    format!("catalogue/model{id}-1.2"),
                    serde_json::json!({
                        "input_cost_per_token": 3,
                        "output_cost_per_token": 4
                    }),
                )
            })
            .collect();
        assert_eq!(
            pricing.load_json(&serde_json::Value::Object(rows).to_string()),
            20
        );
        let names: Vec<_> = (0..20)
            .map(|id| {
                (
                    format!("model{id}-1-2"),
                    format!("provider/missing-{id}"),
                    format!("missing-{id}"),
                )
            })
            .collect();
        let requests: Vec<_> = names
            .iter()
            .map(|(name, provider, missing)| {
                let mut candidates = Vec::with_capacity(3);
                if multiple_candidates {
                    candidates.push((provider.as_str(), MatchKind::StrictExact));
                    candidates.push((missing.as_str(), MatchKind::Fuzzy));
                }
                candidates.push((name.as_str(), MatchKind::Fuzzy));
                (name.as_str(), candidates)
            })
            .collect();
        let lookup = |(name, candidates): &(&str, Vec<(&str, MatchKind)>)| {
            let (_, rates) = pricing.resolve_at(Some(name), candidates, None).unwrap();
            assert_eq!((rates.input, rates.output), (3.0, 4.0));
        };
        requests.iter().for_each(lookup);

        let region = Region::new(ALLOCATOR);
        for request in requests.iter().cycle().take(9_980) {
            lookup(black_box(request));
        }
        let stats = region.change();
        assert_eq!(
            (stats.allocations, stats.reallocations),
            (0, 0),
            "multiple candidates: {multiple_candidates}"
        );
    }
}
