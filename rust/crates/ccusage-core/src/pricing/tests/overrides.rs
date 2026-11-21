use super::super::{Pricing, PricingMap};
use super::{AtomicUsize, MatchKind, Ordering, layered};
use ccusage_cli::PricingOverride;
use std::collections::BTreeMap;

fn build_overrides<F: FnOnce(&mut PricingOverride)>(
    model: &str,
    init: F,
) -> BTreeMap<String, PricingOverride> {
    let mut override_value = PricingOverride::default();
    init(&mut override_value);
    let mut map = BTreeMap::new();
    map.insert(model.to_string(), override_value);
    map
}

#[test]
fn user_override_detection_resolves_configured_model_aliases() {
    let _aliases =
        crate::model_aliases::set_model_aliases_for_tests([("private-model", "custom-model")]);
    let mut pricing = PricingMap::default();
    let overrides = build_overrides("custom-model", |o| {
        o.input_cost_per_token = Some(0.001);
    });
    pricing.apply_overrides(overrides.iter());

    assert!(pricing.has_user_override("custom-model"));
    assert!(pricing.has_user_override("private-model"));
    assert!(!pricing.has_user_override("unrelated-model"));
}

#[test]
fn full_override_creates_new_model() {
    let mut pricing = PricingMap::default();
    let overrides = build_overrides("custom-model", |o| {
        o.input_cost_per_token = Some(1e-6);
        o.output_cost_per_token = Some(2e-6);
        o.cache_creation_input_token_cost = Some(3e-6);
        o.cache_read_input_token_cost = Some(4e-7);
        o.fast_multiplier = Some(2.0);
        o.max_input_tokens = Some(123_456);
    });

    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("custom-model").unwrap();
    assert_eq!(entry.input, 1e-6);
    assert_eq!(entry.output, 2e-6);
    assert_eq!(entry.cache_create, Some(3e-6));
    assert_eq!(entry.cache_read, Some(4e-7));
    assert_eq!(entry.fast_multiplier, 2.0);
    assert_eq!(pricing.context_limit("custom-model"), Some(123_456));
}

#[test]
fn override_only_openai_models_charge_cache_writes_at_input_rate() {
    let input_rate = 1e-6;
    for model in ["gpt-5.5-private", "[pi] gpt-5.5-private"] {
        let mut pricing = PricingMap::default();
        let overrides = build_overrides(model, |value| {
            value.input_cost_per_token = Some(input_rate);
        });
        pricing.apply_overrides(overrides.iter());

        let entry = pricing.find(model).unwrap();
        assert_eq!(entry.cache_creation_rate(), input_rate, "{model}");
        assert_eq!(entry.one_hour_cache_creation_rate(), input_rate, "{model}");
    }
}

#[test]
fn exact_override_wins_over_gpt_5_6_alias() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    let sol = pricing.find("gpt-5.6-sol").unwrap();
    let overrides = build_overrides("gpt-5.6", |o| {
        o.input_cost_per_token = Some(42e-6);
        o.max_input_tokens = Some(654_321);
    });

    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("gpt-5.6").unwrap();
    assert_eq!(entry.input, 42e-6);
    assert_eq!(entry.output, sol.output);
    assert_eq!(entry.cache_create, sol.cache_create);
    assert_eq!(entry.cache_read, sol.cache_read);
    assert_eq!(entry.long_context, None);
    assert_eq!(entry.fast_multiplier, sol.fast_multiplier);
    assert_eq!(pricing.context_limit("gpt-5.6"), Some(654_321));
}

