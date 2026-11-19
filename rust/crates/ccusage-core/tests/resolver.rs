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

#[test]
fn host_labels_covered_by_a_bare_override_bill_on_the_bare_schedule() {
    ccusage_test_support::install_pricing_snapshots();
    let base = PricingMap::load_embedded();
    let model = "deepseek-v4-flash";
    let with_override = base.with_user_overrides(&std::collections::BTreeMap::from([(
        model.to_string(),
        ccusage_cli::PricingOverride {
            cache_read_input_token_cost: Some(0.000_000_5),
            ..Default::default()
        },
    )]));
    let peak = ccusage_core::parse_ts_timestamp("2026-11-17T03:00:00Z");
    let off_peak = ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z");
    for label in [
        "aihubmix/deepseek-v4-flash",
        "openrouter/deepseek/deepseek-v4-flash",
    ] {
        let bare = with_override.find_handle(model).unwrap();
        let overridden = with_override.find_handle(label).unwrap();
        let static_price = base.find_handle(label).unwrap();
        assert!(overridden.is_time_dependent());
        assert!(!static_price.is_time_dependent());
        for timestamp in [peak, off_peak] {
            let rates = |handle: &ccusage_core::pricing::PricingHandle| {
                handle.with_rates_at(timestamp, |price| {
                    (
                        price.input,
                        price.output,
                        price.cache_read_rate_at(price.input),
                    )
                })
            };
            assert_eq!(rates(&overridden), rates(&bare), "{label}");
            assert_eq!(rates(&overridden).2, 0.000_000_5, "{label}");
            assert_eq!(rates(&static_price).0, 0.000_000_3, "{label}");
            assert_eq!(rates(&static_price).1, 0.000_001_2, "{label}");
        }
    }
}
