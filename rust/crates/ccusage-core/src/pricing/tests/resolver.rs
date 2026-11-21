use super::{MatchKind, PricingMap, layered, timestamp};

#[test]
fn case_insensitive_identity_beats_a_hosted_fuzzy_hit() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let model = "GLM-4.7";
    let found = pricing
        .explain(Some(model), &[(model, MatchKind::Fuzzy)])
        .unwrap();
    assert_eq!(found.row_key, "glm-4.7");
    let rates = pricing.find(model).unwrap();
    assert_eq!((rates.input, rates.output), (0.60e-6, 2.20e-6));
}

#[test]
fn case_sensitive_hosted_identity_beats_a_folded_vendor_match() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    primary.load_json(
        r#"{"deepseek/deepseek-fixture.1": {
            "litellm_provider": "deepseek", "input_cost_per_token": 1,
            "output_cost_per_token": 2, "max_input_tokens": 1000
        }}"#,
    );
    let mut hosted = PricingMap::default();
    hosted.load_models_dev_json_for_tests(
        r#"{
            "host/DeepSeek-Fixture.1": {
                "cost": {"input": 9000000, "output": 10000000}, "limit": {"context": 2000}
            },
            "host/Case-Only.1": {
                "cost": {"input": 3000000, "output": 4000000}, "limit": {"context": 3000}
            }
        }"#,
    );
    let pricing = layered(&primary, Some(&hosted));
    for (model, input, limit) in [
        ("host/DeepSeek-Fixture.1", 9.0, 2000),
        ("host/DeepSeek-Fixture-1", 9.0, 2000),
        ("DEEPSEEK-FIXTURE.1", 1.0, 1000),
        ("host/case-only.1", 3.0, 3000),
    ] {
        assert_eq!(pricing.find(model).unwrap().input, input, "{model}");
        assert_eq!(pricing.context_limit(model), Some(limit), "{model}");
    }
}

#[test]
fn case_fallback_keeps_the_direct_vendors_schedule() {
    use super::super::BillingRoute;

    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let model = "DEEPSEEK-V4-PRO";
    let instant = Some(timestamp("2026-11-17T03:00:00Z"));
    let (_, rates) = pricing
        .resolve_on_route_at(
            Some(model),
            &[(model, MatchKind::Fuzzy)],
            BillingRoute::Direct("deepseek"),
            instant,
        )
        .unwrap();
    assert_eq!((rates.input, rates.output), (1.32e-6, 3.96e-6));
}

#[test]
fn direct_litellm_row_beats_the_bare_hosted_spelling() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "deepseek/deepseek-v3.2": {
                "litellm_provider": "deepseek",
                "input_cost_per_token": 0.00000028,
                "output_cost_per_token": 0.00000040
            },
            "deepseek.v3.2": {
                "litellm_provider": "bedrock_converse",
                "input_cost_per_token": 0.00000062,
                "output_cost_per_token": 0.00000185
            }
        }"#,
    );
    for model in [
        "deepseek-v3-2",
        "deepseek-v3.2",
        "deepseek.v3.2",
        "deepseek/deepseek-v3.2",
    ] {
        let price = pricing.find(model).unwrap();
        assert_eq!(price.input, 0.28e-6, "{model}");
        assert_eq!(price.output, 0.40e-6, "{model}");
    }
}

#[test]
fn all_match_kinds_prefer_the_same_id_over_longer_primary_keys() {
    crate::test_support::install_pricing_snapshots();
    use super::super::MatchKind;
    let mut primary = PricingMap::default();
    primary.load_json(
        r#"{
            "sample-1.5-pro": {"input_cost_per_token": 9, "output_cost_per_token": 9},
            "sample-1.5": {"input_cost_per_token": 1, "output_cost_per_token": 1}
        }"#,
    );
    let mut embedded = PricingMap::default();
    embedded.load_json(
        r#"{
            "o1": {"input_cost_per_token": 15, "output_cost_per_token": 60}
        }"#,
    );
    primary.load_json(
        r#"{"azure/o1-mini": {"input_cost_per_token": 1.21, "output_cost_per_token": 4.84}}"#,
    );
    primary.complete_layers(&embedded);
    for kind in [MatchKind::StrictExact, MatchKind::Exact, MatchKind::Fuzzy] {
        let model = if kind == MatchKind::StrictExact {
            "sample-1.5"
        } else {
            "sample-1-5"
        };
        assert_eq!(
            primary
                .resolve_at(None, &[(model, kind)], None)
                .unwrap()
                .1
                .input,
            1.0
        );
    }
    for kind in [MatchKind::Exact, MatchKind::Fuzzy] {
        assert_eq!(
            primary
                .resolve_at(None, &[("o1", kind)], None)
                .unwrap()
                .1
                .input,
            15.0
        );
    }
}