#[test]
fn partial_override_preserves_existing_fields() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "existing".to_string(),
        Pricing {
            input: 10e-6,
            output: 20e-6,
            cache_create: Some(30e-6),
            cache_read: Some(40e-6),
            long_context: crate::pricing::LongContextRates {
                input: Some(15e-6),
                ..Default::default()
            }
            .marginal(),
            fast_multiplier: 1.5,
            ..Pricing::empty()
        },
    );

    let overrides = build_overrides("existing", |o| {
        o.input_cost_per_token = Some(99e-6);
        o.flex_multiplier = Some(0.5);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("existing").unwrap();
    assert_eq!(entry.input, 99e-6);
    assert_eq!(entry.output, 20e-6);
    assert_eq!(entry.cache_create, Some(30e-6));
    assert_eq!(entry.cache_read, Some(40e-6));
    assert_eq!(entry.long_context, None);
    assert_eq!(entry.fast_multiplier, 1.5);
    assert_eq!(entry.flex_multiplier, 0.5);
}

#[test]
fn override_without_cache_read_keeps_it_absent() {
    let mut pricing = PricingMap::default();
    let overrides = build_overrides("new-model", |o| {
        o.input_cost_per_token = Some(1e-6);
    });

    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("new-model").unwrap();
    assert_eq!(entry.cache_read, None);
}

#[test]
fn override_with_zero_cache_read_preserves_zero() {
    let mut pricing = PricingMap::default();
    let overrides = build_overrides("new-model", |o| {
        o.cache_read_input_token_cost = Some(0.0);
    });

    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("new-model").unwrap();
    assert_eq!(entry.cache_read, Some(0.0));
}

#[test]
fn max_input_tokens_writes_context_limits() {
    let mut pricing = PricingMap::default();
    let overrides = build_overrides("with-limit", |o| {
        o.max_input_tokens = Some(2_000_000);
    });
    pricing.apply_overrides(overrides.iter());
    assert_eq!(pricing.context_limit("with-limit"), Some(2_000_000));
}

#[test]
fn missing_max_input_tokens_does_not_clobber_existing_limit() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_limit("model", 500_000);
    let overrides = build_overrides("model", |o| {
        o.input_cost_per_token = Some(1e-6);
    });
    pricing.apply_overrides(overrides.iter());
    assert_eq!(pricing.context_limit("model"), Some(500_000));
}

