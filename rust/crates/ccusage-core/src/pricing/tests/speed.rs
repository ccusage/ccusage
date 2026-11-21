use super::{MatchKind, PricingMap};

#[test]
fn embedded_pricing_includes_codex_priority_multiplier() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert_eq!(pricing.find("gpt-5.6-sol").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.6-terra").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.6-luna").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
    assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-6-astra").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-6-astra").unwrap().flex_multiplier, 0.5);
}

#[test]
fn embedded_pricing_includes_all_openai_flex_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    for model in [
        "gpt-6-astra",
        "gpt-6.1-sol",
        "gpt-6.1-terra",
        "gpt-6-luna",
        "gpt-6-sol",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "gpt-5.5-pro",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.4-pro",
        "gpt-5.2",
        "gpt-5.1",
        "gpt-5",
        "gpt-5-mini",
        "gpt-5-nano",
        "o3",
        "o4-mini",
    ] {
        assert_eq!(pricing.find(model).unwrap().flex_multiplier, 0.5, "{model}");
    }
}

#[test]
fn dated_openai_models_inherit_published_fast_multipliers() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();

    for (model, expected) in [
        ("gpt-6-astra-2026-09-24", 2.0),
        ("gpt-6.1-terra", 2.0),
        ("gpt-5.4-2026-03-05", 2.0),
        ("openai/gpt-5.6-sol-20261001", 2.0),
        ("gpt-5.6-terra-2026-10-01", 2.0),
        ("gpt-5.6-luna-2026-10-01", 2.0),
        ("global.openai.gpt-5.5-2026-04-23", 2.5),
        ("gpt-5.2-codex-2026-01-14", 1.0),
        ("gpt-5.4-unsupported", 1.0),
    ] {
        let json = serde_json::json!({
            model: {
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000002,
            },
        })
        .to_string();
        assert_eq!(pricing.load_json(&json), 1);
        assert_eq!(
            pricing
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .fast_multiplier,
            expected,
            "{model}"
        );
    }
}

#[test]
fn resolves_ultrafast_multiplier_with_the_standard_pricing_entry() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    for (model, expected) in [
        ("gpt-6.1-sol", 6.0),
        ("gpt-6-1-sol", 6.0),
        ("openai/gpt-6-1-sol", 6.0),
        ("openai@gpt-6-1-sol", 6.0),
        ("gpt-6-1-sol-2026-09-24", 6.0),
        ("gpt-6-astra", 6.0),
        ("gpt-6-1-terra", 1.0),
        ("gpt-6-sol", 1.0),
    ] {
        assert_eq!(
            pricing.find(model).unwrap().ultrafast_multiplier,
            expected,
            "{model}"
        );
    }
}

#[test]
fn resolves_ultrafast_multiplier_through_model_aliases() {
    crate::test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("private-sol", "gpt-6.1-sol"),
        ("gpt-6.1-terra", "gpt-6.1-sol"),
    ]);
    let mut pricing = PricingMap::default();
    let json = serde_json::json!({
        "gpt-6.1-sol": {
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.00001,
        },
        "gpt-6.1-terra": {
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000005,
        },
    })
    .to_string();
    assert_eq!(pricing.load_json(&json), 2);

    let private_sol = pricing.find("private-sol").unwrap();
    assert_eq!(private_sol.ultrafast_multiplier, 6.0);
    assert_eq!(private_sol.input, 2e-6);

    let terra = pricing.find("gpt-6.1-terra").unwrap();
    assert_eq!(terra.ultrafast_multiplier, 6.0);
    assert_eq!(terra.input, 2e-6);
}