#[test]
fn strict_candidates_only_admit_literal_primary_keys() {
    crate::test_support::install_pricing_snapshots();
    use super::super::MatchKind;
    let mut pricing = PricingMap::default();
    let mut embedded = PricingMap::default();
    embedded.load_json(
        r#"{
            "reseller/model-1.5": {"input_cost_per_token": 8, "output_cost_per_token": 9},
            "model-1.5": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
    );
    pricing.complete_layers(&embedded);
    let candidates = [
        ("reseller/model-1.5", MatchKind::StrictExact),
        ("model-1.5", MatchKind::Fuzzy),
    ];
    let (matched, rates) = pricing.resolve_at(None, &candidates, None).unwrap();
    assert_eq!(matched, "model-1.5");
    assert_eq!(rates.input, 1.0);
}

#[test]
fn same_identity_in_the_fallback_beats_a_distinct_vendor_model() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{
            "deepseek-v3.2": {"litellm_provider":"deepseek","input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#);
    let mut embedded = PricingMap::default();
    embedded.load_json(
        r#"{
            "reseller/deepseek-v3.2-speciale": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
    );
    pricing.complete_layers(&embedded);
    assert_eq!(
        pricing
            .find("reseller/deepseek-v3.2-speciale")
            .unwrap()
            .input,
        8.0
    );
}

#[test]
fn direct_pricing_keeps_the_fallback_row_for_distinct_variants() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "deepseek-v4-flash": {"input_cost_per_token": 1, "output_cost_per_token": 2}
        }"#,
    );
    let mut embedded = PricingMap::default();
    embedded.load_json(
        r#"{
            "reseller/deepseek-v4-flash-0731": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
    );
    pricing.complete_layers(&embedded);
    assert_eq!(pricing.find("deepseek-v4-flash").unwrap().input, 1.0);
    assert_eq!(
        pricing
            .find("reseller/deepseek-v4-flash-0731")
            .unwrap()
            .input,
        8.0
    );
}

#[test]
fn normalized_model_id_keeps_provider_qualification() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let mut embedded = PricingMap::default();
    embedded.load_json(
        r#"{
            "deepseek-v3-1": {"input_cost_per_token": 1, "output_cost_per_token": 2},
            "deepseek/deepseek-v3.1": {"input_cost_per_token": 8, "output_cost_per_token": 9}
        }"#,
    );
    pricing.complete_layers(&embedded);
    assert_eq!(pricing.find("deepseek/deepseek-v3-1").unwrap().input, 8.0);
}

#[test]
fn bare_hosted_rows_price_each_separator_spelling_when_no_direct_row_exists() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{
            "deepseek.v3.2": {"litellm_provider":"bedrock_converse","input_cost_per_token": 0.62, "output_cost_per_token": 1.85},
            "free-hosted": {"litellm_provider":"bedrock","input_cost_per_token": 0, "output_cost_per_token": 0}
        }"#);
    for model in ["deepseek.v3.2", "deepseek-v3-2", "deepseek-v3.2"] {
        let price = pricing.find(model).unwrap();
        assert_eq!(price.input, 0.62, "{model}");
        assert_eq!(price.output, 1.85, "{model}");
    }
    assert!(pricing.find("free-hosted").is_none());
}

