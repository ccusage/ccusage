use super::{
    MatchKind, PricingMap, build_time_models_dev_json, build_time_pricing_json,
    embedded_models_dev_pricing, layered,
};
use crate::pricing::PricingRows;

fn sorted_rows(rows: &PricingRows) -> Vec<String> {
    let mut rows: Vec<_> = rows
        .entries
        .rows
        .iter()
        .map(|row| {
            format!(
                "{} {:?} {:?} {:?} {:?} {} {} {:?} {:?}",
                row.key,
                row.rates(),
                row.source,
                row.schedule,
                row.context_limit,
                row.exact_only,
                row.whole_request_tiers,
                row.scope,
                row.vendor_start
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn embedded_table_matches_the_rows_loaded_from_its_snapshots() {
    crate::test_support::install_pricing_snapshots();
    let mut models_dev = PricingMap::default();
    models_dev
        .load_models_dev_json_missing(build_time_models_dev_json())
        .unwrap();
    let mut litellm = PricingMap::default();
    litellm.load_json(build_time_pricing_json());
    litellm.complete_layers(&models_dev);

    let embedded = PricingMap::load_embedded();

    assert_eq!(sorted_rows(&embedded.rows), sorted_rows(&litellm.rows));
    assert_eq!(
        sorted_rows(&embedded_models_dev_pricing().rows),
        sorted_rows(&models_dev.rows)
    );
}

#[test]
fn loads_embedded_claude_pricing() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    assert!(pricing.len() > 0);
    assert!(pricing.find("claude-sonnet-4-20250514").is_some());
}

#[test]
fn embedded_pricing_includes_hermes_frontier_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert!(pricing.find("gpt-5.5").is_some());
    assert!(pricing.find("grok-4.3").is_some());
    assert_eq!(pricing.context_limit("grok-4.3"), Some(1_000_000));
}

#[test]
fn embedded_pricing_includes_moonshot_kimi_for_offline_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let kimi_k25 = pricing.find("moonshot/kimi-k2.5").unwrap();
    let kimi_k26 = pricing.find("moonshot/kimi-k2.6").unwrap();

    assert_eq!(kimi_k25.input, 0.6e-6);
    assert_eq!(kimi_k25.output, 3e-6);
    assert_eq!(kimi_k25.cache_read, Some(0.1e-6));
    assert_eq!(kimi_k26.input, 0.95e-6);
    assert_eq!(kimi_k26.output, 4e-6);
    assert_eq!(kimi_k26.cache_read, Some(0.16e-6));
    assert_eq!(pricing.context_limit("moonshot/kimi-k2.5"), Some(262_144));
    assert_eq!(pricing.context_limit("moonshot/kimi-k2.6"), Some(262_144));
}

#[test]
fn offline_prices_models_outside_the_claude_and_moonshot_families() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let grok_build = pricing
        .find("grok-build-0.1")
        .expect("embedded models.dev should include xAI pricing");

    assert!((grok_build.input * 1e6 - 1.0).abs() < 1e-9);
    assert!((grok_build.output * 1e6 - 2.0).abs() < 1e-9);
    assert_eq!(pricing.context_limit("grok-build-0.1"), Some(256_000));
}

