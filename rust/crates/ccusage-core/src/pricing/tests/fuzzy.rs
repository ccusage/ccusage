use super::{MatchKind, Pricing, PricingMap, layered};

#[test]
fn fuzzy_keeps_a_shorter_id_row_and_drops_longer_ones() {
    crate::test_support::install_pricing_snapshots();
    let mut secondary = PricingMap::default();
    assert_eq!(
        secondary.load_models_dev_json_for_tests(
            r#"{
            "hostaaaaa/m-1":{"cost":{"input":1,"output":1}},
            "m-1-20260101":{"cost":{"input":2,"output":2}},
            "hostc/m-2":{"cost":{"input":2,"output":2}},
            "hostc/long/m-2-20260101":{"cost":{"input":3,"output":3}}
        }"#
        ),
        Some(4)
    );
    let pricing = layered(&PricingMap::default(), Some(&secondary));
    let found = pricing
        .explain(
            Some("hostaaaaa/m-1-20260101"),
            &[("hostaaaaa/m-1-20260101", MatchKind::Fuzzy)],
        )
        .unwrap();
    assert_eq!(
        (found.step, found.row_key),
        (
            super::super::ResolutionStep::ModelsDevIdentity,
            "m-1-20260101"
        )
    );
    for (model, row, rate) in [
        ("hostaaaaa/m-1-20260101-high", "hostaaaaa/m-1", 1e-6),
        ("m-2", "hostc/m-2", 2e-6),
    ] {
        let candidates = [(model, MatchKind::Fuzzy)];
        let found = pricing.explain(Some(model), &candidates).unwrap();
        assert_eq!(
            (found.step, found.row_key),
            (super::super::ResolutionStep::ModelsDevFuzzy, row),
            "{model}"
        );
        let found = pricing.find(model).unwrap();
        assert_eq!((found.input, found.output), (rate, rate), "{model}");
    }
}

#[test]
fn respelled_base_model_never_uses_a_longer_model_price() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let price = pricing.find("gpt-5-5").unwrap();
    assert_eq!(price.input, 5e-6);
    assert_eq!(price.output, 30e-6);
    assert_eq!(pricing.find("gpt-5.5-pro").unwrap().input, 30e-6);
}

#[test]
fn same_last_segment_wins_over_a_longer_fuzzy_key() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{
            "short/model-2.5": {"input_cost_per_token": 3, "output_cost_per_token": 4},
            "longer/catalog/model-2.5-extension": {"input_cost_per_token": 9, "output_cost_per_token": 10}
        }"#);
    for name in ["model-2.5", "model-2-5", "short/model-2@5"] {
        let rate = pricing.find(name).unwrap();
        assert_eq!((rate.input, rate.output), (3.0, 4.0), "{name}");
    }
}

#[test]
fn fuzzy_match_requires_model_key_boundaries() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "claude-opus-4-7".to_string(),
        Pricing {
            input: 5e-6,
            output: 25e-6,
            cache_create: Some(6.25e-6),
            cache_read: Some(0.5e-6),
            ..Pricing::empty()
        },
    );
    pricing.insert_test_pricing(
        "claude-opus-4".to_string(),
        Pricing {
            input: 15e-6,
            output: 75e-6,
            cache_create: Some(18.75e-6),
            cache_read: Some(1.5e-6),
            ..Pricing::empty()
        },
    );

    assert!(pricing.find("claude-opus-4.70").is_none());
}

#[test]
fn fuzzy_match_does_not_fall_back_across_numeric_model_versions() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "claude-opus-4".to_string(),
        Pricing {
            input: 15e-6,
            output: 75e-6,
            cache_create: Some(18.75e-6),
            cache_read: Some(1.5e-6),
            ..Pricing::empty()
        },
    );

    assert!(pricing.find("claude-opus-4.8-20260528").is_none());
    assert!(pricing.find("claude-opus-4-9").is_none());
    assert!(pricing.find("claude-opus-5").is_none());
    assert!(pricing.find("claude-opus-4.70").is_none());
    assert!(pricing.find("claude-opus-4-20250514").is_some());
}

#[test]
fn fuzzy_match_allows_date_like_suffixes_for_known_numeric_model_versions() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert!(pricing.find("claude-opus-4-8-20270898").is_some());
}