#[test]
fn free_hosted_rows_answer_only_their_qualified_identity() {
    crate::test_support::install_pricing_snapshots();
    let mut table = PricingMap::default();
    assert_eq!(table.load_json(r#"{
            "host/free-1.5": {"litellm_provider":"host","input_cost_per_token":0,"output_cost_per_token":0,"max_input_tokens":123},
            "bare-free": {"litellm_provider":"host","input_cost_per_token":0,"output_cost_per_token":0},
            "unnamed-free": {"input_cost_per_token":0,"output_cost_per_token":0}
        }"#), 1);
    for name in ["host/free-1.5", "host/free-1-5"] {
        let (model, rate) = table
            .resolve_at(Some(name), &[(name, MatchKind::Fuzzy)], None)
            .unwrap();
        assert_eq!(model, name);
        assert_eq!((rate.input, rate.output), (0.0, 0.0));
        assert_eq!(table.context_limit(name), Some(123));
    }
    for name in [
        "bare-free",
        "unnamed-free",
        "free-1.5",
        "nest/host/free-1.5",
        "host/free-1.5-extra",
    ] {
        assert!(table.find(name).is_none(), "{name}");
        assert_eq!(table.context_limit(name), None, "{name}");
    }
}

#[test]
fn live_direct_provider_row_answers_the_unqualified_model_id() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    pricing.refresh_litellm(|load| {
        assert!(load(r#"{
                "xai/grok-code-fast-1": {"litellm_provider":"xai","input_cost_per_token":1e-6,"output_cost_per_token":2e-6}
            }"#));
        Ok(())
    });
    let model = "grok-code-fast-1";
    let found = pricing
        .explain(Some(model), &[(model, MatchKind::Exact)])
        .unwrap();
    assert_eq!(
        (found.step, found.row_key),
        (
            super::super::ResolutionStep::AuthoritativeIdentity,
            "xai/grok-code-fast-1"
        )
    );
    let rate = pricing.find(model).unwrap();
    assert_eq!((rate.input, rate.output), (1e-6, 2e-6));
}

#[test]
fn free_vendor_row_behind_a_path_yields_to_the_ids_own_listing() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    use super::super::ResolutionStep::{
        AuthoritativeIdentity, AuthoritativeSuffix, ModelsDevIdentity,
    };
    let model = "novita/glm-4.7-flash";
    let candidates = [
        ("openrouter/novita/glm-4.7-flash", MatchKind::Exact),
        (model, MatchKind::Fuzzy),
    ];
    let found = pricing.explain(Some(model), &candidates).unwrap();
    assert_eq!((found.step, found.row_key), (ModelsDevIdentity, model));
    let (_, rates) = pricing.resolve_at(Some(model), &candidates, None).unwrap();
    assert!((rates.input - 0.0707e-6).abs() < 1e-15);
    assert!((rates.output - 0.404e-6).abs() < 1e-15);

    for (model, input, output, step, row) in [
        (
            "novita/glm-4.7-flash",
            0.0707e-6,
            0.404e-6,
            ModelsDevIdentity,
            "novita/glm-4.7-flash",
        ),
        (
            "z-ai/glm-4.7-flash",
            0.13225e-6,
            0.529e-6,
            ModelsDevIdentity,
            "z-ai/glm-4.7-flash",
        ),
        (
            "@cf/zai-org/glm-4.7-flash",
            0.0605e-6,
            0.4e-6,
            ModelsDevIdentity,
            "@cf/zai-org/glm-4.7-flash",
        ),
        (
            "openrouter/z-ai/glm-4.7-flash",
            0.13225e-6,
            0.529e-6,
            ModelsDevIdentity,
            "z-ai/glm-4.7-flash",
        ),
        (
            "kyma/glm-4.7-flash",
            0.0,
            0.0,
            AuthoritativeSuffix,
            "zai/glm-4.7-flash",
        ),
        (
            "glm-4.7-flash",
            0.0,
            0.0,
            AuthoritativeIdentity,
            "zai/glm-4.7-flash",
        ),
        (
            "zai/glm-4.7-flash",
            0.0,
            0.0,
            AuthoritativeIdentity,
            "zai/glm-4.7-flash",
        ),
    ] {
        for kind in [MatchKind::Exact, MatchKind::Fuzzy] {
            let candidates = [(model, kind)];
            let (_, rate) = pricing.resolve_at(Some(model), &candidates, None).unwrap();
            let found = pricing.explain(Some(model), &candidates).unwrap();
            assert_eq!((found.step, found.row_key), (step, row), "{model} {kind:?}");
            assert!(
                (rate.input - input).abs() < 1e-15,
                "{model} {kind:?}: {}",
                rate.input
            );
            assert!(
                (rate.output - output).abs() < 1e-15,
                "{model} {kind:?}: {}",
                rate.output
            );
        }
    }
}