#[test]
fn embedded_pricing_includes_z_ai_glm_models_for_offline_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let glm_51 = pricing.find("glm-5.1").unwrap();
    assert_eq!(glm_51.input, 1.4e-6);
    assert_eq!(glm_51.output, 4.4e-6);
    assert_eq!(glm_51.cache_create, Some(0.0));
    assert_eq!(glm_51.cache_read, Some(0.26e-6));

    let glm_5 = pricing.find("glm-5").unwrap();
    assert_eq!(glm_5.input, 1.0e-6);
    assert_eq!(glm_5.output, 3.2e-6);
    assert_eq!(glm_5.cache_create, Some(0.0));
    assert_eq!(glm_5.cache_read, Some(0.2e-6));
    assert_eq!(pricing.context_limit("zai/glm-5"), Some(200_000));

    let glm_5_turbo = pricing.find("glm-5-turbo").unwrap();
    assert_eq!(glm_5_turbo.input, 1.2e-6);
    assert_eq!(glm_5_turbo.output, 4.0e-6);
    assert_eq!(glm_5_turbo.cache_create, Some(0.0));
    assert_eq!(glm_5_turbo.cache_read, Some(0.24e-6));

    let glm_47 = pricing.find("glm-4.7").unwrap();
    assert_eq!(glm_47.input, 0.6e-6);
    assert_eq!(glm_47.output, 2.2e-6);
    assert_eq!(glm_47.cache_create, Some(0.0));
    assert_eq!(glm_47.cache_read, Some(0.11e-6));

    let glm_46 = pricing.find("glm-4.6").unwrap();
    assert_eq!(glm_46.input, 0.6e-6);
    assert_eq!(glm_46.output, 2.2e-6);
    assert_eq!(glm_46.cache_create, Some(0.0));
    assert_eq!(glm_46.cache_read, Some(0.11e-6));

    let glm_45 = pricing.find("glm-4.5").unwrap();
    assert_eq!(glm_45.input, 0.6e-6);
    assert_eq!(glm_45.output, 2.2e-6);
    assert_eq!(glm_45.cache_create, Some(0.0));
    assert_eq!(glm_45.cache_read, Some(0.11e-6));

    let zai_glm_45 = pricing.find("zai/glm-4.5").unwrap();
    assert_eq!(zai_glm_45.input, 0.6e-6);
    assert_eq!(zai_glm_45.output, 2.2e-6);
    assert_eq!(zai_glm_45.cache_create, Some(0.0));
    assert_eq!(zai_glm_45.cache_read, Some(0.11e-6));
    assert_eq!(pricing.context_limit("zai/glm-4.5"), Some(128_000));
}

#[test]
fn embedded_pricing_resolves_overlapping_model_keys_exactly() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let sonnet_4 = pricing.find("claude-sonnet-4-20250514").unwrap();
    let sonnet_45 = pricing.find("claude-sonnet-4-5-20250929").unwrap();

    assert_eq!(
        pricing.find("claude-sonnet-4-20250514").unwrap().input,
        sonnet_4.input
    );
    assert_eq!(
        pricing.find("claude-sonnet-4-5-20250929").unwrap().input,
        sonnet_45.input,
    );
    assert_eq!(
        pricing
            .find("anthropic.claude-sonnet-4-20250514-v1:0")
            .unwrap()
            .input,
        sonnet_4.input,
    );
    assert_eq!(
        pricing.find("claude-3-5-haiku-20241022").unwrap().input,
        0.8e-6,
    );
}

#[test]
fn embedded_pricing_includes_gpt_5_5_for_offline_codex_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let gpt_55 = pricing.find("gpt-5.5").unwrap();

    assert_eq!(gpt_55.input, 5e-6);
    assert_eq!(gpt_55.output, 30e-6);
    assert_eq!(gpt_55.cache_read, Some(0.5e-6));
    assert_eq!(gpt_55.fast_multiplier, 2.5);
    assert_eq!(pricing.context_limit("gpt-5.5"), Some(1_050_000));
}

#[test]
fn embedded_pricing_preserves_gpt_5_6_generated_sources() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let mut litellm_snapshot = PricingMap::default();
    litellm_snapshot.load_json(build_time_pricing_json());
    let models_dev_snapshot = embedded_models_dev_pricing();

    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let entry = pricing.find(model).unwrap();
        let litellm_source = litellm_snapshot
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates);
        let base_source = litellm_source
            .or_else(|| {
                models_dev_snapshot
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
            })
            .unwrap();
        let tier_source = litellm_source
            .filter(|source| {
                source
                    .long_context
                    .and_then(|context| context.rates.input)
                    .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.output)
                        .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.cache_create)
                        .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.cache_read)
                        .is_some()
            })
            .or_else(|| {
                models_dev_snapshot
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
            })
            .unwrap();
        assert_eq!(entry.input, base_source.input, "{model}");
        assert_eq!(entry.output, base_source.output, "{model}");
        assert_eq!(entry.cache_create, base_source.cache_create, "{model}");
        assert_eq!(entry.cache_read, base_source.cache_read, "{model}");
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.input),
            tier_source
                .long_context
                .and_then(|context| context.rates.input),
            "{model}"
        );
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.output),
            tier_source
                .long_context
                .and_then(|context| context.rates.output),
            "{model}"
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_create),
            tier_source
                .long_context
                .and_then(|context| context.rates.cache_create),
            "{model}"
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_read),
            tier_source
                .long_context
                .and_then(|context| context.rates.cache_read),
            "{model}"
        );
        assert_eq!(
            entry.long_context.map(|context| context.threshold),
            tier_source.long_context.map(|context| context.threshold),
            "{model}"
        );
        assert_eq!(
            pricing.context_limit(model),
            litellm_snapshot
                .rows
                .entries
                .get(model)
                .and_then(|row| row.context_limit.as_ref())
                .copied()
                .or_else(|| models_dev_snapshot
                    .rows
                    .entries
                    .get(model)
                    .and_then(|row| row.context_limit.as_ref())
                    .copied())
                .or_else(|| super::super::BUILTIN_PRICING
                    .iter()
                    .find(|entry| entry.model == model)
                    .and_then(|entry| entry.context_limit)),
            "{model}"
        );
    }
}