#[test]
fn fuzzy_match_prefers_longest_model_key() {
    let mut pricing = PricingMap::default();
    pricing.insert_test_pricing(
        "claude-sonnet-4".to_string(),
        Pricing {
            input: 1.0,
            output: 0.0,
            cache_create: Some(0.0),
            cache_read: Some(0.0),
            ..Pricing::empty()
        },
    );
    pricing.insert_test_pricing(
        "claude-sonnet-4-20250514".to_string(),
        Pricing {
            input: 2.0,
            output: 0.0,
            cache_create: Some(0.0),
            cache_read: Some(0.0),
            ..Pricing::empty()
        },
    );

    let matched = pricing.find("claude-sonnet-4-20250514-v1:0").unwrap();

    assert_eq!(matched.input, 2.0);
}

#[test]
fn an_unlisted_id_that_extends_a_listed_id_with_a_model_word_is_unpriced() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in [
        "gpt-5.4-codex",
        "gpt-5.3-codex-mini",
        "gpt-5.3-codex-max",
        "gpt-6.2-sol-mini",
        "gemini-3.8-flash-lite",
        "gemini-4-argon-lite",
        "kimi-k3-code",
        "kimi-k3-pro",
        "glm-5.3-turbo",
        "glm-5.3-air",
        "glm-5.3-code",
        "llama-4-scout-2",
        "claude-sonnet-5-5-fast",
        "claude-fable-5-1-fast",
        "deepseek-v4-flash-0812",
    ] {
        assert_eq!(pricing.find(model), None, "{model}");
    }
}

#[test]
fn a_model_word_in_front_of_a_listed_id_names_another_model() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in ["chat-gpt-5.4", "my-gpt-5.4-high", "gemini-3.5-flash-extra"] {
        assert_eq!(pricing.find(model), None, "{model}");
    }
}

#[test]
fn a_word_or_version_after_a_live_key_does_not_borrow_its_row() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    pricing.refresh_litellm(|load| {
        assert!(load(
            r#"{
                "mistral/mistral-small": {"litellm_provider":"mistral","input_cost_per_token":1e-7,"output_cost_per_token":3e-7},
                "mistral/mistral-medium": {"litellm_provider":"mistral","input_cost_per_token":1.5e-6,"output_cost_per_token":7.5e-6},
                "xai/grok-code-fast": {"litellm_provider":"xai","input_cost_per_token":1e-6,"output_cost_per_token":2e-6}
            }"#
        ));
        Ok(())
    });
    for model in [
        "mistral/mistral-small-5",
        "mistral/mistral-medium-4",
        "xai/grok-code-fast-2",
    ] {
        assert_eq!(pricing.find(model), None, "{model}");
    }
    let listed = pricing.find("mistral/mistral-small-4").unwrap();
    assert_eq!((listed.input, listed.output), (0.15e-6, 0.6e-6));
}

#[test]
fn an_unlisted_id_whose_only_matches_are_longer_ids_is_unpriced() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in [
        "gpt-6.2",
        "openai/gpt-6.2",
        "gpt-6",
        "o4",
        "kimi-k2.8",
        "deepseek-v4.1",
        "qwen2.5-coder-7b",
        "opus",
    ] {
        assert_eq!(pricing.find(model), None, "{model}");
    }
}

#[test]
fn only_the_logged_id_may_add_a_price_neutral_suffix() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "sample-1-20260101": {"input_cost_per_token": 1, "output_cost_per_token": 1},
                "sample-2[1m]": {"input_cost_per_token": 2, "output_cost_per_token": 2},
                "sample-3-high": {"input_cost_per_token": 3, "output_cost_per_token": 3},
                "host/sample-4": {"input_cost_per_token": 4, "output_cost_per_token": 4},
                "sample-5": {"input_cost_per_token": 5, "output_cost_per_token": 5}
            }"#,
    );
    for model in ["sample-1", "sample-2", "sample-3", "other/sample-3"] {
        assert_eq!(pricing.find(model), None, "{model}");
    }
    for (model, input) in [
        ("sample-1-20260101", 1.0),
        ("sample-4", 4.0),
        ("sample-5-20260101", 5.0),
        ("sample-5[1m]", 5.0),
        ("host/sample-5-high", 5.0),
    ] {
        let found = pricing.find(model).unwrap_or_else(|| panic!("{model}"));
        assert_eq!(found.input, input, "{model}");
    }
}

