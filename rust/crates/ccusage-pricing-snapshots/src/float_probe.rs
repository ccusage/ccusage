use ccusage_core::{
    PricingMap,
    pricing::{EmbeddedSnapshots, MatchKind, set_embedded_snapshots},
};

include!(concat!(env!("OUT_DIR"), "/float-probe.rs"));

#[test]
fn runtime_loads_rates_with_the_float_parsing_that_built_the_pricing_table() {
    set_embedded_snapshots(EmbeddedSnapshots {
        litellm_sources: crate::LITELLM_SOURCES,
        models_dev_catalog_rules: crate::MODELS_DEV_CATALOG_RULES,
        pricing_table: crate::PRICING_TABLE,
        models_dev_table: crate::MODELS_DEV_TABLE,
    });
    let json = format!(
        r#"{{"gpt-test":{{"input_cost_per_token":{FLOAT_PROBE},"output_cost_per_token":0.000002}}}}"#
    );
    let mut pricing = PricingMap::default();
    pricing.load_json(&json);

    let input = pricing
        .resolve_at(None, &[("gpt-test", MatchKind::StrictExact)], None)
        .unwrap()
        .1
        .input;

    assert_eq!(input.to_bits(), FLOAT_PROBE_BITS);
}
