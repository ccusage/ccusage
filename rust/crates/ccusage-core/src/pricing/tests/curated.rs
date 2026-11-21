use super::{AtomicUsize, MatchKind, Ordering, PricingMap, build_time_pricing_json, layered};

#[test]
fn direct_providers_cover_every_curated_vendor() {
    crate::test_support::install_pricing_snapshots();
    let providers = &super::super::embedded::litellm_sources()
        .unwrap()
        .direct_providers;
    for model in super::super::BUILTIN_PRICING {
        assert!(
            providers.iter().any(|provider| provider == model.provider),
            "{}",
            model.provider
        );
    }
}

#[test]
fn gpt_vendor_defaults_do_not_depend_on_the_serving_host() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    for host in ["azure_ai", "github_copilot", "bedrock_mantle", "databricks"] {
        let model = format!("{host}/gpt-5.4");
        pricing.load_json(&format!(
            r#"{{"{model}": {{"input_cost_per_token": 0.0000025, "output_cost_per_token": 0.000015, "litellm_provider": "{host}"}}}}"#
        ));
        let price = pricing
            .resolve_at(None, &[(&model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(price.cache_creation_rate(), 2.5e-6, "{host}");
    }
}

#[test]
fn every_curated_row_fills_only_the_rates_a_loaded_row_leaves_out() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "glm-5": {"input_cost_per_token": 9, "output_cost_per_token": 10},
            "zai/glm-4.5": {"input_cost_per_token": 9, "output_cost_per_token": 10, "cache_read_input_token_cost": 7}
        }"#,
    );
    pricing.put_builtin_pricing();
    let glm_5 = pricing
        .resolve_at(None, &[("glm-5", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!((glm_5.input, glm_5.output), (9.0, 10.0));
    assert_eq!(
        (glm_5.cache_create, glm_5.cache_read),
        (Some(0.0), Some(0.2e-6))
    );
    let glm_45 = pricing
        .resolve_at(None, &[("zai/glm-4.5", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(
        (glm_45.cache_create, glm_45.cache_read),
        (Some(0.0), Some(7.0))
    );
}

#[test]
fn curated_context_limits_fill_litellm_gaps_ahead_of_models_dev() {
    crate::test_support::install_pricing_snapshots();
    let mut live = PricingMap::default();
    live.insert_test_limit("gpt-5.6-sol", 654_321);
    let lookups = AtomicUsize::new(0);
    let fetch = || {
        lookups.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    let curated = layered(&PricingMap::default(), None);
    assert_eq!(
        curated.context_limit_with_live("gpt-5.6-sol", fetch),
        Some(1_050_000)
    );
    assert_eq!(lookups.load(Ordering::Relaxed), 0);

    let mut litellm = PricingMap::default();
    litellm.load_json(
        r#"{"gpt-5.6-sol": {"input_cost_per_token": 1, "output_cost_per_token": 2, "max_input_tokens": 922000}}"#,
    );
    assert_eq!(
        layered(&litellm, None).context_limit_with_live("gpt-5.6-sol", fetch),
        Some(922_000)
    );
}

#[test]
fn curated_rows_use_the_shared_speed_policy_when_the_primary_entry_is_missing() {
    let mut pricing = PricingMap::default();
    pricing.put_builtin_pricing();
    assert_eq!(
        pricing
            .resolve_at(None, &[("gpt-5.1", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .flex_multiplier,
        0.5
    );
}

#[test]
fn litellm_gpt_rows_without_a_cache_write_rate_bill_it_at_the_input_rate() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, input, read) in [
        ("gpt-5.1-codex", 1.25e-6, 0.125e-6),
        ("gpt-5.2-codex", 1.75e-6, 0.175e-6),
        ("gpt-5.2", 1.75e-6, 0.175e-6),
    ] {
        let entry = pricing
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(entry.cache_create, None, "{model}");
        assert_eq!(entry.cache_creation_rate(), input, "{model}");
        assert_eq!(entry.cache_read, Some(read), "{model}");
    }
}

#[test]
fn supplements_preserve_all_published_prices_and_context_metadata() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"gpt-5.2-codex": {
            "input_cost_per_token": 10,
            "output_cost_per_token": 20,
            "cache_creation_input_token_cost": 30,
            "cache_read_input_token_cost": 40,
            "max_input_tokens": 123,
            "provider_specific_entry": {"fast": 3, "flex": 0.25}
        }}"#,
    );
    pricing.put_builtin_pricing();
    let entry = pricing.find("gpt-5.2-codex").unwrap();
    assert_eq!(entry.input, 10.0);
    assert_eq!(entry.output, 20.0);
    assert_eq!(entry.cache_create, Some(30.0));
    assert_eq!(entry.cache_read, Some(40.0));
    assert_eq!(entry.fast_multiplier, 3.0);
    assert_eq!(entry.flex_multiplier, 0.25);
    assert_eq!(pricing.context_limit("gpt-5.2-codex"), Some(123));
}

#[test]
fn provider_prefixed_ids_are_billed_at_the_vendor_list_price_offline() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in [
        "anthropic/claude-sonnet-4",
        "openrouter/anthropic/claude-sonnet-4:beta",
    ] {
        let entry = pricing.find(model).unwrap();
        assert_eq!(entry.input, 3e-6, "{model}");
        assert_eq!(entry.output, 15e-6, "{model}");
        assert_eq!(entry.cache_create, Some(3.75e-6), "{model}");
        assert_eq!(entry.cache_read, Some(0.3e-6), "{model}");
    }
}

#[test]
fn online_and_offline_apply_the_same_cache_supplements() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(build_time_pricing_json()));
        Ok(())
    });
    for (model, expected_write) in [("zai/glm-4.5", 0.0), ("gpt-5.2-codex", 1.75e-6)] {
        let live = online.find(model).unwrap();
        let embedded = offline.find(model).unwrap();
        assert_eq!(live.cache_creation_rate(), expected_write, "{model}");
        assert_eq!(
            live.cache_creation_rate(),
            embedded.cache_creation_rate(),
            "{model}"
        );
        assert_eq!(live.cache_read, embedded.cache_read, "{model}");
    }
}

