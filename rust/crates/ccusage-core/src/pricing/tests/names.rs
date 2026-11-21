use super::{AtomicUsize, MatchKind, Ordering, PricingMap, model_without_date_suffix, online};

#[test]
fn builtin_alias_applies_to_a_separator_respelling() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (canonical, respelling) in [
        ("gpt-5.3-spark", "gpt-5-3-spark"),
        ("gpt-5.6", "gpt-5-6"),
        ("gpt-reserve", "gpt@reserve"),
    ] {
        let expected = pricing.find(canonical).unwrap();
        let actual = pricing.find(respelling).unwrap();
        assert_eq!(
            (actual.input, actual.output),
            (expected.input, expected.output),
            "{respelling}"
        );
        assert_eq!(
            pricing.context_limit(respelling),
            pricing.context_limit(canonical),
            "{respelling}"
        );
    }
    let spark = pricing.find("gpt-5-3-spark").unwrap();
    assert_eq!((spark.input, spark.output), (1.75e-6, 14e-6));
    assert_eq!(pricing.context_limit("gpt-5-3-spark"), Some(128_000));
}

#[test]
fn strips_model_date_suffixes() {
    assert_eq!(model_without_date_suffix("gpt-5.5-2026-04-23"), "gpt-5.5");
    assert_eq!(
        model_without_date_suffix("gpt-5.5-pro-2026-04-23"),
        "gpt-5.5-pro"
    );
    assert_eq!(
        model_without_date_suffix("claude-3-5-haiku-20241022"),
        "claude-3-5-haiku"
    );
    assert_eq!(model_without_date_suffix("gpt-5.6-sol"), "gpt-5.6-sol");
    assert_eq!(model_without_date_suffix("gpt-4-0613"), "gpt-4-0613");
}

#[test]
fn pricing_lookup_resolves_model_aliases() {
    crate::test_support::install_pricing_snapshots();
    let _aliases =
        crate::model_aliases::set_model_aliases_for_tests([("private-gpt-55", "gpt-5.5")]);
    let pricing = PricingMap::load_embedded();

    assert_eq!(
        pricing.find("private-gpt-55").unwrap().input,
        pricing.find("gpt-5.5").unwrap().input
    );
    assert_eq!(pricing.context_limit("private-gpt-55"), Some(1_050_000));
}

#[test]
fn pricing_lookup_prefers_known_original_model_before_alias() {
    crate::test_support::install_pricing_snapshots();
    let _aliases =
        crate::model_aliases::set_model_aliases_for_tests([("claude-opus-4-8", "mythos-5")]);
    let pricing = PricingMap::load_embedded();

    let original = pricing
        .find_entry("claude-opus-4-8", MatchKind::Fuzzy)
        .unwrap();
    let resolved = pricing.find("claude-opus-4-8").unwrap();

    assert_eq!(resolved.input, original.input);
    assert_eq!(
        pricing.context_limit("claude-opus-4-8"),
        pricing.context_limit_entry("claude-opus-4-8", MatchKind::Fuzzy)
    );
}

#[test]
fn pricing_lookup_prefers_an_exact_alias_target_over_a_fuzzy_original_match() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "claude-opus-5-turbo",
        "sentinel-exact-target",
    )]);
    let mut pricing = PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "claude-opus-5": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "max_input_tokens": 123456
                    },
                    "sentinel-exact-target": {
                        "input_cost_per_token": 0.000009,
                        "output_cost_per_token": 0.000017,
                        "max_input_tokens": 765432
                    }
                }"#,
        ),
        2
    );

    let pricing = online(&pricing);
    let live_lookups = AtomicUsize::new(0);
    let live = || {
        live_lookups.fetch_add(1, Ordering::Relaxed);
        None
    };
    let resolved = pricing.find_with_live("claude-opus-5-turbo", live).unwrap();
    assert_eq!(resolved.input, 9e-6);
    assert_eq!(resolved.output, 17e-6);
    assert_eq!(
        pricing.context_limit_with_live("claude-opus-5-turbo", live),
        Some(765_432)
    );
    assert_eq!(live_lookups.load(Ordering::Relaxed), 0);
}

#[test]
fn pricing_lookup_preserves_a_live_exact_only_builtin_alias_target() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "gpt-5.6": {
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
        models_dev.load_json(
            r#"{
                    "gpt-5.6-luna": {
                        "input_cost_per_token": 0.000009,
                        "output_cost_per_token": 0.000017,
                        "max_input_tokens": 765432
                    }
                }"#,
        ),
        1
    );
    models_dev.mark_test_exact_only("gpt-5.6-luna");

    let pricing = online(&pricing);
    let resolved = pricing
        .find_with_live("gpt-reserve", || Some(&models_dev))
        .unwrap();
    assert_eq!(resolved.input, 9e-6);
    assert_eq!(resolved.output, 17e-6);
    assert_eq!(
        pricing.context_limit_with_live("gpt-reserve", || Some(&models_dev)),
        Some(765_432)
    );
}

#[test]
fn model_suffix_matches_only_at_the_last_occurrence() {
    assert!(super::super::matches_model_suffix(
        "claude-opus-4-6",
        "opus-4-6"
    ));
    assert!(super::super::matches_model_suffix(
        "us-claude-opus-4-6-v1",
        "opus-4-6"
    ));
    assert!(!super::super::matches_model_suffix(
        "opus-4-6-x-opus-4-60",
        "opus-4-6"
    ));
    assert!(!super::super::matches_model_suffix("opus-4", "opus-4-6"));
    assert!(!super::super::matches_model_suffix("xopus-4-6", "opus-4-6"));
    assert!(!super::super::matches_model_suffix("4opus-4-6", "opus-4-6"));
    assert!(super::super::matches_model_suffix("modèle-opus", "opus"));
    assert!(super::super::matches_model_suffix("anything", ""));
}

#[test]
fn builtin_price_aliases_require_an_explicit_table_entry() {
    assert_eq!(super::super::data::ALIASES.len(), 3);
}

#[test]
fn an_alias_to_a_google_spelling_reaches_the_gemini_row_when_resolving_and_as_an_override_base() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "private-model",
        "google/gemini-4-argon",
    )]);
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let resolved = pricing
        .find_at(
            "private-model",
            crate::parse_ts_timestamp("2026-12-02T00:00:00Z").unwrap(),
        )
        .unwrap();
    assert_eq!((resolved.input * 1e6, resolved.output * 1e6), (4.0, 20.0));

    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        ..Default::default()
    };
    let overrides =
        std::collections::BTreeMap::from([("private-model".to_owned(), override_value)]);
    let overridden = pricing
        .with_user_overrides(overrides.iter())
        .find("private-model")
        .unwrap();
    assert_eq!((overridden.input, overridden.output), (50e-6, 10e-6));
}