#[test]
fn builtin_gpt_5_6_fallbacks_preserve_source_precedence() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004123,
                    "output_cost_per_token": 0.000020123,
                    "cache_creation_input_token_cost": 0.000005123,
                    "cache_read_input_token_cost": 0.0000004123,
                    "max_input_tokens": 654321
                }
            }"#,
    );
    let loaded = pricing
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let loaded_context_limit = pricing.context_limit("gpt-5.6-sol");

    let mut builtin = PricingMap::default();
    builtin.put_builtin_pricing();
    let builtin_rates = builtin
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_ne!(loaded.input, builtin_rates.input);
    assert_ne!(loaded.output, builtin_rates.output);

    pricing.put_builtin_pricing();
    let resolved = pricing
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(resolved.input, loaded.input);
    assert_eq!(resolved.output, loaded.output);
    assert_eq!(resolved.cache_create, loaded.cache_create);
    assert_eq!(resolved.cache_read, loaded.cache_read);
    assert_eq!(resolved.fast_multiplier, loaded.fast_multiplier);
    assert_eq!(pricing.context_limit("gpt-5.6-sol"), loaded_context_limit);

    let mut embedded_models_dev = PricingMap::default();
    embedded_models_dev.insert_test_limit("gpt-5.6-sol", 777_777);
    let mut live_models_dev = PricingMap::default();
    live_models_dev.insert_test_limit("gpt-5.6-sol", 888_888);
    assert_eq!(
        layered(&pricing, Some(&embedded_models_dev))
            .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
        Some(654_321)
    );
    assert_eq!(
        layered(&PricingMap::default(), Some(&embedded_models_dev))
            .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
        Some(1_050_000)
    );
}

#[test]
fn gpt_5_6_alias_resolves_to_sol_when_the_generic_entry_is_missing() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004,
                    "output_cost_per_token": 0.00002,
                    "cache_creation_input_token_cost": 0.000005,
                    "cache_read_input_token_cost": 0.0000004,
                    "input_cost_per_token_above_200k_tokens": 0.000008,
                    "output_cost_per_token_above_200k_tokens": 0.00003,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.00001,
                    "cache_read_input_token_cost_above_200k_tokens": 0.0000008,
                    "max_input_tokens": 1050000
                }
            }"#,
    );
    let alias = pricing.find("gpt-5.6").unwrap();
    let sol = pricing.find("gpt-5.6-sol").unwrap();

    assert_eq!(alias.input, sol.input);
    assert_eq!(alias.output, sol.output);
    assert_eq!(alias.cache_create, sol.cache_create);
    assert_eq!(alias.cache_read, sol.cache_read);
    assert_eq!(
        alias.long_context.and_then(|context| context.rates.input),
        sol.long_context.and_then(|context| context.rates.input)
    );
    assert_eq!(
        alias.long_context.and_then(|context| context.rates.output),
        sol.long_context.and_then(|context| context.rates.output)
    );
    assert_eq!(
        pricing.context_limit("gpt-5.6"),
        pricing.context_limit("gpt-5.6-sol")
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.6")
            .unwrap()
            .long_context_split_threshold(),
        PricingMap::load_embedded()
            .find("gpt-5.6-sol")
            .unwrap()
            .long_context_split_threshold()
    );
}

