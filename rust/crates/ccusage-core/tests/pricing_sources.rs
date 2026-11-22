use ccusage_core::PricingMap;

#[test]
fn load_json_changes_nothing_without_source_rules() {
    let mut table = PricingMap::default();
    let json = r#"{
        "entry-1":{"input_cost_per_token":1,"output_cost_per_token":2},
        "entry-2":{"litellm_provider":"provider","input_cost_per_token":3,"output_cost_per_token":4}
    }"#;
    assert_eq!(table.load_json(json), 0);
    assert!(table.find("entry-1").is_none());
    assert!(table.find("entry-2").is_none());

    ccusage_test_support::install_pricing_snapshots();
    assert_eq!(table.load_json(json), 2);
    assert_eq!(table.find("entry-1").unwrap().input, 1.0);
    assert_eq!(table.find("entry-2").unwrap().input, 3.0);
}