#[test]
fn fills_flex_multiplier_only_for_supported_openai_model_names() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();

    for (model, expected) in [
        ("gpt-6-astra-2026-09-24", 0.5),
        ("gpt-6.1-sol-20260924", 0.5),
        ("gpt-6.1-terra", 0.5),
        ("openai/gpt-5.5", 0.5),
        ("openrouter/openai/gpt-5.4-2026-03-05", 0.5),
        ("azure/gpt-5.2", 0.5),
        ("openai/gpt-5-mini-2025-08-07", 0.5),
        ("o3-2025-04-16", 0.5),
        ("openai/o4-mini-2025-04-16", 0.5),
        ("gpt-5.6", 0.5),
        ("openai/gpt-reserve", 0.5),
        ("gpt-4.1", 1.0),
        ("gpt-5-pro", 1.0),
        ("gpt-5.2-pro", 1.0),
        ("gpt-5.1-codex", 1.0),
        ("gpt-5.2-codex", 1.0),
        ("gpt-5.3-codex", 1.0),
        ("gpt-5.4-mini-extra", 1.0),
        ("gpt-6-astra-unsupported", 1.0),
        ("o3-mini", 1.0),
        ("o3-pro", 1.0),
        ("openai/o4-mini-deep-research", 1.0),
        ("gpt-unknown", 1.0),
    ] {
        let json = serde_json::json!({
            model: {
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000002,
            },
        })
        .to_string();
        assert_eq!(pricing.load_json(&json), 1);
        assert_eq!(
            pricing
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .unwrap()
                .flex_multiplier,
            expected,
            "{model}"
        );
    }
}

#[test]
fn embedded_pricing_includes_claude_fast_multiplier_for_provider_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert_eq!(
        pricing
            .find("anthropic.claude-opus-4-6-v1")
            .unwrap()
            .fast_multiplier,
        6.0
    );
    assert_eq!(
        pricing
            .find("anthropic.claude-opus-4-7")
            .unwrap()
            .fast_multiplier,
        6.0
    );
    assert_eq!(
        pricing
            .find("anthropic.claude-opus-4-8")
            .unwrap()
            .fast_multiplier,
        2.0
    );
}

#[test]
fn preserves_a_fast_model_name_without_a_standard_sibling() {
    crate::test_support::install_pricing_snapshots();
    let entry = PricingMap::load_embedded()
        .find("qwen2.5-coder-7b-fast")
        .unwrap();
    assert_eq!((entry.input, entry.output), (0.00000003, 0.00000009));
}

#[test]
fn fills_codex_fast_multiplier_when_litellm_pricing_omits_it() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.5": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000030,
                    "cache_read_input_token_cost": 0.0000005
                },
                "gpt-5.4": {
                    "input_cost_per_token": 0.0000025,
                    "output_cost_per_token": 0.000015,
                    "cache_read_input_token_cost": 0.00000025
                },
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                },
                "gpt-5.2-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                },
                "gpt-6-astra": {
                    "input_cost_per_token": 0.000010,
                    "output_cost_per_token": 0.000050,
                    "cache_read_input_token_cost": 0.000001
                }
            }"#,
    );

    assert_eq!(pricing.find("gpt-5.5").unwrap().fast_multiplier, 2.5);
    assert_eq!(pricing.find("gpt-5.4").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.3-codex").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-5.2-codex").unwrap().fast_multiplier, 1.0);
    assert_eq!(pricing.find("gpt-6-astra").unwrap().fast_multiplier, 2.0);
    assert_eq!(pricing.find("gpt-6-astra").unwrap().flex_multiplier, 0.5);
}

#[test]
fn fills_claude_fast_multiplier_when_litellm_pricing_omits_it() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "vertex_ai/claude-opus-4-7@default": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "openrouter/anthropic/claude-opus-4.7": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4.7-20260416": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4.8-20260528": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "claude-opus-4-70": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                },
                "vertex_ai/xclaude-opus-4-7@default": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.000025
                }
            }"#,
    );

    assert_eq!(
        pricing
            .find("vertex_ai/claude-opus-4-7@default")
            .unwrap()
            .fast_multiplier,
        6.0
    );
    assert_eq!(
        pricing
            .find("openrouter/anthropic/claude-opus-4.7")
            .unwrap()
            .fast_multiplier,
        6.0
    );
    assert_eq!(
        pricing
            .find("claude-opus-4.7-20260416")
            .unwrap()
            .fast_multiplier,
        6.0
    );
    assert_eq!(
        pricing
            .find("claude-opus-4.8-20260528")
            .unwrap()
            .fast_multiplier,
        2.0
    );
    assert_eq!(
        pricing.find("claude-opus-4-70").unwrap().fast_multiplier,
        1.0
    );
    assert_eq!(
        pricing
            .find("vertex_ai/xclaude-opus-4-7@default")
            .unwrap()
            .fast_multiplier,
        1.0
    );
}
