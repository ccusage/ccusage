use super::{
    AtomicUsize, Lookup, MatchKind, Ordering, PricingMap, build_time_models_dev_json,
    build_time_pricing_json, embedded_models_dev_pricing, fs_fixture, layered, online,
};

#[test]
fn live_catalogue_is_selected_once_after_authoritative_misses() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{"sample": {"input_cost_per_token": 1, "output_cost_per_token": 2}}"#);
    assert_eq!(pricing.find("sample-20260101").unwrap().input, 1.0);
    assert!(pricing.find("other-model").is_none());
    let mut live = PricingMap::default();
    live.load_json(
        r#"{
            "sample-20260101": {"input_cost_per_token": 3, "output_cost_per_token": 4},
            "other-model": {"input_cost_per_token": 5, "output_cost_per_token": 6}
        }"#,
    );
    live.mark_test_exact_only("sample-20260101");
    let pricing = online(&pricing);
    let attempts = AtomicUsize::new(0);
    let fetch = || {
        attempts.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    assert_eq!(
        pricing
            .with_live_layer("sample", super::super::Lookup::Pricing, fetch)
            .resolve_at(None, &[("sample", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .input,
        1.0
    );
    assert_eq!(attempts.load(Ordering::Relaxed), 0);
    let merged = pricing.with_live_layer("trigger", super::super::Lookup::Pricing, fetch);
    assert_eq!(
        merged
            .resolve_at(None, &[("sample-20260101", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .input,
        3.0
    );
    assert_eq!(pricing.find("sample-20260101").unwrap().input, 3.0);
    assert_eq!(pricing.find("other-model").unwrap().input, 5.0);
    pricing.with_live_layer("trigger-again", super::super::Lookup::Pricing, fetch);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn a_map_derived_after_the_live_merge_does_not_merge_it_again() {
    crate::test_support::install_pricing_snapshots();
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"md-only-1": { "cost": { "input": 3000000, "output": 4000000 } }}"#,
    );
    let base = layered(&PricingMap::default(), None);
    assert_eq!(
        base.find_with_live("md-only-1", || Some(&live))
            .unwrap()
            .input,
        3.0
    );
    let derived = base.with_user_overrides(std::iter::empty());
    let lookups = AtomicUsize::new(0);
    let fetch = || {
        lookups.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    assert_eq!(
        derived.find_with_live("md-only-1", fetch).unwrap().input,
        3.0
    );
    assert_eq!(lookups.load(Ordering::Relaxed), 0);
}

#[test]
fn live_models_dev_replaces_the_embedded_snapshot_for_a_models_dev_only_id() {
    crate::test_support::install_pricing_snapshots();
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"md-only-1": { "cost": { "input": 1000000, "output": 2000000 }, "limit": { "context": 1000 } }}"#,
    );
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"md-only-1": { "cost": { "input": 3000000, "output": 4000000 }, "limit": { "context": 3000 } }}"#,
    );
    let pricing = layered(&PricingMap::default(), Some(&embedded));
    let lookups = AtomicUsize::new(0);
    let fetch = || {
        lookups.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    assert_eq!(
        pricing.find_with_live("md-only-1", fetch).unwrap().input,
        3.0
    );
    assert_eq!(
        pricing.context_limit_with_live("md-only-1", fetch),
        Some(3000)
    );
    assert_eq!(lookups.load(Ordering::Relaxed), 1);

    let offline = layered(&PricingMap::default(), Some(&embedded));
    assert_eq!(
        offline.find_with_live("md-only-1", || None).unwrap().input,
        1.0
    );
}

#[test]
fn exact_fallback_uses_live_models_dev_before_the_embedded_snapshot() {
    crate::test_support::install_pricing_snapshots();
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"md-only-1": {"cost": {"input": 1000000, "output": 2000000}}}"#,
    );
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"md-only-1": {"cost": {"input": 3000000, "output": 4000000}}}"#,
    );
    let pricing = layered(&PricingMap::default(), Some(&embedded));
    let lookups = AtomicUsize::new(0);
    let fetch = || {
        lookups.fetch_add(1, Ordering::Relaxed);
        Some(&live)
    };
    for _ in 0..2 {
        assert_eq!(
            pricing
                .with_live_layer("md-only-1", Lookup::Pricing, fetch)
                .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
                .map(|(_, pricing)| pricing)
                .unwrap()
                .input,
            3.0
        );
    }
    assert_eq!(lookups.load(Ordering::Relaxed), 1);
    let offline = layered(&PricingMap::default(), Some(&embedded));
    assert_eq!(
        offline
            .with_live_layer("md-only-1", Lookup::Pricing, || None)
            .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .unwrap()
            .input,
        1.0
    );
}