#[test]
fn price_neutral_suffixes_keep_the_listed_rate() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, listed) in [
        ("o4-mini-high", "o4-mini"),
        ("gpt-5.4-xhigh", "gpt-5.4"),
        ("gpt-5.5-2026-11-01", "gpt-5.5"),
        ("claude-opus-4-8[1m]", "claude-opus-4-8"),
        ("claude-opus-4-8-20261101-v1:0", "claude-opus-4-8"),
        ("vertex_ai/claude-opus-4-7@default", "claude-opus-4-7"),
        ("grok-4.5-latest", "grok-4.5"),
        ("gemini-3.5-flash-extra-low", "gemini-3.5-flash"),
        ("gemini-3.5-flash-extra-high", "gemini-3.5-flash"),
        ("host/gpt-5.4-high", "gpt-5.4"),
        ("host.gpt-5-4-high", "gpt-5.4"),
    ] {
        let found = pricing.find(model).unwrap_or_else(|| panic!("{model}"));
        let expected = pricing.find(listed).unwrap();
        assert_eq!(
            (found.input, found.output),
            (expected.input, expected.output),
            "{model}"
        );
    }
}

#[test]
fn a_thinking_or_reasoning_run_bills_at_the_listed_rate_for_vendors_that_bill_it_as_output() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, listed) in [
        ("claude-sonnet-5-5-thinking", "claude-sonnet-5-5"),
        ("claude-sonnet-5-5:thinking-high", "claude-sonnet-5-5"),
        ("gemini-4-argon-thinking", "gemini-4-argon"),
        ("gemini-4-argon-thinking-low", "gemini-4-argon"),
        ("deepseek-v4.1-flash-thinking", "deepseek-v4.1-flash"),
        ("zai/glm-5.3:thinking", "zai/glm-5.3"),
        ("grok-4.7-reasoning", "grok-4.7"),
        ("grok-4-7-non-reasoning", "grok-4.7"),
        ("qwen4-max-thinking", "qwen4-max"),
        ("alibaba/qwen3.8-max:thinking", "alibaba/qwen3.8-max"),
        ("qwen3-8-max-thinking", "alibaba/qwen3.8-max"),
        ("qwen3.5-plus-thinking", "qwen3.5-plus"),
        ("kimi-k3-thinking", "kimi-k3"),
        ("moonshot/kimi-k2.6-thinking", "moonshot/kimi-k2.6"),
        (
            "mistral/mistral-large-4-thinking",
            "mistral/mistral-large-4",
        ),
        ("mistral-medium-3.5-thinking", "mistral-medium-3.5"),
    ] {
        let found = pricing.find(model).unwrap_or_else(|| panic!("{model}"));
        let expected = pricing.find(listed).unwrap();
        assert_eq!(
            (found.input, found.output),
            (expected.input, expected.output),
            "{model}"
        );
    }
}

#[test]
fn a_qwen_thinking_run_bills_at_the_thinking_rate_where_alibaba_lists_one() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, input, output) in [
        ("qwen-plus-thinking", 0.4e-6, 4e-6),
        ("dashscope/qwen-plus-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-latest-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-2025-12-01-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-2025-09-11-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-2025-07-28-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-2025-07-14-thinking", 0.4e-6, 4e-6),
        ("qwen-plus-2025-04-28-thinking", 0.4e-6, 4e-6),
        ("qwen-turbo-thinking", 0.05e-6, 0.5e-6),
        ("qwen3-30b-a3b-thinking", 0.2e-6, 2.4e-6),
        ("qwen3-14b-thinking", 0.35e-6, 4.2e-6),
        ("qwen3-8b-thinking", 0.18e-6, 2.1e-6),
    ] {
        let found = pricing.find(model).unwrap_or_else(|| panic!("{model}"));
        assert_eq!((found.input, found.output), (input, output), "{model}");
    }
}

#[test]
fn a_qwen_plus_thinking_cache_hit_bills_at_the_implicit_cache_rate() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let found = pricing.find("qwen-plus-thinking").unwrap();
    assert_eq!(
        (found.cache_create, found.cache_read),
        (Some(0.5e-6), Some(0.08e-6))
    );
}

#[test]
fn a_thinking_variant_without_a_documented_billing_rule_stays_unpriced() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in [
        "qwen-plus-2025-01-25-thinking",
        "gemma-4-31b-it-thinking",
        "mistral-large-4-reasoning",
        "mistral.devstral-2-123b-thinking",
        "claude-sonnet-5-5-reasoning",
        "grok-4.7-thinking",
    ] {
        assert!(pricing.find(model).is_none(), "{model}");
    }
}