#[test]
fn cache_read_fallback_is_independent_of_cache_write_family() {
    crate::test_support::install_pricing_snapshots();
    for model in ["gpt-5.3-codex-xhigh", "auto/openai/gpt-oss-120b"] {
        let pricing = PricingMap::load_embedded()
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(pricing.cache_read, None, "{model}");
        assert_eq!(
            pricing.cache_read_rate_at(pricing.input),
            pricing.input * 0.1,
            "{model}"
        );
        assert_eq!(pricing.cache_creation_rate(), pricing.input, "{model}");
    }
}

#[test]
fn model_vendor_precedence_is_independent_of_segment_order() {
    use super::super::{data, row::cache_defaults};
    for model in [
        "claude-proxy/openai.gpt-5",
        "openai.gpt-5/claude-proxy",
        "claude-proxy.openai.gpt-5",
    ] {
        assert_eq!(
            cache_defaults(model, Some("anthropic")),
            &data::WRITES_AT_INPUT_RATE
        );
    }
    assert_eq!(
        cache_defaults("proxy/anthropic.claude-sonnet-4", Some("openai")),
        &data::WRITES_SURCHARGED
    );
    assert_eq!(
        cache_defaults("proxy/openai-claude-sonnet", None),
        &data::WRITES_SURCHARGED
    );
    for model in [
        "claude-proxy/openai.gpt-6.2-astra",
        "openai.gpt-6.2-astra/claude-proxy",
    ] {
        assert_eq!(
            cache_defaults(model, None),
            &data::OPENAI_GPT_6_REDUCED_READ,
            "{model}"
        );
    }
}