#[test]
fn input_override_recomputes_absent_cache_rates_and_drops_published_tiers() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "claude-model".to_string(),
        Pricing {
            input: 3e-6,
            output: 15e-6,
            long_context: crate::pricing::LongContextRates {
                cache_create: Some(4.6875e-6),
                cache_read: Some(3.75e-7),
                ..Default::default()
            }
            .marginal(),
            ..Pricing::empty()
        },
    );

    let overrides = build_overrides("claude-model", |o| {
        o.input_cost_per_token = Some(2e-6);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("claude-model").unwrap();
    assert_eq!(entry.input, 2e-6);
    assert_eq!(entry.output, 15e-6);
    assert_eq!(entry.cache_create, None);
    assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
    assert_eq!(entry.cache_read, None);
    assert!((entry.cache_read_rate_at(entry.input) - 2e-7).abs() < 1e-15);
    assert_eq!(entry.long_context, None);
}

#[test]
fn input_override_does_not_scale_zero_cache() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "no-cache-model".to_string(),
        Pricing {
            input: 5e-6,
            output: 10e-6,
            cache_create: Some(0.0),
            cache_read: Some(0.0),
            ..Pricing::empty()
        },
    );

    let overrides = build_overrides("no-cache-model", |o| {
        o.input_cost_per_token = Some(2e-6);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("no-cache-model").unwrap();
    assert_eq!(entry.cache_create, Some(0.0));
    assert_eq!(entry.cache_read, Some(0.0));
}

#[test]
fn explicit_cache_override_replaces_the_default() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "model".to_string(),
        Pricing {
            input: 3e-6,
            output: 15e-6,
            ..Pricing::empty()
        },
    );

    let overrides = build_overrides("model", |o| {
        o.input_cost_per_token = Some(2e-6);
        o.cache_read_input_token_cost = Some(5e-7);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("model").unwrap();
    assert_eq!(entry.input, 2e-6);
    assert_eq!(entry.cache_read, Some(5e-7));
    assert_eq!(entry.cache_create, None);
    assert!((entry.cache_creation_rate() - 2.5e-6).abs() < 1e-15);
}

#[test]
fn override_answers_the_model_id_behind_a_label_path() {
    crate::test_support::install_pricing_snapshots();
    let embedded = PricingMap::load_embedded();
    let mut live = PricingMap::load_embedded();
    live.refresh_litellm(|load| {
        load(r#"{
                "xai/grok-4.3": {"litellm_provider":"xai","input_cost_per_token":1.25e-6,"output_cost_per_token":2.5e-6},
                "gemini/gemini-2.5-pro": {"litellm_provider":"gemini","input_cost_per_token":1.25e-6,"output_cost_per_token":1e-5},
                "hostx/acme-1": {"litellm_provider":"hostx","input_cost_per_token":3e-6,"output_cost_per_token":4e-6}
            }"#);
        Ok(())
    });
    let override_at = |rate| ccusage_cli::PricingOverride {
        input_cost_per_token: Some(rate),
        output_cost_per_token: Some(rate),
        ..Default::default()
    };
    let overrides =
        ["grok-4.3", "gemini-2.5-pro", "acme-1"].map(|key| (key.to_owned(), override_at(50e-6)));
    let specific = ("hostx/acme-1".to_owned(), override_at(7e-6));
    for base in [&embedded, &live] {
        let pricing = base.with_user_overrides(overrides.iter().map(|(key, rate)| (key, rate)));
        for label in [
            "xai/grok-4.3",
            "openrouter/xai/grok-4.3",
            "gemini/gemini-2.5-pro",
            "hostx/acme-1",
        ] {
            let candidates = [(label, MatchKind::Fuzzy)];
            let found = pricing.explain(Some(label), &candidates).unwrap();
            assert_eq!(
                found.step,
                super::super::ResolutionStep::ReportedOverride,
                "{label}"
            );
            let rate = pricing.find(label).unwrap();
            assert_eq!((rate.input, rate.output), (50e-6, 50e-6), "{label}");
            assert!(pricing.has_user_override(label), "{label}");
        }
        assert!(!pricing.has_user_override("hostx/acme-1-flash"));
        let pricing = base.with_user_overrides(
            overrides
                .iter()
                .chain([&specific])
                .map(|(key, rate)| (key, rate)),
        );
        let rate = pricing.find("hostx/acme-1").unwrap();
        assert_eq!((rate.input, rate.output), (7e-6, 7e-6));
    }
}

#[test]
fn override_competes_in_the_fuzzy_pool_under_the_shadowed_key() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    assert_eq!(primary.load_json(r#"{
            "openai/acme-9": {"litellm_provider":"openai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "acme-9-low": {"litellm_provider":"host","input_cost_per_token":2e-6,"output_cost_per_token":2e-6}
        }"#), 2);
    let key = "acme-9".to_owned();
    let value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        output_cost_per_token: Some(50e-6),
        ..Default::default()
    };
    let pricing = primary.with_user_overrides([(&key, &value)]);
    let label = "openai/acme-9-low-high";
    let candidates = [(label, MatchKind::Fuzzy)];
    let found = pricing.explain(Some(label), &candidates).unwrap();
    assert_eq!(
        (found.step, found.row_key),
        (super::super::ResolutionStep::AuthoritativeFuzzy, "acme-9")
    );
    let rate = pricing.find(label).unwrap();
    assert_eq!((rate.input, rate.output), (50e-6, 50e-6));
}

#[test]
fn provider_candidates_cannot_bypass_a_reported_override() {
    crate::test_support::install_pricing_snapshots();
    use super::super::MatchKind;
    let mut primary = PricingMap::default();
    primary.load_json(
        r#"{
            "vendor/sample-4.7": {"input_cost_per_token": 2, "output_cost_per_token": 3}
        }"#,
    );
    let override_id = "sample.4-7".to_string();
    let rates = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(5.0),
        output_cost_per_token: Some(6.0),
        ..ccusage_cli::PricingOverride::default()
    };
    let pricing = primary.with_user_overrides([(&override_id, &rates)]);
    let (_, entry) = pricing
        .resolve_at(
            Some("sample-4.7"),
            &[
                ("vendor/sample-4.7", MatchKind::Exact),
                ("sample-4.7", MatchKind::Fuzzy),
            ],
            None,
        )
        .unwrap();
    assert_eq!((entry.input, entry.output), (5.0, 6.0));
}

