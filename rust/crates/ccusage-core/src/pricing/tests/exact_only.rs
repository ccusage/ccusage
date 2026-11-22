use super::{
    AtomicUsize, MatchKind, Ordering, Pricing, PricingMap, embedded_models_dev_pricing, layered,
};

fn exact_only_fast_fallback_fixture() -> (PricingMap, PricingMap, PricingMap) {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    assert_eq!(
        primary.load_json(
            r#"{
                    "claude-opus-5": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    }
                }"#,
        ),
        1
    );

    let mut models_dev = PricingMap::default();
    assert_eq!(
        models_dev.load_models_dev_json_for_tests(
            r#"{
                    "claude-opus-5": {
                        "cost": { "input": 3, "output": 4 },
                        "limit": { "context": 234567 }
                    },
                    "claude-opus-5-fast": {
                        "cost": { "input": 9, "output": 17 },
                        "limit": { "context": 765432 },
                        "exactOnly": true
                    }
                }"#,
        ),
        Some(2)
    );

    let mut embedded_models_dev = PricingMap::default();
    assert_eq!(
        embedded_models_dev.load_models_dev_json_for_tests(
            r#"{
                    "claude-opus-5-fast": {
                        "cost": { "input": 5, "output": 10 },
                        "limit": { "context": 765432 },
                        "exactOnly": true
                    }
                }"#,
        ),
        Some(1)
    );
    (primary, models_dev, embedded_models_dev)
}

#[test]
fn exact_only_fallback_beats_a_fuzzy_primary_match() {
    crate::test_support::install_pricing_snapshots();
    let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
    let pricing = layered(&primary, Some(&embedded_models_dev));

    let base = pricing.find("claude-opus-5").unwrap();
    let fast = pricing
        .find_with_live("claude-opus-5-fast", || Some(&models_dev))
        .expect("the exact-only fallback prices the Fast tier");
    assert_eq!(fast.input, 9e-6);
    assert_eq!(fast.output, 17e-6);
    assert_ne!(fast.input, base.input);
    assert_eq!(
        pricing.context_limit_with_live("claude-opus-5-fast", || Some(&models_dev)),
        Some(765_432)
    );
}

#[test]
fn live_exact_only_fallback_beats_a_fuzzy_primary_match() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    assert_eq!(
        primary.load_json(
            r#"{
                    "kimi-k2.7-code": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    }
                }"#,
        ),
        1
    );

    let mut models_dev = PricingMap::default();
    assert_eq!(
        models_dev.load_models_dev_json_missing(
            r#"{
                    "moonshotai": {
                        "models": {
                            "kimi-k2.7-code-highspeed": {
                                "cost": { "input": 9, "output": 17 },
                                "limit": { "context": 765432 }
                            }
                        }
                    }
                }"#,
        ),
        Some(1)
    );
    assert!(
        models_dev
            .rows
            .entries
            .get("kimi-k2.7-code-highspeed")
            .is_some_and(|row| row.exact_only)
    );

    let pricing = layered(&primary, None);
    let tier = pricing
        .find_with_live("kimi-k2.7-code-highspeed", || Some(&models_dev))
        .expect("the live fallback prices the Highspeed tier");
    assert_eq!(tier.input, 9e-6);
    assert_eq!(tier.output, 17e-6);
    assert_eq!(
        pricing.context_limit_with_live("kimi-k2.7-code-highspeed", || Some(&models_dev)),
        Some(765_432)
    );

    let pricing = layered(&primary, None);
    let live_lookups = AtomicUsize::new(0);
    let live = || {
        live_lookups.fetch_add(1, Ordering::Relaxed);
        Some(&models_dev)
    };
    let base = pricing
        .find_with_live("kimi-k2.7-code", live)
        .expect("the primary map prices the base model exactly");
    assert_eq!(base.input, 1e-6);
    assert_eq!(
        pricing.context_limit_with_live("kimi-k2.7-code", live),
        Some(123_456)
    );
    assert_eq!(live_lookups.load(Ordering::Relaxed), 0);
}