#[test]
fn hosted_openai_model_names_keep_openai_cache_write_defaults() {
    crate::test_support::install_pricing_snapshots();
    for (model, provider) in [
        ("bedrock_mantle/openai.gpt-5.4", "bedrock_mantle"),
        ("global.openai.gpt-5.4", "bedrock_converse"),
        ("us.openai.gpt-5.5", "bedrock_converse"),
        ("oci/openai.gpt-5", "oci"),
        ("snowflake/openai-gpt-5", "snowflake"),
        ("azure_ai/codex-mini", "azure_ai"),
    ] {
        let mut map = PricingMap::default();
        map.load_json(
            &serde_json::json!({ model: {
                "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6,
                "litellm_provider": provider,
            }})
            .to_string(),
        );
        let pricing = map
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(pricing.cache_creation_rate(), pricing.input, "{model}");
        assert_eq!(
            pricing.one_hour_cache_creation_rate(),
            pricing.input,
            "{model}"
        );
    }
}

#[test]
fn rows_without_cache_rates_use_the_model_vendors_documented_multipliers() {
    crate::test_support::install_pricing_snapshots();
    let cases = [
        ("claude-sonnet-4-5", [1.25, 2.0, 0.1]),
        ("claude-opus-5-5", [1.25, 2.0, 0.05]),
        ("claude-sonnet-5.5", [1.25, 2.0, 0.05]),
        ("claude-fable-5-2", [1.25, 2.0, 0.025]),
        ("claude-mythos-5.1", [1.25, 2.0, 0.025]),
        ("claude-fable-5", [1.25, 2.0, 0.1]),
        ("claude-mythos-5", [1.25, 2.0, 0.1]),
        ("gpt-5.5", [1.0, 1.0, 0.1]),
        ("gpt-5.6-sol", [1.25, 2.5, 0.1]),
        ("openai/gpt-6-astra", [1.25, 2.5, 0.1]),
        ("gpt-6.1-sol", [1.25, 2.5, 0.05]),
        ("global.openai.gpt-6.2-astra", [1.25, 2.5, 0.05]),
        ("gemini-3-flash", [1.0, 1.0, 0.1]),
        ("gemini/gemini-4-carbon", [1.0, 1.0, 0.05]),
        ("deepseek-v4-flash", [1.0, 1.0, 0.02]),
        ("xai/grok-4.7", [1.0, 1.0, 0.25]),
        ("grok-4.5-build", [1.0, 1.0, 0.15]),
        ("moonshotai/kimi-k3", [1.0, 2.0, 0.1]),
        ("zai/glm-5.3", [1.0, 1.0, 0.2]),
        ("qwen3-coder-plus", [1.0, 1.0, 0.2]),
        ("minimax/MiniMax-M3", [1.0, 1.0, 0.2]),
        ("mistral/devstral-medium-2", [1.0, 1.0, 0.1]),
        ("bedrock/amazon.nova-pro-v1:0", [1.0, 1.0, 0.25]),
        ("unidentified-model", [1.25, 2.0, 0.1]),
    ];
    let rows: serde_json::Map<String, serde_json::Value> = cases
        .iter()
        .map(|(model, _)| {
            (
                (*model).to_string(),
                serde_json::json!({"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6}),
            )
        })
        .collect();
    let mut pricing = PricingMap::default();
    pricing.load_json(&serde_json::Value::Object(rows).to_string());

    for (model, [write, write_1h, read]) in cases {
        let rates = pricing
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        let actual = [
            rates.cache_creation_rate(),
            rates.one_hour_cache_creation_rate(),
            rates.cache_read_rate_at(rates.input),
        ]
        .map(|rate| rate / rates.input);
        for (actual, expected) in actual.into_iter().zip([write, write_1h, read]) {
            assert!(
                (actual - expected).abs() < 1e-12,
                "{model}: {actual} != {expected}"
            );
        }
    }
}

#[test]
fn disputed_list_prices_bill_the_vendor_rate_online_and_offline() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(
            r#"{
                "vertex_ai/gemini-2.0-flash": {"input_cost_per_token": 1.5e-7, "output_cost_per_token": 6e-7, "litellm_provider": "vertex_ai"},
                "gemini-2.0-flash": {"input_cost_per_token": 1.5e-7, "output_cost_per_token": 6e-7, "litellm_provider": "vertex_ai-language-models"},
                "deepseek/deepseek-v3.2": {"input_cost_per_token": 2.8e-7, "output_cost_per_token": 4e-7, "cache_read_input_token_cost": 2.8e-8, "litellm_provider": "deepseek"},
                "deepseek/deepseek-chat": {"input_cost_per_token": 2.8e-7, "output_cost_per_token": 4.2e-7, "cache_creation_input_token_cost": 0, "cache_read_input_token_cost": 2.8e-8, "litellm_provider": "deepseek"}
            }"#
        ));
        Ok(())
    });
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for (model, [input, output, write, read]) in [
            ("kimi-k2-thinking", [0.6e-6, 2.5e-6, 0.6e-6, 0.15e-6]),
            (
                "moonshot/kimi-k2-thinking",
                [0.6e-6, 2.5e-6, 0.6e-6, 0.15e-6],
            ),
            ("kimi-k2-turbo-preview", [1.15e-6, 8e-6, 1.15e-6, 0.15e-6]),
            ("deepseek-v3.2", [0.28e-6, 0.42e-6, 0.28e-6, 0.028e-6]),
            ("gemini-2.0-flash", [0.1e-6, 0.4e-6, 0.1e-6, 0.025e-6]),
        ] {
            let rates = pricing
                .resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], None)
                .map(|(_, rates)| rates)
                .unwrap();
            assert_eq!(
                [
                    rates.input,
                    rates.output,
                    rates.cache_creation_rate(),
                    rates.cache_read_rate_at(rates.input),
                ],
                [input, output, write, read],
                "{mode} {model}"
            );
        }
    }
    let deepseek_chat = online.find("deepseek-chat").unwrap();
    assert_eq!(
        deepseek_chat.cache_creation_rate(),
        0.0,
        "a published zero write rate stays"
    );
}

