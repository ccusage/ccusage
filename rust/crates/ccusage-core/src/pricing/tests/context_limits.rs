use super::{Lookup, MatchKind, PricingMap, layered};

#[test]
fn reads_embedded_model_context_limits() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert_eq!(
        pricing.context_limit_entry(
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
            MatchKind::Exact
        ),
        Some(200_000)
    );
}

#[test]
fn context_limit_prefers_the_paid_host_listing_over_the_free_vendor_row() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, limit) in [
        ("@cf/zai-org/glm-4.7-flash", 131_072),
        ("cloudflare/@cf/zai-org/glm-4.7-flash", 131_072),
        ("z-ai/glm-4.7-flash", 202_752),
    ] {
        let rates = pricing.find(model).unwrap();
        assert!(rates.input > 0.0 && rates.output > 0.0, "{model}");
        assert_eq!(pricing.context_limit(model), Some(limit), "{model}");
    }
}

#[test]
fn aliased_tier_id_does_not_take_the_base_models_limit() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "claude-opus-5-turbo",
        "claude-opus-5-fast",
    )]);
    let mut secondary = PricingMap::default();
    assert_eq!(
        secondary.load_models_dev_json_for_tests(
            r#"{
            "claude-opus-5":{"cost":{"input":3,"output":4},"limit":{"context":1000000}},
            "claude-opus-5-fast":{"cost":{"input":6,"output":8},"exactOnly":true}
        }"#
        ),
        Some(2)
    );
    let pricing = layered(&PricingMap::default(), Some(&secondary));
    assert_eq!(
        pricing.context_limit_entry("claude-opus-5", MatchKind::Fuzzy),
        Some(1_000_000)
    );
    assert_eq!(
        pricing.context_limit_entry("claude-opus-5-turbo", MatchKind::Fuzzy),
        None
    );
    assert_eq!(
        pricing
            .find_with_live("claude-opus-5-turbo", || None)
            .unwrap()
            .input,
        6e-6
    );
}

#[test]
fn a_router_path_to_an_exact_only_tier_does_not_take_the_base_models_limit() {
    crate::test_support::install_pricing_snapshots();
    let mut secondary = PricingMap::default();
    assert_eq!(
        secondary.load_models_dev_json_for_tests(
            r#"{
            "sample-1":{"cost":{"input":3,"output":4},"limit":{"context":123456}},
            "sample-1-fast":{"cost":{"input":6,"output":8},"exactOnly":true},
            "sample-2":{"cost":{"input":3,"output":4},"limit":{"context":123456}},
            "sample-2-fast":{"cost":{"input":6,"output":8},"exactOnly":true,"limit":{"context":654321}}
        }"#
        ),
        Some(4)
    );
    let pricing = layered(&PricingMap::default(), Some(&secondary));
    assert_eq!(
        pricing
            .find_with_live("router/sample-1-fast", || None)
            .unwrap()
            .input,
        6e-6
    );
    assert_eq!(
        pricing.context_limit_entry("router/sample-1-fast", MatchKind::Fuzzy),
        None
    );
    assert_eq!(
        pricing.context_limit_entry("router/sample-2-fast", MatchKind::Fuzzy),
        Some(654_321)
    );
}

#[test]
fn context_limits_keep_their_pre_wave_exact_only_boundaries() {
    crate::test_support::install_pricing_snapshots();
    let map = PricingMap::load_embedded();
    assert_eq!(map.context_limit("qwen_ai_platform/glm-5.1"), Some(200_000));
    assert_eq!(
        map.context_limit("novita/zai-org/glm-5-turbo"),
        Some(200_000)
    );
}

#[test]
fn live_exact_only_rows_without_limits_keep_embedded_tiers_out_of_fuzzy_limits() {
    crate::test_support::install_pricing_snapshots();
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"sample-fast":{"exactOnly":true,"cost":{"input":1,"output":2},"limit":{"context":123456}}}"#,
    );
    let pricing = layered(&PricingMap::default(), Some(&embedded));
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"sample-fast":{"exactOnly":true,"cost":{"input":3,"output":4}}}"#,
    );
    let pricing = pricing.with_live_layer("sample-fast-extra", Lookup::Context, || Some(&live));
    assert!(pricing.find("sample-fast-extra").is_none());
    assert_eq!(pricing.context_limit("sample-fast-extra"), None);
    assert_eq!(pricing.context_limit("sample-fast"), None);
}