#[test]
fn input_override_moves_default_writes_on_curated_loaded_models() {
    crate::test_support::install_pricing_snapshots();
    for (model, expected) in [("gpt-5", 2e-6), ("moonshot/kimi-k2.5", 2.5e-6)] {
        let mut pricing = PricingMap::default();
        pricing.load_json(&format!(
            r#"{{"{model}": {{"input_cost_per_token": 0.00000125, "output_cost_per_token": 0.00001, "litellm_provider": "{}"}}}}"#,
            if model.starts_with("gpt-") { "openai" } else { "moonshot" }
        ));
        pricing.put_builtin_pricing();
        let overrides = [(
            model.to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(2e-6),
                ..ccusage_cli::PricingOverride::default()
            },
        )];
        let overridden =
            pricing.with_user_overrides(overrides.iter().map(|(id, value)| (id, value)));
        let price = overridden.find(model).unwrap();
        assert_eq!(price.cache_create, None, "{model}");
        assert!(
            (price.cache_creation_rate() - expected).abs() < 1e-15,
            "{model}"
        );
    }
}

#[test]
fn resolver_table_override_inherits_from_the_selected_live_catalogue() {
    crate::test_support::install_pricing_snapshots();
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"sample-only":{"cost":{"input":1,"output":2},"limit":{"context":120000}}}"#,
    );
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"sample-only":{"cost":{"input":8,"output":13,"tiers":[{"input":17,"tier":{"type":"context","size":200000}}]},"limit":{"context":300000}}}"#,
    );
    let id = "sample-only".to_string();
    let fields = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(5e-6),
        ..Default::default()
    };
    let map =
        layered(&PricingMap::default(), Some(&embedded)).with_user_overrides([(&id, &fields)]);
    let attempts = AtomicUsize::new(0);
    let load = || {
        attempts.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    assert_eq!(attempts.load(Ordering::Relaxed), 0);
    let rate = map.find_with_live(&id, load).unwrap();
    assert_eq!(rate.input, 5e-6);
    assert!((rate.output - 13e-6).abs() < 1e-12);
    assert!(rate.long_context.is_none());
    assert_eq!(map.context_limit(&id), Some(300_000));
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn fuzzy_override_uses_the_live_exact_only_flag() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    assert_eq!(
        primary.load_json(
            r#"{
            "host/sample": {"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":2}
        }"#
        ),
        1
    );
    let mut embedded = PricingMap::default();
    let mut live = PricingMap::default();
    assert_eq!(
        embedded.load_models_dev_json_for_tests(
            r#"{
            "host/sample":{"cost":{"input":3,"output":4}}
        }"#
        ),
        Some(1)
    );
    assert_eq!(
        live.load_models_dev_json_for_tests(
            r#"{
            "host/sample":{"cost":{"input":5,"output":6},"exactOnly":true}
        }"#
        ),
        Some(1)
    );
    let fields = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(8.0),
        ..Default::default()
    };
    let key = "host/sample".to_owned();
    let map = layered(&primary, Some(&embedded)).with_user_overrides([(&key, &fields)]);
    let attempts = AtomicUsize::new(0);
    let load = || {
        attempts.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };

    assert_eq!(map.find_with_live(&key, load).unwrap().input, 8.0);
    assert_eq!(attempts.load(Ordering::Relaxed), 0);
    assert!(map.find_with_live("sample", load).is_none());
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert!(map.overrides.resolved(0).unwrap().row.exact_only);
    assert_eq!(map.find_with_live(&key, load).unwrap().input, 8.0);
}

#[test]
fn resolver_table_override_stands_in_for_the_qualified_vendor_row() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    map.load_json(r#"{
            "sample-4.7": {"litellm_provider":"zai","input_cost_per_token":1,"output_cost_per_token":2,"max_input_tokens":120000,"input_cost_per_token_above_200k_tokens":5},
            "zai/sample-4.7": {"litellm_provider":"zai","input_cost_per_token":3,"output_cost_per_token":4,"max_input_tokens":90000}
        }"#);
    let id = "sample-4-7".to_string();
    let fields = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50.0),
        ..Default::default()
    };
    let map = map.with_user_overrides([(&id, &fields)]);
    for (case, model) in [
        ("reported", "sample-4.7"),
        ("respelled", "sample@4-7"),
        ("48_qualified", "zai/sample-4.7"),
        ("host_path", "reseller/zai/sample-4.7"),
    ] {
        let rate = map.find(model).expect(case);
        assert_eq!((rate.input, rate.output), (50.0, 2.0), "{case}");
        assert_eq!(rate.long_context, None, "{case}");
        assert_eq!(map.context_limit(model), Some(120_000), "{case}");
    }
}