#[test]
fn curated_rows_keep_the_write_rate_their_loaded_row_publishes() {
    crate::test_support::install_pricing_snapshots();
    for (key, model, provider, input, output, published, expected) in [
        (
            "deepseek/deepseek-v3.2",
            "deepseek-v3.2",
            "deepseek",
            "2.8e-7",
            0.42e-6,
            "0",
            0.0,
        ),
        (
            "deepseek/deepseek-v3.2",
            "deepseek-v3.2",
            "deepseek",
            "2.8e-7",
            0.42e-6,
            "5e-7",
            0.5e-6,
        ),
        (
            "moonshot/kimi-k2-thinking",
            "kimi-k2-thinking",
            "moonshot",
            "6e-7",
            2.5e-6,
            "0",
            0.0,
        ),
        (
            "moonshot/kimi-k2-thinking",
            "kimi-k2-thinking",
            "moonshot",
            "6e-7",
            2.5e-6,
            "7e-7",
            0.7e-6,
        ),
    ] {
        let mut pricing = PricingMap::load_embedded();
        pricing.refresh_litellm(|load| {
            assert!(load(&format!(
                r#"{{"{key}": {{"input_cost_per_token": {input}, "output_cost_per_token": 4e-7, "cache_creation_input_token_cost": {published}, "litellm_provider": "{provider}"}}}}"#
            )));
            Ok(())
        });
        let rates = pricing.find(model).unwrap();
        assert_eq!(
            (rates.output, rates.cache_creation_rate()),
            (output, expected),
            "{key} {published}"
        );
    }
}