#[test]
fn models_dev_layers_share_rows_without_copying_them_into_primary() {
    crate::test_support::install_pricing_snapshots();
    let mut embedded = PricingMap::default();
    embedded.load_models_dev_json_for_tests(
        r#"{"md-only-1": {"cost": {"input": 1000000, "output": 2000000}}}"#,
    );
    let pricing = layered(&PricingMap::default(), Some(&embedded));
    assert!(pricing.rows.get("md-only-1").is_none());
    assert!(std::sync::Arc::ptr_eq(
        pricing.models_dev.as_ref().unwrap(),
        &embedded.rows,
    ));
    let mut live = PricingMap::default();
    live.load_models_dev_json_for_tests(
        r#"{"md-only-1": {"cost": {"input": 3000000, "output": 4000000}}}"#,
    );
    let merged = pricing.with_live_layer("md-only-1", Lookup::Pricing, || Some(&live));
    assert!(std::sync::Arc::ptr_eq(&merged.rows, &pricing.rows));
    assert!(std::sync::Arc::ptr_eq(
        merged.live_models_dev.get().unwrap().as_ref().unwrap(),
        &live.rows,
    ));
    assert_eq!(
        merged
            .resolve_at(None, &[("md-only-1", MatchKind::Exact)], None)
            .map(|(_, pricing)| pricing)
            .unwrap()
            .input,
        3.0
    );
}

#[test]
fn accepts_a_fetched_models_dev_body_only_when_it_prices_a_model() {
    crate::test_support::install_pricing_snapshots();
    let models_dev = r#"{"openai":{"models":{"gpt-test":{"cost":{"input":1.0,"output":2.0}}}}}"#;
    let litellm =
        r#"{"gpt-test":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#;
    let free = r#"{"openai":{"models":{"free":{"cost":{"input":0.0,"output":0.0}}}}}"#;
    let asset_priced = r#"{"openai":{"models":{"unpriced":{"modalities":{"input":[],"output":["text"]},"cost":{"input":1.0,"output":2.0}}}}}"#;

    for (body, expected) in [
        (models_dev, true),
        (litellm, false),
        ("{}", false),
        (free, false),
        (asset_priced, false),
    ] {
        let mut accepted = None;
        let pricing = super::super::fetch::load_models_dev_pricing(|load| {
            accepted = Some(load(body));
            Ok(())
        });
        assert_eq!(accepted, Some(expected), "{body}");
        assert_eq!(pricing.is_some(), expected, "{body}");
    }
}