#[test]
fn overrides_retain_the_models_dev_exact_only_boundary() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    primary.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"sample-premium": {"exactOnly": true, "cost": {"input": 3000000, "output": 4000000}}}"#,
    );
    let mut pricing = layered(&primary, Some(&embedded));
    pricing.enable_models_dev_fallback = false;
    let model = "sample-premium".to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(5.0),
        ..Default::default()
    };
    let overridden = pricing.with_user_overrides([(&model, &override_value)]);
    assert_eq!(overridden.find("sample-premium").unwrap().input, 5.0);
    assert_eq!(
        overridden.find("sample-premium-20260101").unwrap().input,
        5.0
    );
}

#[test]
fn loading_with_overrides_keeps_the_shared_base_unchanged() {
    crate::test_support::install_pricing_snapshots();
    let model = "gpt-5".to_string();
    let overrides = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(42e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &overrides)]);
    let base = PricingMap::load_with_overrides(true, false, std::iter::empty());
    assert_eq!(pricing.find(&model).unwrap().input, 42e-6);
    assert_ne!(base.find(&model).unwrap().input, 42e-6);
}

#[test]
fn one_hour_cache_override_replaces_the_published_rate() {
    crate::test_support::install_pricing_snapshots();
    let model = "claude-sonnet-4-20250514".to_string();
    for rate in [0.0, 7e-6] {
        let override_value = ccusage_cli::PricingOverride {
            cache_creation_input_token_cost_above_1hr: Some(rate),
            ..Default::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);
        assert_eq!(pricing.find(&model).unwrap().cache_create_1h, Some(rate));
    }
}

#[test]
fn keeps_published_speed_multipliers_for_user_price_overrides() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("private-sol", "gpt-6.1-sol"),
        ("private-dated-sol", "gpt-6.1-sol-2026-09-24"),
    ]);
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(3e-6),
        ..Default::default()
    };
    let sol = PricingMap::load_embedded().find("gpt-6.1-sol").unwrap();

    for model in ["gpt-6.1-sol-2026-09-24", "private-dated-sol", "private-sol"] {
        let key = model.to_string();
        let pricing = PricingMap::load_with_overrides(true, false, [(&key, &override_value)]);
        let overridden = pricing.find(model).unwrap();
        assert_eq!(overridden.input, 3e-6, "{model}");
        assert_eq!(overridden.fast_multiplier, 2.0, "{model}");
        assert_eq!(overridden.ultrafast_multiplier, 6.0, "{model}");
        assert_eq!(overridden.flex_multiplier, 0.5, "{model}");
    }

    let key = "private-sol".to_string();
    let aliased = PricingMap::load_with_overrides(true, false, [(&key, &override_value)])
        .find(&key)
        .unwrap();
    assert_eq!(aliased.output, sol.output);
}