#[test]
fn embedded_gpt_5_6_exact_entry_wins_over_the_sol_alias() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let exact = pricing
        .resolve_at(None, &[("gpt-5.6", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let resolved = pricing.find("gpt-5.6").unwrap();

    assert_eq!(resolved.input, exact.input);
    assert_eq!(resolved.output, exact.output);
    assert_eq!(resolved.cache_create, exact.cache_create);
    assert_eq!(resolved.cache_read, exact.cache_read);
    assert_eq!(
        resolved
            .long_context
            .and_then(|context| context.rates.input),
        exact.long_context.and_then(|context| context.rates.input)
    );
    assert_eq!(
        resolved
            .long_context
            .and_then(|context| context.rates.output),
        exact.long_context.and_then(|context| context.rates.output)
    );
    assert_eq!(
        resolved.long_context.map(|context| context.threshold),
        exact.long_context.map(|context| context.threshold)
    );
    assert_eq!(
        pricing.context_limit("gpt-5.6"),
        pricing
            .rows
            .entries
            .get("gpt-5.6")
            .and_then(|row| row.context_limit.as_ref())
            .copied()
    );
}

#[test]
fn embedded_pricing_does_not_resolve_undated_codex_auto_review_model() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert!(pricing.find("codex-auto-review").is_none());
    assert!(pricing.context_limit("codex-auto-review").is_none());
}

#[test]
fn embedded_pricing_resolves_codex_spark_short_model_alias() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let short_spark = pricing
        .find("gpt-5.3-spark")
        .expect("gpt-5.3-spark should resolve via model alias");
    let codex_spark = pricing
        .find("gpt-5.3-codex-spark")
        .expect("canonical Codex Spark pricing should exist");

    assert_eq!(short_spark.input, codex_spark.input);
    assert_eq!(short_spark.output, codex_spark.output);
    assert_eq!(short_spark.cache_read, codex_spark.cache_read);
    assert_eq!(short_spark.fast_multiplier, codex_spark.fast_multiplier);
}

#[test]
fn embedded_pricing_resolves_codex_gpt_reserve_to_gpt_5_6_luna() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let reserve = pricing
        .find("gpt-reserve")
        .expect("gpt-reserve should resolve via model alias");
    let luna = pricing
        .find("gpt-5.6-luna")
        .expect("GPT-5.6 Luna pricing should exist");

    assert_eq!(reserve.input, luna.input);
    assert_eq!(reserve.output, luna.output);
    assert_eq!(reserve.cache_read, luna.cache_read);
    assert_eq!(reserve.fast_multiplier, luna.fast_multiplier);
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-reserve")
            .unwrap()
            .long_context_split_threshold(),
        PricingMap::load_embedded()
            .find("gpt-5.6-luna")
            .unwrap()
            .long_context_split_threshold()
    );
}

#[test]
fn embedded_pricing_resolves_opus_47_dot_model_names() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let opus_47 = pricing.find("claude-opus-4-7").unwrap();

    assert_eq!(
        pricing.find("claude-opus-4.7-20260416").unwrap().input,
        opus_47.input
    );
    assert_eq!(
        pricing.context_limit("claude-opus-4.7"),
        pricing.context_limit("claude-opus-4-7")
    );
}

#[test]
fn embedded_pricing_resolves_opus_48_dot_model_names() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let canonical = pricing.find("claude-opus-4-8").unwrap();

    let opus_48 = pricing.find("claude-opus-4.8-20260528").unwrap();
    assert_eq!(opus_48.input, canonical.input);
    assert_eq!(opus_48.output, canonical.output);
    assert_eq!(opus_48.cache_create, canonical.cache_create);
    assert_eq!(opus_48.cache_read, canonical.cache_read);
    assert_eq!(
        pricing.context_limit("claude-opus-4.8"),
        pricing.context_limit("claude-opus-4-8")
    );
}