#[test]
fn vendor_path_suffix_and_same_identity_outrank_fuzzy_matches() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    primary.load_json(r#"{"sample": {"litellm_provider":"openai","input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{
                "vendor-sample": { "cost": { "input": 3000000, "output": 4000000 } },
                "vendor/sample": { "cost": { "input": 3000000, "output": 4000000 } },
                "sample-tier": { "cost": { "input": 5000000, "output": 6000000 }, "exactOnly": true },
                "unrelated-9": { "cost": { "input": 7000000, "output": 8000000 } }
            }"#,
    );
    let pricing = layered(&primary, Some(&embedded));
    assert_eq!(pricing.find("vendor/sample").unwrap().input, 1.0);
    assert_eq!(pricing.find("vendor-sample").unwrap().input, 3.0);
    assert_eq!(
        pricing
            .resolve_at(
                None,
                &[("vendor/sample", super::super::MatchKind::Exact)],
                None
            )
            .unwrap()
            .1
            .input,
        1.0
    );
    assert_eq!(pricing.find("sample-tier").unwrap().input, 5.0);
    assert_eq!(pricing.find("unrelated-9").unwrap().input, 7.0);
}

#[test]
fn ordered_candidates_keep_free_prices_and_per_candidate_match_kinds() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "free": {"litellm_provider":"openai","input_cost_per_token": 0, "output_cost_per_token": 0},
            "paid": {"input_cost_per_token": 1, "output_cost_per_token": 2},
            "provider/paid-version": {"input_cost_per_token": 9, "output_cost_per_token": 10}
        }"#,
    );
    let (matched, free) = pricing
        .resolve_at(
            None,
            &[
                ("free", super::super::MatchKind::Fuzzy),
                ("paid", super::super::MatchKind::Fuzzy),
            ],
            None,
        )
        .unwrap();
    assert_eq!(matched, "free");
    assert_eq!(free.input, 0.0);
    let (matched, paid) = pricing
        .resolve_at(
            None,
            &[
                ("provider/paid", super::super::MatchKind::Exact),
                ("paid", super::super::MatchKind::Fuzzy),
            ],
            None,
        )
        .unwrap();
    assert_eq!(matched, "paid");
    assert_eq!(paid.input, 1.0);
    let (matched, price) = pricing
        .resolve_at(
            None,
            &[
                ("provider/paid", super::super::MatchKind::Fuzzy),
                ("paid", super::super::MatchKind::Fuzzy),
            ],
            None,
        )
        .unwrap();
    assert_eq!(matched, "paid");
    assert_eq!(price.input, 1.0);
}

#[test]
fn respelled_adapter_ids_use_their_own_list_rates() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, input, output) in [
        ("gpt-5-1", 1.25, 10.0),
        ("gpt-5-1-codex", 1.25, 10.0),
        ("gpt-5-2", 1.75, 14.0),
        ("gpt-5-4", 2.5, 15.0),
        ("gpt-5-5", 5.0, 30.0),
        ("gpt-5-6", 4.0, 20.0),
        ("gpt-4-1", 2.0, 8.0),
        ("gemini-2-5-flash", 0.3, 2.5),
        ("gemini-3-5-flash", 1.5, 9.0),
        ("glm-4-7", 0.6, 2.2),
        ("deepseek/deepseek.v4.flash", 0.3, 1.2),
        ("deepseek/deepseek.v4.pro", 1.32, 3.96),
    ] {
        let rates = pricing
            .resolve_at(None, &[(model, super::super::MatchKind::Fuzzy)], None)
            .unwrap()
            .1;
        let direct = pricing.find(model).unwrap();
        assert_eq!(
            (rates.input, rates.output),
            (direct.input, direct.output),
            "{model}"
        );
        assert!((rates.input * 1e6 - input).abs() < 1e-9, "{model}");
        assert!((rates.output * 1e6 - output).abs() < 1e-9, "{model}");
    }
}