#[test]
fn alias_cycle_overrides_build_on_the_snapshot_not_on_each_other() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("cycle-a", "cycle-b"),
        ("cycle-b", "cycle-a"),
        ("into-cycle", "cycle-b"),
    ]);
    let overrides = std::collections::BTreeMap::from([
        (
            "cycle-a".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(4e-6),
                ..Default::default()
            },
        ),
        (
            "cycle-b".to_string(),
            ccusage_cli::PricingOverride {
                output_cost_per_token: Some(9e-6),
                ..Default::default()
            },
        ),
        (
            "into-cycle".to_string(),
            ccusage_cli::PricingOverride {
                cache_read_input_token_cost: Some(1e-7),
                ..Default::default()
            },
        ),
    ]);
    let pricing = PricingMap::load_with_overrides(true, false, &overrides);

    let cycle_a = pricing
        .resolve_at(None, &[("cycle-a", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!((cycle_a.input, cycle_a.output), (4e-6, 0.0));
    let cycle_b = pricing
        .resolve_at(None, &[("cycle-b", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!((cycle_b.input, cycle_b.output), (0.0, 9e-6));
    let into_cycle = pricing
        .resolve_at(None, &[("into-cycle", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(
        (into_cycle.input, into_cycle.output, into_cycle.cache_read),
        (0.0, 9e-6, Some(1e-7))
    );
}

#[test]
fn applies_target_overrides_before_aliases_that_sort_first() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("a-private-sol", "gpt-6.1-sol"),
        ("cycle-a", "cycle-b"),
        ("cycle-b", "cycle-a"),
        ("a-into-cycle", "cycle-a"),
    ]);
    let target_override = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(3e-6),
        output_cost_per_token: Some(7e-6),
        ..Default::default()
    };
    let alias_override = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(5e-6),
        ..Default::default()
    };
    let cycle_override = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(4e-6),
        output_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let overrides = std::collections::BTreeMap::from([
        ("a-private-sol".to_string(), alias_override.clone()),
        ("gpt-6.1-sol".to_string(), target_override),
        ("cycle-a".to_string(), cycle_override.clone()),
        ("cycle-b".to_string(), cycle_override),
        ("a-into-cycle".to_string(), alias_override),
    ]);
    let pricing = PricingMap::load_with_overrides(true, false, &overrides);

    let alias = pricing.find("a-private-sol").unwrap();
    assert_eq!(alias.input, 5e-6);
    assert_eq!(alias.output, 7e-6);
    assert_eq!(pricing.find("cycle-a").unwrap().input, 4e-6);
    assert_eq!(pricing.find("cycle-b").unwrap().input, 4e-6);
    let into_cycle = pricing.find("a-into-cycle").unwrap();
    assert_eq!(into_cycle.input, 5e-6);
    assert_eq!(into_cycle.output, 9e-6);
}

#[test]
fn overrides_with_the_same_identity_merge_fields() {
    let first = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(10e-6),
        ..Default::default()
    };
    let second = ccusage_cli::PricingOverride {
        output_cost_per_token: Some(20e-6),
        ..Default::default()
    };
    let pricing = PricingMap::default().with_user_overrides([
        (&"k-1.5".to_owned(), &first),
        (&"k-1-5".to_owned(), &second),
    ]);
    for model in ["k-1.5", "k-1-5"] {
        let p = pricing.find(model).unwrap();
        assert_eq!((p.input, p.output), (10e-6, 20e-6), "{model}");
    }
}

#[test]
fn respelled_override_applies_to_the_models_dev_exact_only_row() {
    crate::test_support::install_pricing_snapshots();
    use crate::cli::PricingOverride;
    let original = "kimi-k2.7-code-highspeed";
    let key = "kimi-k2-7-code-highspeed".to_string();
    let baseline = PricingMap::load_embedded()
        .resolve_at(None, &[(original, MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let value = PricingOverride {
        input_cost_per_token: Some(7e-6),
        ..PricingOverride::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&key, &value)]);
    for resolved in [
        pricing.find(original),
        pricing
            .resolve_at(None, &[(original, MatchKind::Exact)], None)
            .map(|(_, rates)| rates),
        pricing
            .resolve_at(None, &[(original, MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing),
    ] {
        let resolved = resolved.unwrap();
        assert_eq!(resolved.input, 7e-6);
        assert_eq!(resolved.output, baseline.output);
        assert_eq!(resolved.cache_read, baseline.cache_read);
    }
    assert_eq!(
        pricing
            .resolve_at(None, &[(&key, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .input,
        7e-6
    );
}

#[test]
fn a_family_thinking_rule_matches_an_override_spelled_with_either_separator() {
    crate::test_support::install_pricing_snapshots();
    for key in ["proxy/qwen3.5-plus", "proxy/qwen3-5-plus"] {
        let mut pricing = PricingMap::default();
        let overrides = build_overrides(key, |o| {
            o.input_cost_per_token = Some(7e-6);
            o.output_cost_per_token = Some(8e-6);
        });
        pricing.apply_overrides(overrides.iter());
        for model in ["proxy/qwen3.5-plus-thinking", "proxy/qwen3-5-plus-thinking"] {
            let found = pricing
                .find(model)
                .unwrap_or_else(|| panic!("{key}: {model}"));
            assert_eq!((found.input, found.output), (7e-6, 8e-6), "{key}: {model}");
        }
    }
}

#[test]
fn a_google_spelled_override_inherits_the_gemini_row_like_its_vendor_spelling() {
    crate::test_support::install_pricing_snapshots();
    let base = PricingMap::load_embedded();
    let inherited = |key: &str| {
        let overrides = build_overrides(key, |o| {
            o.input_cost_per_token = Some(50e-6);
        });
        let found = base
            .with_user_overrides(overrides.iter())
            .find(key)
            .unwrap();
        (found.output, found.cache_read_rate_at(found.input))
    };

    let vendor = inherited("gemini/gemini-2.5-pro");
    assert_eq!(vendor.0, 10e-6);
    assert_eq!(inherited("google/gemini-2.5-pro"), vendor);
}

fn tiered_test_pricing() -> PricingMap {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "tiered".to_string(),
        Pricing {
            input: 1e-6,
            output: 2e-6,
            long_context: crate::pricing::LongContextRates {
                input: Some(0.5e-6),
                output: Some(1e-6),
                cache_read: Some(0.1e-6),
                ..Default::default()
            }
            .marginal()
            .map(|tier| crate::pricing::LongContext {
                threshold: 32_000,
                ..tier
            }),
            ..Pricing::empty()
        },
    );
    pricing
}

#[test]
fn a_base_rate_only_override_is_the_price_at_every_context_size() {
    let mut pricing = tiered_test_pricing();
    let overrides = build_overrides("tiered", |o| {
        o.input_cost_per_token = Some(50e-6);
        o.output_cost_per_token = Some(50e-6);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("tiered").unwrap();
    assert_eq!((entry.input, entry.output), (50e-6, 50e-6));
    assert_eq!(entry.long_context, None);
}

#[test]
fn an_override_tier_holds_only_the_above_threshold_rates_it_spells_out() {
    let mut pricing = tiered_test_pricing();
    let overrides = build_overrides("tiered", |o| {
        o.input_cost_per_token = Some(50e-6);
        o.output_cost_per_token = Some(50e-6);
        o.input_cost_per_token_above_200k_tokens = Some(80e-6);
    });
    pricing.apply_overrides(overrides.iter());

    let tier = pricing.find("tiered").unwrap().long_context.unwrap();
    assert_eq!(tier.threshold, 32_000);
    assert_eq!(
        (tier.rates.input, tier.rates.output, tier.rates.cache_read),
        (Some(80e-6), None, None)
    );
}

#[test]
fn an_override_tier_bills_omitted_one_hour_writes_at_the_base_one_hour_rate() {
    let mut pricing = tiered_test_pricing();
    let overrides = build_overrides("tiered", |o| {
        o.cache_creation_input_token_cost_above_1hr = Some(0.0);
        o.input_cost_per_token_above_200k_tokens = Some(2e-6);
    });
    pricing.apply_overrides(overrides.iter());

    let entry = pricing.find("tiered").unwrap();
    assert_eq!(entry.one_hour_cache_creation_rate_above_200k(), Some(0.0));
}

#[test]
fn an_override_without_token_rates_keeps_the_published_tier() {
    let mut pricing = tiered_test_pricing();
    let overrides = build_overrides("tiered", |o| {
        o.max_input_tokens = Some(500_000);
    });
    pricing.apply_overrides(overrides.iter());

    let rates = pricing.find("tiered").unwrap().long_context.unwrap().rates;
    assert_eq!((rates.input, rates.output), (Some(0.5e-6), Some(1e-6)));
}

#[test]
fn a_bare_override_covers_host_qualified_labels_with_their_own_catalogue_rate() {
    crate::test_support::install_pricing_snapshots();
    let embedded = PricingMap::load_embedded();
    let rates = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        output_cost_per_token: Some(50e-6),
        ..Default::default()
    };
    for (key, label) in [("gpt-5.5", "azure/gpt-5.5"), ("glm-4.7", "zai/glm-4.7")] {
        let published = embedded.find(label).unwrap();
        assert_ne!(
            (published.input, published.output),
            (50e-6, 50e-6),
            "{label}"
        );
        let id = key.to_owned();
        let pricing = embedded.with_user_overrides([(&id, &rates)]);
        let rate = pricing.find(label).unwrap();
        assert_eq!((rate.input, rate.output), (50e-6, 50e-6), "{label}");
    }
}

#[test]
fn a_layered_override_keeps_the_above_threshold_rates_its_parent_spells_out() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([("tiered-child", "tiered")]);
    let mut pricing = tiered_test_pricing();
    let parent = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        input_cost_per_token_above_200k_tokens: Some(80e-6),
        ..Default::default()
    };
    let child = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(60e-6),
        ..Default::default()
    };
    let (parent_key, child_key) = ("tiered".to_owned(), "tiered-child".to_owned());
    pricing.apply_overrides([(&parent_key, &parent), (&child_key, &child)]);

    let entry = pricing.find("tiered-child").unwrap();
    assert_eq!(entry.input, 60e-6);
    let tier = entry.long_context.unwrap();
    assert_eq!((tier.threshold, tier.rates.input), (32_000, Some(80e-6)));
}

#[test]
fn a_layered_override_tier_on_an_untiered_parent_uses_the_published_threshold() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([("tiered-child", "tiered")]);
    let mut pricing = tiered_test_pricing();
    let parent = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        ..Default::default()
    };
    let child = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(60e-6),
        input_cost_per_token_above_200k_tokens: Some(80e-6),
        ..Default::default()
    };
    let (parent_key, child_key) = ("tiered".to_owned(), "tiered-child".to_owned());
    pricing.apply_overrides([(&parent_key, &parent), (&child_key, &child)]);

    assert_eq!(pricing.find("tiered").unwrap().long_context, None);
    let entry = pricing.find("tiered-child").unwrap();
    assert_eq!(entry.input, 60e-6);
    let tier = entry.long_context.unwrap();
    assert_eq!((tier.threshold, tier.rates.input), (32_000, Some(80e-6)));
}

#[test]
fn a_full_label_override_wins_over_the_bare_key() {
    crate::test_support::install_pricing_snapshots();
    let embedded = PricingMap::load_embedded();
    let at = |rate| ccusage_cli::PricingOverride {
        input_cost_per_token: Some(rate),
        output_cost_per_token: Some(rate),
        ..Default::default()
    };
    let (bare, full) = ("gpt-5.5".to_owned(), "azure/gpt-5.5".to_owned());
    let (bare_rate, full_rate) = (at(50e-6), at(7e-6));
    let pricing = embedded.with_user_overrides([(&bare, &bare_rate), (&full, &full_rate)]);
    let host = pricing.find("azure/gpt-5.5").unwrap();
    assert_eq!((host.input, host.output), (7e-6, 7e-6));
    let direct = pricing.find("gpt-5.5").unwrap();
    assert_eq!((direct.input, direct.output), (50e-6, 50e-6));
}

#[test]
fn an_override_on_an_alias_of_a_google_spelling_inherits_the_gemini_row() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "private-gemini",
        "google/gemini-2.5-pro",
    )]);
    crate::test_support::install_pricing_snapshots();
    let mut base = PricingMap::default();
    base.load_json(
        r#"{"gemini/gemini-2.5-pro": {"litellm_provider": "gemini", "input_cost_per_token": 1.25e-6, "output_cost_per_token": 1e-5}}"#,
    );
    let overrides = build_overrides("private-gemini", |o| {
        o.input_cost_per_token = Some(50e-6);
    });

    let found = base
        .with_user_overrides(overrides.iter())
        .find("private-gemini")
        .unwrap();

    assert_eq!((found.input, found.output), (50e-6, 1e-5));
}
