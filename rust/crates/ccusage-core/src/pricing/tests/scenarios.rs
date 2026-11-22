use ccusage_cli::PricingOverride;

use super::super::{BillingRoute, MatchKind, PricingMap, ResolutionStep};
use super::timestamp;
use crate::TokenUsageRaw;
use crate::cost::{EntryPricing, PriceTable, UsageGranularity, price_entry, price_entry_on_route};

#[test]
fn resolver_scenarios_on_the_embedded_tables() {
    crate::test_support::install_pricing_snapshots();
    check_cases(
        &PricingMap::load_embedded(),
        include_str!("embedded-cases.tsv"),
    );
}

#[test]
fn resolver_scenarios_with_a_live_document() {
    crate::test_support::install_pricing_snapshots();
    let mut live = PricingMap::load_embedded();
    live.refresh_litellm(|load| {
        assert!(load(include_str!("live-pricing.json")));
        Ok(())
    });
    check_cases(&live, include_str!("live-cases.tsv"));
}

fn check_cases(base: &PricingMap, cases: &str) {
    for line in cases.lines().filter(|line| !line.starts_with('#')) {
        let fields = line.split('|').collect::<Vec<_>>();
        if !(7..=8).contains(&fields.len()) {
            panic!("invalid resolver case {line}");
        }
        let (case, label, candidates, expected, step, key, overrides) = (
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
        );
        let route = match fields.get(7).copied().unwrap_or("") {
            "" => None,
            "direct" => Some(BillingRoute::Direct("deepseek")),
            "foreign" => Some(BillingRoute::Foreign),
            route => panic!("{line}: unknown billing route {route}"),
        };
        let name = format!("{case} {label}");
        let candidates = candidates
            .split(',')
            .map(|entry| {
                let (kind, model) = entry.split_once(':').unwrap();
                let kind = match kind {
                    "S" => MatchKind::StrictExact,
                    "E" => MatchKind::Exact,
                    "F" => MatchKind::Fuzzy,
                    "B" => MatchKind::Fallback,
                    _ => panic!("{name}: unknown candidate kind {kind}"),
                };
                (model, kind)
            })
            .collect::<Vec<_>>();
        let step = match step {
            "S1" => ResolutionStep::ReportedOverride,
            "S2" => ResolutionStep::AuthoritativeIdentity,
            "S3" => ResolutionStep::AuthoritativeSuffix,
            "S4" => ResolutionStep::ModelsDevIdentity,
            "S5" => ResolutionStep::AuthoritativeFuzzy,
            "S6" => ResolutionStep::ModelsDevFuzzy,
            _ => panic!("{name}: unknown resolution step {step}"),
        };
        let overrides = overrides
            .split(',')
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let (key, rates) = entry.split_once(':').unwrap();
                let (input, output) = rates.split_once('/').unwrap();
                let rate =
                    |value: &str| (!value.is_empty()).then(|| value.parse::<f64>().unwrap() / 1e6);
                (
                    key.to_owned(),
                    PricingOverride {
                        input_cost_per_token: rate(input),
                        output_cost_per_token: rate(output),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let overridden = (!overrides.is_empty())
            .then(|| base.with_user_overrides(overrides.iter().map(|(key, rates)| (key, rates))));
        let map = overridden.as_ref().unwrap_or(base);
        let found = map
            .explain(Some(label), &candidates)
            .unwrap_or_else(|| panic!("{name}: unpriced"));
        assert_eq!((found.step, found.row_key), (step, key), "{name}");
        let (expected, limit) = expected
            .split_once('+')
            .map_or((expected, None), |(rates, limit)| {
                (rates, Some(limit.parse().unwrap()))
            });
        if let Some(limit) = limit {
            assert_eq!(map.context_limit(label), Some(limit), "{name}");
        }
        let instants: &[_] = if case.starts_with("087-") || case.starts_with("088-") {
            &["2026-11-17T10:00:00Z", "2026-11-17T03:00:00Z"]
        } else {
            &["2026-11-17T10:00:00Z"]
        };
        for instant in instants {
            let timestamp = Some(timestamp(instant));
            if let Some((shape, cost)) = expected.split_once(':') {
                let (
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                ) = match shape {
                    "L" => (300_000, 1_000, 0, 0),
                    "a" => (1_000, 1_000, 1_000, 1_000),
                    "o" => (0, 1_000, 0, 0),
                    "w" => (0, 0, 0, 1_000),
                    _ => panic!("{name}: unknown token shape {shape}"),
                };
                let entry_pricing = EntryPricing {
                    granularity: UsageGranularity::Request,
                    usage: TokenUsageRaw {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                        ..Default::default()
                    },
                    reasoning_tokens: 0,
                    reported_model: Some(label),
                    candidates: &candidates,
                    stored_cost: None,
                    timestamp,
                };
                let entry = match route {
                    Some(route) => {
                        price_entry_on_route(entry_pricing, route, PriceTable::Calculate(map))
                    }
                    None => price_entry(entry_pricing, PriceTable::Calculate(map)),
                };
                assert!(!entry.missing_pricing, "{name}");
                assert_close(entry.cost, cost, &name);
            } else {
                let (_, rates) = match route {
                    Some(route) => {
                        map.resolve_on_route_at(Some(label), &candidates, route, timestamp)
                    }
                    None => map.resolve_at(Some(label), &candidates, timestamp),
                }
                .unwrap();
                let mut expected = expected.split('/');
                assert_close(rates.input * 1e6, expected.next().unwrap(), &name);
                assert_close(rates.output * 1e6, expected.next().unwrap(), &name);
                if let Some(read) = expected.next() {
                    assert_close(rates.cache_read.unwrap() * 1e6, read, &name);
                }
            }
        }
    }
}

fn assert_close(actual: f64, expected: &str, name: &str) {
    assert!(
        (actual - expected.parse::<f64>().unwrap()).abs() < 1e-10,
        "{name}: expected {expected}, got {actual}"
    );
}