#[test]
fn embedded_pricing_carries_exact_only_catalog_entries() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let embedded = embedded_models_dev_pricing();

    for model in ["claude-opus-5-fast", "claude-opus-5@eu"] {
        assert!(
            embedded.rows.get(model).is_some_and(|row| row.exact_only),
            "{model}"
        );
        assert!(pricing.find(model).is_some(), "{model}");
        assert!(pricing.context_limit(model).is_some(), "{model}");
    }

    let expected_fast = embedded
        .resolve_at(None, &[("claude-opus-5-fast", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let actual_fast = pricing.find("claude-opus-5-fast").unwrap();
    assert_eq!(actual_fast.input, expected_fast.input);
    assert_eq!(actual_fast.output, expected_fast.output);
    assert_eq!(
        pricing.context_limit("claude-opus-5-fast"),
        embedded.context_limit("claude-opus-5-fast")
    );

    assert!(pricing.find("claude-opus-5-20260115").is_some());
}

#[test]
fn a_separator_spelling_of_an_exact_only_id_is_priced_as_that_id() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let base = pricing.find("claude-opus-5").unwrap();
    let eu = pricing.find("claude-opus-5@eu").unwrap();
    let dashed = pricing
        .find("claude-opus-5-eu")
        .expect("the dashed spelling names the EU alias");
    assert!(dashed.input > base.input);
    assert!((dashed.input - eu.input).abs() < 1e-12);
    assert!((dashed.output - eu.output).abs() < 1e-12);
    assert_eq!(
        pricing.context_limit("claude-opus-5-eu"),
        pricing.context_limit("claude-opus-5@eu")
    );

    assert!(pricing.find("claude-opus-5-eu-preview").is_none());
}

#[test]
fn a_dotted_spelling_of_a_dash_spelled_exact_only_id_is_priced_as_that_id() {
    crate::test_support::install_pricing_snapshots();
    let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
    let pricing = layered(&primary, Some(&embedded_models_dev));

    let base = pricing.find("claude-opus-5").unwrap();
    let dotted = pricing
        .find_with_live("claude-opus-5.fast", || Some(&models_dev))
        .expect("the dotted spelling names the Fast tier");
    assert_eq!(dotted.input, 9e-6);
    assert_eq!(dotted.output, 17e-6);
    assert!(dotted.input > base.input);
    assert_eq!(
        pricing.context_limit_with_live("claude-opus-5.fast", || Some(&models_dev)),
        Some(765_432)
    );
}

#[test]
fn exact_fallback_lookup_resolves_separator_spellings_of_exact_only_ids() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let expected = pricing
        .resolve_at(None, &[("claude-opus-5@eu", MatchKind::Exact)], None)
        .map(|(_, pricing)| pricing)
        .expect("the embedded snapshot should contain the exact-only regional id");

    for spelling in ["claude-opus-5-eu", "claude-opus-5.eu"] {
        let resolved = pricing
            .resolve_at(None, &[(spelling, MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .expect("separator-equivalent exact-only ids should resolve");
        assert_eq!(resolved.input, expected.input, "{spelling}");
        assert_eq!(resolved.output, expected.output, "{spelling}");
    }
    assert!(
        pricing
            .resolve_at(
                None,
                &[("claude-opus-5-eu-preview", MatchKind::Exact)],
                None
            )
            .map(|(_, pricing)| pricing)
            .is_none()
    );
}

#[test]
fn exact_fallback_lookup_prefers_primary_normalized_entries() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    pricing.load_json(
        r#"{
                "claude-opus-5.eu": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010
                }
            }"#,
    );

    let resolved = pricing
        .resolve_at(None, &[("claude-opus-5-eu", MatchKind::Exact)], None)
        .map(|(_, pricing)| pricing)
        .expect("the normalized primary entry should resolve");
    assert_eq!(resolved.input, 0.000009);
    assert_eq!(resolved.output, 0.000010);
}