#[test]
fn embedded_models_dev_omits_models_priced_per_asset() {
    crate::test_support::install_pricing_snapshots();
    let embedded = embedded_models_dev_pricing();

    for model in [
        "whisper-large-v3",
        "gemini-2.5-flash-image",
        "gemini-3-pro-image-preview",
    ] {
        assert!(
            embedded
                .resolve_at(None, &[(model, MatchKind::Exact)], None)
                .map(|(_, rates)| rates)
                .is_none(),
            "{model} prices assets rather than text tokens and must stay out of the snapshot"
        );
    }
    assert!(
        embedded
            .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(
        embedded
            .resolve_at(None, &[("gemini-3-flash-preview", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
}

#[test]
fn embedded_models_dev_prices_resold_models_from_their_author() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let kimi_k27_code = pricing.find("kimi-k2.7-code").unwrap();

    assert!((kimi_k27_code.input * 1e6 - 0.95).abs() < 1e-9);
    assert!((kimi_k27_code.output * 1e6 - 4.0).abs() < 1e-9);
}

#[test]
fn offline_prices_kimi_k3_from_embedded_models_dev() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let kimi_k3 = pricing.find("moonshot/kimi-k3").unwrap_or_else(|| {
        pricing
            .find("kimi-k3")
            .expect("embedded models.dev should include kimi-k3 pricing")
    });

    assert_eq!(kimi_k3.input, 3e-6);
    assert_eq!(kimi_k3.output, 15e-6);
    assert_eq!(kimi_k3.cache_read, Some(0.3e-6));
    assert!(
        pricing
            .context_limit("moonshot/kimi-k3")
            .or_else(|| pricing.context_limit("kimi-k3"))
            == Some(1_048_576)
    );
}

#[test]
fn keeps_models_dev_fallback_disabled_for_embedded_and_offline_pricing() {
    crate::test_support::install_pricing_snapshots();
    use ccusage_cli::PricingOverride;
    assert!(!PricingMap::load_embedded().models_dev_fallback_enabled());
    assert!(
        !PricingMap::load_with_overrides(
            true,
            false,
            std::iter::empty::<(&String, &PricingOverride)>(),
        )
        .models_dev_fallback_enabled()
    );
}

#[test]
fn tries_models_dev_pricing_once_per_process() {
    let cell = std::sync::OnceLock::new();
    let attempts = AtomicUsize::new(0);

    let failed = super::super::fetch::load_models_dev_pricing_once(&cell, |_| {
        attempts.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "temporary failure",
        ))
    });
    let later = super::super::fetch::load_models_dev_pricing_once(&cell, |load| {
        attempts.fetch_add(1, Ordering::Relaxed);
        load(r#"{"openai":{"models":{"gpt-late":{"cost":{"input":1.0,"output":2.0}}}}}"#);
        Ok(())
    });

    assert!(failed.is_none());
    assert!(later.is_none());
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn keeps_the_models_dev_pricing_its_one_attempt_loaded() {
    crate::test_support::install_pricing_snapshots();
    let cell = std::sync::OnceLock::new();
    let loaded = super::super::fetch::load_models_dev_pricing_once(&cell, |load| {
        load(
            r#"{
                "openai": {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-first": {
                            "id": "gpt-first",
                            "name": "GPT First",
                            "cost": {
                                "input": 1.0,
                                "output": 2.0
                            },
                            "limit": {
                                "context": 42
                            }
                        }
                    }
                }
            }"#,
        );
        Ok(())
    })
    .expect("a usable models.dev body should load");

    let gpt_first = loaded
        .find_entry("gpt-first", MatchKind::Fuzzy)
        .expect("the loaded body should price its model");
    assert_eq!(gpt_first.input, 0.000001);
    assert_eq!(gpt_first.output, 0.000002);
    assert_eq!(
        loaded.context_limit_entry("gpt-first", MatchKind::Fuzzy),
        Some(42)
    );
    assert!(
        super::super::fetch::load_models_dev_pricing_once(&cell, |_| {
            Err(std::io::Error::other("must not fetch again"))
        })
        .is_some()
    );
}

