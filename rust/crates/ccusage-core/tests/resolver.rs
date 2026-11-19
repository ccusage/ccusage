use ccusage_core::pricing::{Explanation, MatchKind, PricingMap, ResolutionStep, Source};

#[test]
fn explanation_identifies_the_row_used_to_price_the_request() {
    ccusage_test_support::install_pricing_snapshots();
    let mut table = PricingMap::default();
    table.load_json(
        r#"{
        "host/beam-1.2": {"input_cost_per_token": 1, "output_cost_per_token": 3},
        "host/beam-1.2-extra": {"input_cost_per_token": 8, "output_cost_per_token": 9}
    }"#,
    );
    let candidates = [
        ("beam-1-2", MatchKind::Fuzzy),
        ("host/beam-1-2", MatchKind::Exact),
    ];
    let label = Some("beam-1-2");
    assert_eq!(
        table.explain(label, &candidates),
        Some(Explanation {
            candidate: "host/beam-1-2",
            step: ResolutionStep::AuthoritativeIdentity,
            row_key: "host/beam-1.2",
            source: Source::LiteLlm,
        }),
    );
    let (candidate, price) = table.resolve_at(label, &candidates, None).unwrap();
    assert_eq!(candidate, "host/beam-1-2");
    assert_eq!((price.input, price.output), (1.0, 3.0));
}