#[test]
fn embedded_pricing_resolves_separator_aliases_for_other_claude_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let sonnet_46 = pricing.find("claude-sonnet-4-6").unwrap();
    let haiku_45 = pricing.find("claude-haiku-4-5").unwrap();

    assert_eq!(
        pricing.find("claude-sonnet-4.6-20260416").unwrap().input,
        sonnet_46.input
    );
    assert_eq!(
        pricing.find("claude-haiku-4.5").unwrap().input,
        haiku_45.input
    );
    assert_eq!(
        pricing.context_limit("claude-sonnet-4.6"),
        pricing.context_limit("claude-sonnet-4-6")
    );
    assert_eq!(
        pricing.context_limit("claude-haiku-4.5"),
        pricing.context_limit("claude-haiku-4-5")
    );
}

#[test]
fn offline_runs_bill_xai_ids_from_xais_own_rows_like_online_runs() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(build_time_pricing_json()));
        Ok(())
    });
    for (model, [input, output, read, long_input]) in [
        ("grok-code-fast-1", [1e-6, 2e-6, 0.2e-6, 2e-6]),
        ("grok-4.20", [1.25e-6, 2.5e-6, 0.2e-6, 2.5e-6]),
        (
            "grok-4.20-0309-reasoning",
            [1.25e-6, 2.5e-6, 0.2e-6, 2.5e-6],
        ),
        ("grok-4.7", [2e-6, 6e-6, 0.5e-6, 4e-6]),
    ] {
        for (mode, pricing) in [("offline", &offline), ("online", &online)] {
            let (_, rates) = pricing
                .resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], None)
                .unwrap();
            assert_eq!(
                [
                    rates.input,
                    rates.output,
                    rates.cache_read_rate_at(rates.input),
                    rates
                        .long_context
                        .and_then(|context| context.rates.input)
                        .unwrap_or(rates.input),
                ],
                [input, output, read, long_input],
                "{mode} {model}"
            );
            assert_eq!(
                rates.long_context.map(|context| context.mode),
                Some(super::super::LongContextMode::WholeRequest),
                "{mode} {model}"
            );
        }
    }
}

fn xai_cost(
    pricing: &PricingMap,
    model: &str,
    input_tokens: u64,
    timestamp: Option<crate::TimestampMs>,
) -> f64 {
    let candidates = [(model, MatchKind::Fuzzy)];
    crate::cost::price_entry(
        crate::cost::EntryPricing {
            granularity: crate::cost::UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens,
                output_tokens: 1_000,
                ..Default::default()
            },
            reasoning_tokens: 0,
            residual_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp,
        },
        crate::cost::PriceTable::Calculate(pricing),
    )
    .cost
}

#[test]
fn xai_bills_the_long_context_rate_from_a_200k_token_prompt() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (input_tokens, expected) in [
        (199_999, 199_999.0 * 1.25e-6 + 1_000.0 * 2.5e-6),
        (200_000, 200_000.0 * 2.5e-6 + 1_000.0 * 5e-6),
    ] {
        let cost = xai_cost(&pricing, "grok-4.20", input_tokens, None);
        assert!(
            (cost - expected).abs() < 1e-9,
            "{input_tokens}: {cost} != {expected}"
        );
    }
}

#[test]
fn grok_code_fast_1_bills_its_own_rates_until_xai_aliased_it_to_grok_build() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(build_time_pricing_json()));
        Ok(())
    });
    let before = super::timestamp("2026-05-15T14:59:11Z");
    let after = super::timestamp("2026-05-15T14:59:12Z");
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for model in [
            "grok-code-fast-1",
            "grok-code-fast",
            "grok-code-fast-1-0825",
        ] {
            for (instant, input_tokens, expected) in [
                (before, 1_000, 1_000.0 * 0.2e-6 + 1_000.0 * 1.5e-6),
                (before, 250_000, 250_000.0 * 0.2e-6 + 1_000.0 * 1.5e-6),
                (after, 1_000, 1_000.0 * 1e-6 + 1_000.0 * 2e-6),
                (after, 250_000, 250_000.0 * 2e-6 + 1_000.0 * 4e-6),
            ] {
                let cost = xai_cost(pricing, model, input_tokens, Some(instant));
                assert!(
                    (cost - expected).abs() < 1e-9,
                    "{mode} {model} {input_tokens}: {cost} != {expected}"
                );
            }
        }
    }
}