#[test]
fn exact_candidate_kinds_keep_the_same_model_and_respelled_overrides() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    pricing.load_json(
        r#"{"gpt-5-5":{"input_cost_per_token":0.000005,"output_cost_per_token":0.00003}}"#,
    );
    for kind in [
        super::super::MatchKind::Exact,
        super::super::MatchKind::StrictExact,
    ] {
        let (_, price) = pricing
            .resolve_at(None, &[("gpt-5-5", kind)], None)
            .unwrap();
        assert_eq!(price.input, 5e-6);
        assert_eq!(price.output, 30e-6);
    }
    let key = "claude-sonnet-4-5".to_string();
    let value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(7e-6),
        ..Default::default()
    };
    let pricing = pricing.with_user_overrides([(&key, &value)]);
    for model in [
        "claude-sonnet-4-5",
        "claude-sonnet-4.5",
        "claude.sonnet.4.5",
    ] {
        assert!(pricing.has_user_override(model), "{model}");
        for kind in [
            super::super::MatchKind::Exact,
            super::super::MatchKind::Fuzzy,
        ] {
            assert_eq!(
                pricing
                    .resolve_at(None, &[(model, kind)], None)
                    .unwrap()
                    .1
                    .input,
                7e-6
            );
        }
        assert_eq!(pricing.find(model).unwrap().input, 7e-6);
    }
    let mut primary = PricingMap::default();
    primary.load_json(r#"{"sample-pro":{"input_cost_per_token":9,"output_cost_per_token":18}}"#);
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"sample":{"cost":{"input":1000000,"output":2000000}}}"#,
    );
    let pricing = layered(&primary, Some(&embedded));
    let (_, price) = pricing
        .resolve_at(None, &[("sample", super::super::MatchKind::Exact)], None)
        .unwrap();
    assert_eq!(price.input, 1.0);
    assert_eq!(price.output, 2.0);
}

#[test]
fn strict_candidates_use_authoritative_identities_without_path_suffixes() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    primary.load_json(r#"{"sample":{"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":2},"grok-4-1-x":{"input_cost_per_token":3,"output_cost_per_token":4}}"#);
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"vendor/sample":{"cost":{"input":3000000,"output":4000000}}}"#,
    );
    let pricing = layered(&primary, Some(&embedded));
    assert_eq!(
        pricing
            .resolve_at(
                None,
                &[("vendor/sample", super::super::MatchKind::Exact)],
                None
            )
            .unwrap()
            .1
            .input,
        1.0
    );
    assert!(
        pricing
            .resolve_at(
                None,
                &[("vendor/sample", super::super::MatchKind::StrictExact)],
                None
            )
            .is_none()
    );
    assert_eq!(
        pricing
            .resolve_at(
                None,
                &[("grok-4.1-x", super::super::MatchKind::StrictExact)],
                None
            )
            .unwrap()
            .1
            .input,
        3.0
    );
    assert_eq!(
        pricing
            .resolve_at(
                None,
                &[("grok-4.1-x", super::super::MatchKind::Exact)],
                None
            )
            .unwrap()
            .1
            .input,
        3.0
    );
}

#[test]
fn resolver_table_vendor_free_rows_only_answer_their_identity_or_segment() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    map.load_json(r#"{
            "plan-2.5": {"litellm_provider":"zai","input_cost_per_token": 1,"output_cost_per_token":2},
            "zai/plan-2.5-flash": {"litellm_provider":"zai","input_cost_per_token": 0,"output_cost_per_token":0},
            "host/plan-2.5-flash": {"litellm_provider":"reseller","input_cost_per_token": 0,"output_cost_per_token":0}
        }"#);
    for (case, name, expected) in [
        ("54_bare_free", "plan-2.5-flash", (0.0, 0.0)),
        (
            "55_foreign_prefix_free",
            "foreign/plan-2-5-flash",
            (0.0, 0.0),
        ),
        ("53_qualified_free", "zai/plan-2-5-flash", (0.0, 0.0)),
    ] {
        let rates = map.find(name).expect(case);
        assert_eq!((rates.input, rates.output), expected, "{case}");
    }
    assert!(
        map.find("plan-2.5-flash-20260101").is_none(),
        "free_tier_does_not_win_fuzzy"
    );
    let (_, hosted) = map
        .resolve_at(
            None,
            &[("host/plan-2.5-flash", MatchKind::StrictExact)],
            None,
        )
        .unwrap();
    assert_eq!((hosted.input, hosted.output), (0.0, 0.0));
}

