use std::alloc::System;
use std::hint::black_box;

use ccusage_core::PricingMap;
use ccusage_core::pricing::MatchKind;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn repeated_pricing_neither_allocates_nor_reallocates() {
    ccusage_test_support::install_pricing_snapshots();
    host_pass_does_not_allocate_to_compare_prefixes();
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

fn host_pass_does_not_allocate_to_compare_prefixes() {
    let mut pricing = PricingMap::default();
    let model = "Example-Host/sample";
    assert_eq!(
        pricing.load_json(
            r#"{"Example-Host/sample":{"input_cost_per_token":1,"output_cost_per_token":1}}"#,
        ),
        1,
    );
    let resolve = |reported, kind| {
        assert_eq!(
            pricing
                .resolve_at(reported, &[(model, kind)], None)
                .expect("the host comparison fixture has an exact pricing row")
                .1
                .input,
            1.0,
        );
    };
    resolve(None, MatchKind::Fuzzy);
    let region = Region::new(ALLOCATOR);
    resolve(Some("fuzzy"), MatchKind::Fuzzy);
    let without_host = region.change();
    let region = Region::new(ALLOCATOR);
    resolve(Some("exact"), MatchKind::Exact);
    let with_host = region.change();
    assert_eq!(with_host.allocations, without_host.allocations);
    assert_eq!(with_host.reallocations, without_host.reallocations);
}