#[test]
fn exact_fallback_prefers_literal_keys_then_the_smallest_spelling() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    assert_eq!(pricing.load_json(r#"{
            "acme-alpha-1.5": {"litellm_provider":"openai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "acme.alpha@1.5": {"litellm_provider":"openai","input_cost_per_token":2e-6,"output_cost_per_token":2e-6}
        }"#), 2);
    for (request, key, rate) in [
        ("acme-alpha-1.5", "acme-alpha-1.5", 1e-6),
        ("acme.alpha@1.5", "acme.alpha@1.5", 2e-6),
        ("acme-alpha-1-5", "acme-alpha-1.5", 1e-6),
    ] {
        let candidates = [(request, MatchKind::Exact)];
        let found = pricing.explain(None, &candidates).unwrap();
        assert_eq!(found.row_key, key, "{request}");
        let (_, price) = pricing.resolve_at(None, &candidates, None).unwrap();
        assert_eq!((price.input, price.output), (rate, rate), "{request}");
    }
}

#[test]
fn exact_fallback_lookup_sees_a_colliding_entry_added_after_a_lookup() {
    let mut pricing = PricingMap::default();
    pricing.supplement_pricing(
        "acme-alpha-1.5",
        Pricing {
            input: 1e-6,
            ..Pricing::empty()
        },
        None,
        "",
    );
    let resolved = pricing
        .resolve_at(None, &[("acme-alpha-1-5", MatchKind::Exact)], None)
        .map(|(_, pricing)| pricing)
        .expect("a single normalized match should resolve");
    assert_eq!(resolved.input, 1e-6);

    pricing.supplement_pricing(
        "acme.alpha@1.5",
        Pricing {
            input: 2e-6,
            ..Pricing::empty()
        },
        None,
        "",
    );

    assert_eq!(
        pricing
            .resolve_at(None, &[("acme-alpha-1-5", MatchKind::Exact)], None)
            .unwrap()
            .1
            .input,
        1e-6
    );
    assert_eq!(
        pricing
            .resolve_at(None, &[("acme.alpha@1.5", MatchKind::Exact)], None)
            .unwrap()
            .1
            .input,
        2e-6
    );
}

#[test]
fn exact_fallback_lookup_sees_rates_patched_after_a_lookup() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "zai/glm-test.5": {
                    "input_cost_per_token": 0.2,
                    "output_cost_per_token": 1.1
                }
            }"#,
    );
    let before = pricing
        .resolve_at(None, &[("zai/glm-test-5", MatchKind::Exact)], None)
        .map(|(_, pricing)| pricing)
        .expect("the normalized entry should resolve");
    assert!(before.cache_read.is_none());

    pricing.supplement_pricing(
        "zai/glm-test.5",
        Pricing {
            cache_read: Some(0.03),
            ..Pricing::empty()
        },
        None,
        "",
    );

    let after = pricing
        .resolve_at(None, &[("zai/glm-test-5", MatchKind::Exact)], None)
        .map(|(_, pricing)| pricing)
        .unwrap();
    assert_eq!(after.cache_read, Some(0.03));
}

#[test]
fn configured_alias_of_an_exact_only_tier_beats_a_fuzzy_match_on_the_alias() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "claude-opus-5-turbo",
        "claude-opus-5-fast",
    )]);
    let (primary, models_dev, embedded_models_dev) = exact_only_fast_fallback_fixture();
    let pricing = layered(&primary, Some(&embedded_models_dev));

    let turbo = pricing
        .find_with_live("claude-opus-5-turbo", || Some(&models_dev))
        .expect("the alias resolves to the Fast tier");
    assert_eq!(turbo.input, 9e-6);
    assert_eq!(turbo.output, 17e-6);
    assert_eq!(
        pricing.context_limit_with_live("claude-opus-5-turbo", || Some(&models_dev)),
        Some(765_432)
    );

    let mut builtin = PricingMap::default();
    builtin.insert_test_limit("claude-opus-5", 123_456);
    let mut exact_only_without_context = PricingMap::default();
    assert_eq!(
        exact_only_without_context.load_models_dev_json_for_tests(
            r#"{
                    "claude-opus-5-fast": {
                        "cost": { "input": 9, "output": 17 },
                        "exactOnly": true
                    }
                }"#,
        ),
        Some(1)
    );
    assert_eq!(
        layered(&builtin, Some(&exact_only_without_context))
            .context_limit_with_live("claude-opus-5-fast", || None),
        None,
        "an exact-only id without a published limit must not inherit a fuzzy builtin limit"
    );
}

#[test]
fn a_spelling_that_names_an_exact_only_tier_is_billed_at_that_tier() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let tier = pricing.find("gemini-3.5-flash-lite").unwrap();
    assert_eq!((tier.input, tier.output), (0.3e-6, 2.5e-6));
    for model in [
        "github_copilot/gemini-3.5-flash-lite",
        "models/gemini-3.5-flash-lite",
        "gemini-3.5-flash-lite[1m]",
        "gemini-3.5-flash-lite-high",
        "gemini-3.5-flash-lite-20261101",
    ] {
        assert_eq!(pricing.find(model), Some(tier), "{model}");
    }
}