#[test]
fn resolver_table_segment_matches() {
    crate::test_support::install_pricing_snapshots();
    let map = PricingMap::load_embedded();
    for (case, model, expected) in [
        (
            "71_fireworks_short_flash",
            "deepseek-v4p1-flash",
            (0.3e-6, 1.2e-6),
        ),
        ("72_fireworks_short_glm", "glm-5p3", (1.4e-6, 4.4e-6)),
    ] {
        let entry = map.find(model).expect(case);
        assert!(
            (entry.input - expected.0).abs() < 1e-12,
            "{case}: {entry:?}"
        );
        assert!(
            (entry.output - expected.1).abs() < 1e-12,
            "{case}: {entry:?}"
        );
    }
}

#[test]
fn resolver_row_23_bare_id_reaches_the_direct_vendor_row() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let found = pricing
        .explain(None, &[("deepseek-v3.2", MatchKind::StrictExact)])
        .unwrap();
    assert_eq!(
        found.step,
        super::super::ResolutionStep::AuthoritativeIdentity
    );
    assert_eq!(found.row_key, "deepseek/deepseek-v3.2");
}

#[test]
fn conflicting_direct_vendor_rates_do_not_answer_by_vendor_id() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    assert_eq!(pricing.load_json(r#"{
            "openai/twin-1": {"litellm_provider":"openai","input_cost_per_token":1,"output_cost_per_token":1},
            "anthropic/twin-1": {"litellm_provider":"anthropic","input_cost_per_token":2,"output_cost_per_token":2}
        }"#), 2);
    assert!(
        pricing
            .resolve_at(None, &[("twin-1", MatchKind::Exact)], None)
            .is_none()
    );
    assert_eq!(pricing.find("openai/twin-1").unwrap().input, 1.0);
    assert_eq!(pricing.find("anthropic/twin-1").unwrap().input, 2.0);
}

