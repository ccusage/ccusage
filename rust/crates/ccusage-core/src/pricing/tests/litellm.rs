use super::{MatchKind, Pricing, PricingMap, build_time_pricing_json};

#[test]
fn each_pricing_loader_loads_only_its_own_document() {
    crate::test_support::install_pricing_snapshots();
    let litellm =
        r#"{"gpt-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
    let models_dev = r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;

    assert_eq!(PricingMap::default().load_json(litellm), 1);
    assert_eq!(PricingMap::default().load_json(models_dev), 0);
    assert_eq!(
        PricingMap::default().load_models_dev_json_missing(models_dev),
        Some(1)
    );
    assert_eq!(
        PricingMap::default().load_models_dev_json_missing(litellm),
        None
    );
}

#[test]
fn accepts_a_fetched_litellm_body_only_when_it_prices_a_model() {
    crate::test_support::install_pricing_snapshots();
    let litellm =
        r#"{"gpt-fetched":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
    let models_dev = r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;

    for (body, expected) in [(litellm, true), (models_dev, false), ("{}", false)] {
        let mut pricing = PricingMap::default();
        let mut accepted = None;
        pricing.refresh_litellm(|load| {
            accepted = Some(load(body));
            Ok(())
        });
        assert_eq!(accepted, Some(expected), "{body}");
        assert_eq!(
            pricing
                .resolve_at(None, &[("gpt-fetched", MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_some(),
            expected,
            "{body}"
        );
    }
}

#[test]
fn glm_cache_patch_keeps_an_explicitly_published_cache_write_rate() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "zai/glm-test-model": {
                    "input_cost_per_token": 0.2,
                    "output_cost_per_token": 1.1,
                    "cache_read_input_token_cost": 0.03,
                    "cache_creation_input_token_cost": 0.25
                }
            }"#,
    );
    let zeroed = Pricing {
        cache_create: Some(0.0),
        cache_read: Some(0.03),
        ..Pricing::empty()
    };

    pricing.supplement_pricing("zai/glm-test-model", zeroed, None, "");

    let entry = pricing
        .resolve_at(None, &[("zai/glm-test-model", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(entry.cache_create, Some(0.25));
    assert_eq!(entry.cache_read, Some(0.03));
}

#[test]
fn embedded_pricing_patches_z_ai_glm_entries_without_litellm_cache_rates() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let glm_45_air = pricing.find("zai/glm-4.5-air").unwrap();
    assert_eq!(glm_45_air.input, 0.2e-6);
    assert_eq!(glm_45_air.output, 1.1e-6);
    assert_eq!(glm_45_air.cache_create, Some(0.0));
    assert_eq!(glm_45_air.cache_read, Some(0.03e-6));

    let glm_45_x = pricing.find("zai/glm-4.5-x").unwrap();
    assert_eq!(glm_45_x.input, 2.2e-6);
    assert_eq!(glm_45_x.output, 8.9e-6);
    assert_eq!(glm_45_x.cache_create, Some(0.0));
    assert_eq!(glm_45_x.cache_read, Some(0.45e-6));

    let glm_45v = pricing.find("zai/glm-4.5v").unwrap();
    assert_eq!(glm_45v.input, 0.6e-6);
    assert_eq!(glm_45v.output, 1.8e-6);
    assert_eq!(glm_45v.cache_create, Some(0.0));
    assert_eq!(glm_45v.cache_read, Some(0.11e-6));
}

#[test]
fn keeps_absent_litellm_cache_rates_absent() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-with-cache": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                },
                "gpt-without-cache": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
    );

    assert!(pricing.find("gpt-with-cache").unwrap().cache_read.is_some());
    assert!(
        !pricing
            .find("gpt-without-cache")
            .unwrap()
            .cache_read
            .is_some()
    );
}

#[test]
fn skips_invalid_litellm_entries_without_discarding_valid_pricing() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let loaded = pricing.load_json(
        r#"{
                "sample_spec": {
                    "max_input_tokens": "max input tokens, if the provider specifies it"
                },
                "gpt-valid": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "max_input_tokens": 123
                }
            }"#,
    );

    assert_eq!(loaded, 1);
    assert!(pricing.find("gpt-valid").is_some());
    assert_eq!(pricing.context_limit("gpt-valid"), Some(123));
}

#[test]
fn loads_every_litellm_rate_field() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let loaded = pricing.load_json(
        r#"{
                "gpt-every-rate": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_creation_input_token_cost": 0.00000125,
                    "cache_read_input_token_cost": 0.0000001,
                    "input_cost_per_token_above_200k_tokens": 0.000002,
                    "output_cost_per_token_above_200k_tokens": 0.000020,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.0000025,
                    "cache_read_input_token_cost_above_200k_tokens": 0.0000002,
                    "max_input_tokens": 123456,
                    "provider_specific_entry": { "fast": 1.5 }
                }
            }"#,
    );

    assert_eq!(loaded, 1);
    let entry = pricing.find("gpt-every-rate").unwrap();
    assert_eq!(entry.input, 1e-6);
    assert_eq!(entry.output, 10e-6);
    assert_eq!(entry.cache_create, Some(1.25e-6));
    assert_eq!(entry.cache_read, Some(0.1e-6));
    assert_eq!(
        entry.long_context.and_then(|context| context.rates.input),
        Some(2e-6)
    );
    assert_eq!(
        entry.long_context.and_then(|context| context.rates.output),
        Some(20e-6)
    );
    assert_eq!(
        entry
            .long_context
            .and_then(|context| context.rates.cache_create),
        Some(2.5e-6)
    );
    assert_eq!(
        entry
            .long_context
            .and_then(|context| context.rates.cache_read),
        Some(0.2e-6)
    );
    assert_eq!(entry.fast_multiplier, 1.5);
    assert_eq!(pricing.context_limit("gpt-every-rate"), Some(123456));
}

#[test]
fn embedded_build_time_pricing_is_filtered() {
    crate::test_support::install_pricing_snapshots();
    let json = build_time_pricing_json();
    assert!(json.len() < 200_000);
    assert!(!json.contains("\"source\""));
    assert!(!json.contains("vertex_ai/"));
    assert!(json.contains("claude-opus-4-6"));
}

#[test]
fn google_zero_token_rate_rows_are_left_unpriced_whether_free_tier_or_priced_per_request() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gemini/gemma-3-27b-it": {"litellm_provider": "gemini", "input_cost_per_token": 0, "output_cost_per_token": 0},
            "gemini/lyria-3.5": {"litellm_provider": "gemini", "input_cost_per_token": 0, "output_cost_per_token": 0, "output_cost_per_image": 0.08},
            "gemini/gemini-2.5-flash": {"litellm_provider": "gemini", "input_cost_per_token": 3e-7, "output_cost_per_token": 2.5e-6}
        }"#,
    );

    assert!(pricing.find("gemini/gemma-3-27b-it").is_none());
    assert!(pricing.find("gemini/lyria-3.5").is_none());
    assert_eq!(pricing.find("gemini/gemini-2.5-flash").unwrap().input, 3e-7);
}