#[test]
fn loads_missing_models_dev_pricing_without_overriding_litellm() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-primary": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001,
                    "max_input_tokens": 123
                },
                "openrouter/gpt-alias": {
                    "input_cost_per_token": 0.000003,
                    "output_cost_per_token": 0.000030,
                    "max_input_tokens": 321
                }
            }"#,
    );

    let models_dev_json = r#"{
                "openai": {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-primary": {
                            "id": "gpt-primary",
                            "name": "GPT Primary",
                            "cost": {
                                "input": 9.0,
                                "output": 90.0,
                                "cache_read": 0.9,
                                "cache_write": 11.25
                            },
                            "limit": {
                                "context": 999
                            }
                        },
                        "gpt-fallback": {
                            "id": "gpt-fallback",
                            "name": "GPT Fallback",
                            "cost": {
                                "input": 2.0,
                                "output": 8.0,
                                "cache_read": 0.2,
                                "cache_write": 2.5
                            },
                            "limit": {
                                "context": 456
                            }
                        },
                        "gpt-alias": {
                            "id": "gpt-alias",
                            "name": "GPT Alias",
                            "cost": {
                                "input": 4.0,
                                "output": 16.0
                            },
                            "limit": {
                                "context": 654
                            }
                        }
                    }
                }
            }"#;

    assert_eq!(
        pricing.load_models_dev_json_missing(models_dev_json),
        Some(2)
    );

    let primary = pricing.find("gpt-primary").unwrap();
    let fallback = pricing.find("gpt-fallback").unwrap();
    let alias = pricing
        .resolve_at(None, &[("gpt-alias", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();

    assert_eq!(primary.input, 1e-6);
    assert_eq!(primary.output, 10e-6);
    assert_eq!(primary.cache_read, Some(0.1e-6));
    assert_eq!(pricing.context_limit("gpt-primary"), Some(123));
    assert!((fallback.input - 2e-6).abs() < f64::EPSILON);
    assert!((fallback.output - 8e-6).abs() < f64::EPSILON);
    assert!((fallback.cache_create.unwrap() - 2.5e-6).abs() < f64::EPSILON);
    assert!((fallback.cache_read.unwrap() - 0.2e-6).abs() < f64::EPSILON);
    assert_eq!(
        fallback
            .long_context
            .and_then(|context| context.rates.input),
        None
    );
    assert_eq!(
        fallback
            .long_context
            .and_then(|context| context.rates.output),
        None
    );
    assert_eq!(fallback.fast_multiplier, 1.0);
    assert_eq!(pricing.context_limit("gpt-fallback"), Some(456));
    assert!((alias.input - 4e-6).abs() < f64::EPSILON);
    assert_eq!(pricing.context_limit("gpt-alias"), Some(654));
}

#[test]
fn applies_fast_multiplier_override_to_models_dev_fallback() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let models_dev_json = r#"{
            "openai": {
                "id": "openai",
                "models": {
                    "gpt-6-astra": {
                        "id": "gpt-6-astra",
                        "cost": {
                            "input": 10.0,
                            "output": 50.0
                        }
                    }
                }
            }
        }"#;

    assert_eq!(
        pricing.load_models_dev_json_missing(models_dev_json),
        Some(1)
    );
    assert_eq!(
        pricing
            .resolve_at(None, &[("gpt-6-astra", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .fast_multiplier,
        2.0
    );
    assert_eq!(
        pricing
            .resolve_at(None, &[("gpt-6-astra", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap()
            .flex_multiplier,
        0.5
    );
}

#[test]
fn live_models_dev_pricing_prefers_the_authoring_catalog_over_resellers() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.6, "output": 3.0 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.73, "output": 3.5 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 0.95, "output": 4.0 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let kimi = pricing
        .resolve_at(None, &[("kimi-k2.7-code", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((kimi.input * 1e6 - 0.95).abs() < 1e-9);
    assert!((kimi.output * 1e6 - 4.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_skips_models_priced_per_asset() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "gemini-2.5-flash-image": {
                            "modalities": { "input": ["text", "image"], "output": ["text"] },
                            "cost": { "input": 0.3, "output": 30 }
                        }
                    }
                },
                "scaleway": {
                    "models": {
                        "some-unlisted-transcriber": {
                            "modalities": { "input": ["audio"], "output": ["text"] },
                            "cost": { "input": 0.003, "output": 0 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k3": {
                            "modalities": { "input": ["text", "image", "video"], "output": ["text"] },
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .resolve_at(None, &[("gemini-2.5-flash-image", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
    assert!(
        pricing
            .resolve_at(
                None,
                &[("some-unlisted-transcriber", MatchKind::Exact)],
                None
            )
            .map(|(_, rates)| rates)
            .is_none()
    );
    assert!(
        pricing
            .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
}

#[test]
fn live_models_dev_pricing_keys_asset_pricing_on_the_source_model_id() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "google": {
                    "models": {
                        "gemini-2.5-flash-image": {
                            "id": "models/gemini-2.5-flash-image",
                            "modalities": { "input": ["text"], "output": ["text"] },
                            "cost": { "input": 0.3, "output": 30 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(0));
    assert!(
        pricing
            .resolve_at(
                None,
                &[("models/gemini-2.5-flash-image", MatchKind::Exact)],
                None
            )
            .map(|(_, rates)| rates)
            .is_none()
    );
}

#[test]
fn live_models_dev_pricing_rejects_explicitly_empty_modalities() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "moonshotai": {
                    "models": {
                        "empty-output-model": {
                            "modalities": { "input": ["text"], "output": [] },
                            "cost": { "input": 1, "output": 2 }
                        },
                        "empty-input-model": {
                            "modalities": { "input": [], "output": ["text"] },
                            "cost": { "input": 1, "output": 2 }
                        },
                        "no-modalities-model": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .resolve_at(None, &[("empty-output-model", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
    assert!(
        pricing
            .resolve_at(None, &[("empty-input-model", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
    assert!(
        pricing
            .resolve_at(None, &[("no-modalities-model", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
}

#[test]
fn live_models_dev_pricing_carries_the_reseller_ids_generation_carries() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "fireworks-ai": {
                    "models": {
                        "accounts/fireworks/models/kimi-k2p6": {
                            "cost": { "input": 0.6, "output": 2.5 }
                        }
                    }
                },
                "venice": {
                    "models": {
                        "claude-opus-5-fast": {
                            "cost": { "input": 12, "output": 60 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "claude-3-haiku-20240307": {
                            "cost": { "input": 0.25, "output": 1.25 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(3));
    assert!(
        pricing
            .resolve_at(
                None,
                &[("accounts/fireworks/models/kimi-k2p6", MatchKind::Exact)],
                None
            )
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(
        pricing
            .resolve_at(None, &[("claude-opus-5-fast", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(
        pricing
            .resolve_at(None, &[("claude-3-haiku-20240307", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(
        pricing
            .rows
            .entries
            .get("claude-opus-5-fast")
            .is_some_and(|row| row.exact_only)
    );
    assert!(
        !pricing
            .rows
            .entries
            .get("claude-3-haiku-20240307")
            .is_some_and(|row| row.exact_only)
    );
}

#[test]
fn live_models_dev_pricing_prefers_the_more_detailed_entry_within_a_tier() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1 },
                            "limit": { "context": 128000 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!(entry.cache_read.is_some());
    assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
    assert_eq!(
        pricing
            .rows
            .entries
            .get("kimi-k2.6-nitro")
            .and_then(|row| row.context_limit.as_ref()),
        Some(&128_000)
    );
}

#[test]
fn live_models_dev_pricing_keeps_authored_token_models_a_catalog_mislabels() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "claude-opus-5": {
                            "modalities": { "input": ["text"], "output": ["text", "image"] },
                            "cost": { "input": 5, "output": 25 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .resolve_at(None, &[("claude-opus-5", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
}

#[test]
fn live_models_dev_pricing_ranks_by_the_catalog_s_own_provider_id() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "aaa-filed-under-another-key": {
                    "id": "moonshotai",
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 3, "output": 4 }
                        }
                    }
                },
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_orders_detail_the_way_generation_does() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1 }
                        }
                    }
                },
                "openrouter": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2, "cache_write": 1.25 },
                            "limit": { "context": 128000 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!(entry.cache_read.is_some());
    assert!((entry.cache_read.unwrap() * 1e6 - 0.1).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_drops_the_context_limit_of_a_replaced_catalog() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "302ai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 1, "output": 2 },
                            "limit": { "context": 128000 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k2.6-nitro": {
                            "cost": { "input": 3, "output": 4 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(None, &[("kimi-k2.6-nitro", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((entry.input * 1e6 - 3.0).abs() < 1e-9);
    assert_eq!(
        pricing
            .rows
            .entries
            .get("kimi-k2.6-nitro")
            .and_then(|row| row.context_limit.as_ref()),
        None
    );
}

#[test]
fn live_models_dev_pricing_resolves_duplicate_ids_inside_one_catalog_stably() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "moonshotai": {
                    "models": {
                        "b-alias": {
                            "id": "shared-model",
                            "cost": { "input": 9, "output": 9 }
                        },
                        "a-alias": {
                            "id": "shared-model",
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(None, &[("shared-model", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_merges_spelling_duplicates_onto_one_entry() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "llmgateway": {
                    "models": {
                        "grok-9-5": {
                            "cost": { "input": 2, "output": 6 }
                        }
                    }
                },
                "xai": {
                    "models": {
                        "grok-9.5": {
                            "cost": {
                                "input": 2, "output": 6, "cache_read": 0.3,
                                "tiers": [{
                                    "input": 4, "output": 12, "cache_read": 0.6,
                                    "tier": { "type": "context", "size": 200000 }
                                }]
                            }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(pricing.rows.get("grok-9-5").is_none());
    assert_eq!(
        pricing
            .explain(None, &[("grok-9-5", MatchKind::Exact)])
            .unwrap()
            .row_key,
        "grok-9.5"
    );
    let entry = pricing
        .resolve_at(None, &[("grok-9.5", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(
        entry.long_context.map(|context| context.threshold),
        Some(200_000)
    );
    let via_dash = pricing.find("grok-9-5").unwrap();
    assert_eq!(
        via_dash.long_context.map(|context| context.threshold),
        Some(200_000)
    );
}

#[test]
fn live_models_dev_pricing_prefers_a_tiered_catalog_within_a_trust_tier() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "llmgateway": {
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 1, "output": 2, "cache_read": 0.1, "cache_write": 1.25 },
                            "limit": { "context": 500000 }
                        }
                    }
                },
                "venice": {
                    "models": {
                        "some-reseller-only-model": {
                            "cost": {
                                "input": 1, "output": 2,
                                "tiers": [{
                                    "input": 2, "output": 4,
                                    "tier": { "type": "context", "size": 200000 }
                                }]
                            }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(
            None,
            &[("some-reseller-only-model", MatchKind::Exact)],
            None,
        )
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(
        entry.long_context.map(|context| context.threshold),
        Some(200_000)
    );
}

#[test]
fn live_models_dev_pricing_stores_a_model_with_an_empty_declared_id_by_its_key() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "moonshotai": {
                    "models": {
                        "kimi-k9": {
                            "id": "",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .resolve_at(None, &[("kimi-k9", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(
        pricing
            .resolve_at(None, &[("", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
}

#[test]
fn live_models_dev_pricing_orders_same_id_catalogs_by_their_map_key() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "zzz-alias": {
                    "id": "some-gateway",
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                },
                "aaa-alias": {
                    "id": "some-gateway",
                    "models": {
                        "some-reseller-only-model": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(
            None,
            &[("some-reseller-only-model", MatchKind::Exact)],
            None,
        )
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_breaks_exact_ties_by_source_key_across_same_id_catalogs() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "aaa-key": {
                    "id": "some-gateway",
                    "models": {
                        "zz-spelling": {
                            "id": "some-reseller-only-model",
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                },
                "zzz-key": {
                    "id": "some-gateway",
                    "models": {
                        "aa-spelling": {
                            "id": "some-reseller-only-model",
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    let entry = pricing
        .resolve_at(
            None,
            &[("some-reseller-only-model", MatchKind::Exact)],
            None,
        )
        .map(|(_, rates)| rates)
        .unwrap();
    assert!((entry.input * 1e6 - 1.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_skips_flat_fee_catalogs() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "kimi-for-coding": {
                    "models": {
                        "kimi-for-coding": {
                            "cost": { "input": 0, "output": 0 }
                        }
                    }
                },
                "moonshotai": {
                    "models": {
                        "kimi-k3": {
                            "cost": { "input": 0, "output": 0 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(0));
    assert!(
        pricing
            .resolve_at(None, &[("kimi-for-coding", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
    assert!(
        pricing
            .resolve_at(None, &[("kimi-k3", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_none()
    );
}

#[test]
fn live_models_dev_pricing_keeps_a_tier_out_of_the_fuzzy_lookup() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "moonshotai": {
                    "models": {
                        "kimi-k2.7-code": {
                            "cost": { "input": 1, "output": 2 }
                        },
                        "kimi-k2.7-code-highspeed": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(2));
    let base = pricing.find("kimi-k2-7-code").unwrap();
    assert!((base.input * 1e6 - 1.0).abs() < 1e-9);
    let tier = pricing.find("kimi-k2.7-code-highspeed").unwrap();
    assert!((tier.input * 1e6 - 9.0).abs() < 1e-9);
}

#[test]
fn live_models_dev_pricing_marks_a_regional_alias_of_a_model_exact_only() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "google-vertex-anthropic": {
                    "models": {
                        "claude-opus-5@eu": {
                            "cost": { "input": 9, "output": 9 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .rows
            .entries
            .get("claude-opus-5@eu")
            .is_some_and(|row| row.exact_only)
    );
}

#[test]
fn live_models_dev_pricing_keeps_an_unversioned_id_out_of_the_fuzzy_lookup() {
    crate::test_support::install_pricing_snapshots();
    let json = r#"{
                "moonshotai": {
                    "models": {
                        "auto": {
                            "cost": { "input": 1, "output": 2 }
                        }
                    }
                }
            }"#;
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(json), Some(1));
    assert!(
        pricing
            .resolve_at(None, &[("auto", MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .is_some()
    );
    assert!(pricing.find("codex-auto-review").is_none());
    assert!(pricing.context_limit("codex-auto-review").is_none());
}

#[test]
fn rejects_malformed_models_dev_provider_payload() {
    let fixture = fs_fixture!({
        "models-dev.json": r#"{
                "openai": {
                    "models": {
                        "gpt-fallback": {
                            "cost": {
                                "input": 2.0,
                                "output": 8.0
                            }
                        }
                    }
                },
                "broken-provider": {
                    "name": "Broken Provider"
                }
            }"#,
    });
    let json = std::fs::read_to_string(fixture.path("models-dev.json")).unwrap();
    let mut pricing = PricingMap::default();

    assert_eq!(pricing.load_models_dev_json_missing(&json), None);
    assert_eq!(pricing.len(), 0);
}

#[test]
fn loads_flat_models_dev_pricing_snapshot() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let models_dev_json = r#"{
                "claude-fallback": {
                    "cost": {
                        "input": 3.0,
                        "output": 15.0,
                        "cache_read": 0.3,
                        "cache_write": 3.75
                    },
                    "limit": {
                        "context": 200000
                    }
                }
            }"#;

    assert_eq!(
        pricing.load_models_dev_json_missing(models_dev_json),
        Some(1)
    );

    let fallback = pricing.find("claude-fallback").unwrap();
    assert!((fallback.input - 3e-6).abs() < f64::EPSILON);
    assert!((fallback.output - 15e-6).abs() < f64::EPSILON);
    assert!((fallback.cache_create.unwrap() - 3.75e-6).abs() < f64::EPSILON);
    assert!((fallback.cache_read.unwrap() - 0.3e-6).abs() < f64::EPSILON);
    assert_eq!(pricing.context_limit("claude-fallback"), Some(200000));
}

#[test]
fn applies_speed_multipliers_to_qualified_flat_models_dev_models() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let models_dev_json = r#"{
                "openai/gpt-6-astra": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "openai.gpt-5.4": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "global.openai.gpt-6.1-sol": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "global.openai.gpt-5.4-mini-2026-03-17": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                },
                "openai.gpt-5.4-mini-extra": {
                    "cost": {
                        "input": 10.0,
                        "output": 50.0
                    }
                }
            }"#;

    assert_eq!(
        pricing.load_models_dev_json_missing(models_dev_json),
        Some(5)
    );

    for (name, fast, flex) in [
        ("openai/gpt-6-astra", 2.0, 0.5),
        ("openai.gpt-5.4", 2.0, 0.5),
        ("global.openai.gpt-6.1-sol", 2.0, 0.5),
        ("global.openai.gpt-5.4-mini-2026-03-17", 1.0, 0.5),
        ("openai.gpt-5.4-mini-extra", 1.0, 1.0),
    ] {
        let model = pricing
            .resolve_at(None, &[(name, MatchKind::Exact)], None)
            .map(|(_, rates)| rates)
            .unwrap();
        assert_eq!(model.fast_multiplier, fast, "{name}");
        assert_eq!(model.flex_multiplier, flex, "{name}");
    }
}

#[test]
fn embedded_models_dev_snapshot_is_parseable() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    assert!(
        map.load_models_dev_json_missing(build_time_models_dev_json())
            .is_some()
    );
}

#[test]
fn offline_resolves_models_only_in_embedded_models_dev() {
    crate::test_support::install_pricing_snapshots();
    use ccusage_cli::PricingOverride;
    let offline = PricingMap::load_with_overrides(
        true,
        false,
        std::iter::empty::<(&String, &PricingOverride)>(),
    );
    let mut primary = PricingMap::default();
    primary.load_json(build_time_pricing_json());
    primary.put_builtin_pricing();
    let model = embedded_models_dev_pricing()
        .rows
        .matches()
        .map(|found| found.key)
        .find(|model| primary.find(model).is_none())
        .expect("the embedded models.dev snapshot should price a model the primary table misses");
    assert!(offline.find(model).is_some());
    assert!(PricingMap::default().find(model).is_none());
}

#[test]
fn offline_prices_new_anthropic_model_from_embedded_models_dev() {
    crate::test_support::install_pricing_snapshots();
    use ccusage_cli::PricingOverride;
    assert!(
        embedded_models_dev_pricing()
            .find_entry("claude-fable-5-1", MatchKind::Fuzzy)
            .is_some(),
        "embedded models.dev snapshot should include claude-fable-5-1"
    );
    let offline = PricingMap::load_with_overrides(
        true,
        false,
        std::iter::empty::<(&String, &PricingOverride)>(),
    );
    assert!(offline.find("claude-fable-5-1").is_some());
}

#[test]
fn authoritative_rows_keep_the_embedded_tier_after_a_live_models_dev_load() {
    crate::test_support::install_pricing_snapshots();
    let mut primary = PricingMap::default();
    assert_eq!(primary.load_json(r#"{
            "gpt-5.5":{"litellm_provider":"openai","input_cost_per_token":6e-6,"output_cost_per_token":31e-6}
        }"#), 1);
    let mut live = PricingMap::default();
    assert_eq!(live.load_models_dev_json_for_tests(r#"{
            "live-only":{"cost":{"input":7,"output":9}},
            "gpt-5.5":{"cost":{"input":16,"output":31,"tiers":[{"input":20,"tier":{"type":"context","size":240000}}]}}
        }"#), Some(2));
    let pricing = layered(&primary, Some(embedded_models_dev_pricing()));
    assert_eq!(
        pricing
            .find_with_live("live-only", || Some(&live))
            .unwrap()
            .input,
        7e-6
    );
    let found = pricing.find("gpt-5.5").unwrap();
    assert_eq!(found.input, 6e-6);
    let tier = found.long_context.unwrap();
    assert_eq!(tier.threshold, 272_000);
    assert_eq!(tier.rates.input, Some(10e-6));
}