#[test]
fn resolver_table() {
    use super::super::ResolutionStep::{
        AuthoritativeFuzzy, AuthoritativeIdentity, AuthoritativeSuffix, ModelsDevIdentity,
        ReportedOverride,
    };

    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "fixture-2.5": {"input_cost_per_token":2,"output_cost_per_token":3},
            "zai/desk-model-2.5": {"litellm_provider":"zai","input_cost_per_token":3,"output_cost_per_token":4},
            "catalogue/same-longer": {"input_cost_per_token":4,"output_cost_per_token":5},
            "standard/tiny-model-3": {"input_cost_per_token":5,"output_cost_per_token":6},
            "other/catalogue/tiny-model-3-preview": {"input_cost_per_token":6,"output_cost_per_token":7},
            "zai/free-niche-model": {"litellm_provider":"zai","input_cost_per_token":0,"output_cost_per_token":0},
            "deepseek/deepseek-v4-flash": {"litellm_provider":"deepseek","input_cost_per_token":1,"output_cost_per_token":2},
            "gpt-cache-fixture": {"input_cost_per_token":8,"output_cost_per_token":9},
            "override-base": {"litellm_provider":"zai","input_cost_per_token":9,"output_cost_per_token":10},
            "zai/override-base": {"litellm_provider":"zai","input_cost_per_token":10,"output_cost_per_token":11},
            "embedding-only": {"input_cost_per_token":11,"output_cost_per_token":0},
            "provider/embedding-with-output": {"input_cost_per_token":12,"output_cost_per_token":13}
        }"#,
    );
    let mut models_dev = PricingMap::default();
    models_dev
        .load_models_dev_json_for_tests(r#"{"provider/same":{"cost":{"input":7,"output":8}}}"#)
        .unwrap();
    let keys = ["reported-model".to_owned(), "override-base".to_owned()];
    let patch = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50.0),
        ..Default::default()
    };
    let pricing = layered(&pricing, Some(&models_dev))
        .with_user_overrides(keys.iter().map(|key| (key, &patch)));
    let fuzzy = MatchKind::Fuzzy;
    let strict = MatchKind::StrictExact;
    let cases = [
        (
            "R1_45_reported_override",
            "reported-model",
            &[("fixture-2.5", fuzzy)][..],
            ReportedOverride,
            (50.0, 0.0),
        ),
        (
            "R2_10_separator_identity",
            "fixture-2-5",
            &[("fixture-2-5", fuzzy)][..],
            AuthoritativeIdentity,
            (2.0, 3.0),
        ),
        (
            "R3_authoritative_path_suffix",
            "reseller/zai/desk-model-2-5",
            &[("reseller/zai/desk-model-2-5", fuzzy)][..],
            AuthoritativeSuffix,
            (3.0, 4.0),
        ),
        (
            "R4_73_later_identity_beats_fuzzy",
            "same",
            &[("same", fuzzy), ("provider/same", fuzzy)][..],
            ModelsDevIdentity,
            (7e-6, 8e-6),
        ),
        (
            "R5_71_fuzzy_segment",
            "tiny-model-3",
            &[("tiny-model-3", fuzzy)][..],
            AuthoritativeFuzzy,
            (5.0, 6.0),
        ),
        (
            "R6_55_free_final_segment",
            "foreign/free-niche-model",
            &[("foreign/free-niche-model", fuzzy)][..],
            AuthoritativeSuffix,
            (0.0, 0.0),
        ),
        (
            "R7_34_vendor_schedule",
            "deepseek-v4-flash",
            &[("deepseek-v4-flash", fuzzy)][..],
            AuthoritativeIdentity,
            (0.15e-6, 0.6e-6),
        ),
        (
            "R8_9_vendor_cache_default",
            "gpt-cache-fixture",
            &[("gpt-cache-fixture", fuzzy)][..],
            AuthoritativeIdentity,
            (8.0, 9.0),
        ),
        (
            "R9_48_override_inherits_same_model",
            "zai/override-base",
            &[("zai/override-base", fuzzy)][..],
            ReportedOverride,
            (50.0, 10.0),
        ),
        (
            "R10_94_zero_output_keeps_selected_row",
            "embedding-only",
            &[
                ("embedding-only", strict),
                ("provider/embedding-with-output", fuzzy),
            ][..],
            AuthoritativeIdentity,
            (11.0, 0.0),
        ),
    ];
    let instant = timestamp("2026-11-17T10:00:00Z");
    for (case, label, candidates, step, expected) in cases {
        let (_, rates) = pricing
            .resolve_at(Some(label), candidates, Some(instant))
            .expect(case);
        assert!(
            (rates.input - expected.0).abs() < 1e-12,
            "{case}: {rates:?}"
        );
        assert!(
            (rates.output - expected.1).abs() < 1e-12,
            "{case}: {rates:?}"
        );
        assert_eq!(
            pricing.explain(Some(label), candidates).expect(case).step,
            step,
            "{case}"
        );
    }
    assert_eq!(
        pricing
            .find("gpt-cache-fixture")
            .unwrap()
            .cache_creation_rate(),
        8.0,
        "R8_9_vendor_cache_default"
    );
}

#[test]
fn resolver_table_14_qualified_google_flash_uses_the_models_dev_rate() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let model = "google/gemini-2.5-flash";
    let entry = pricing.find(model).unwrap();
    assert_eq!((entry.input, entry.output), (0.0000003, 0.0000025));
    let resolved = pricing
        .explain(Some(model), &[(model, MatchKind::Fuzzy)])
        .unwrap();
    assert_eq!(
        resolved.step,
        super::super::ResolutionStep::ModelsDevIdentity
    );
}

#[test]
fn a_router_path_reaches_the_models_dev_row_of_the_vendor_id_behind_it() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, row) in [
        (
            "openrouter/anthropic/claude-opus-4.8-fast",
            "anthropic/claude-opus-4.8-fast",
        ),
        (
            "openrouter/openai/gpt-6.2-sol-fast",
            "openai/gpt-6.2-sol-fast",
        ),
        (
            "openrouter/deepseek/deepseek-v4-flash-0731",
            "deepseek/deepseek-v4-flash-0731",
        ),
        (
            "openrouter/deepseek/deepseek-v4-pro-0813",
            "deepseek/deepseek-v4-pro-0813",
        ),
    ] {
        let found = pricing
            .explain(Some(model), &[(model, MatchKind::Fuzzy)])
            .unwrap();
        assert_eq!(
            (found.step, found.row_key),
            (super::super::ResolutionStep::ModelsDevIdentity, row),
            "{model}"
        );
        assert_eq!(pricing.find(model), pricing.find(row), "{model}");
    }
}
