use super::{BillingRoute, MatchKind, PricingMap, timestamp};

fn direct_deepseek_pricing() -> PricingMap {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "deepseek/deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                },
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                },
                "deepseek-v4-pro": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.000000435,
                    "output_cost_per_token": 0.00000087,
                    "cache_creation_input_token_cost": 0.000000456,
                    "cache_read_input_token_cost": 0.000000003625
                },
                "openrouter/deepseek-v4-flash": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010,
                    "cache_creation_input_token_cost": 0.000007,
                    "cache_read_input_token_cost": 0.000008
                },
                "openrouter/deepseek-v4-flash-vision-exp": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010
                }
            }"#,
    );
    pricing
}

#[test]
fn deepseek_flash_uses_current_peak_and_off_peak_rates() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (instant, multiplier) in [
        ("2026-11-18T00:59:59Z", 1.0),
        ("2026-11-18T01:00:00Z", 2.0),
        ("2026-11-18T03:59:59Z", 2.0),
        ("2026-11-18T04:00:00Z", 1.0),
        ("2026-11-18T05:59:59Z", 1.0),
        ("2026-11-18T06:00:00Z", 2.0),
        ("2026-11-18T09:59:59Z", 2.0),
        ("2026-11-18T10:00:00Z", 1.0),
        ("2026-11-21T02:00:00Z", 1.0),
        ("2026-11-22T07:00:00Z", 1.0),
        ("2026-11-18T09:00:00+08:00", 2.0),
    ] {
        for model in ["deepseek-flash", "deepseek/deepseek-flash"] {
            let rates = pricing.find_at(model, timestamp(instant)).unwrap();
            for (actual, off_peak) in [
                (rates.input, 0.15),
                (rates.output, 0.6),
                (rates.cache_create.unwrap(), 0.15),
                (rates.cache_read.unwrap(), 0.003),
            ] {
                assert!(
                    (actual * 1e6 - off_peak * multiplier).abs() < 1e-12,
                    "{model} at {instant}: {rates:?}"
                );
            }
        }
    }
}

#[test]
fn deepseek_flash_schedule_preserves_hosted_rates_and_explicit_overrides() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "deepseek-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.0000003,
                    "output_cost_per_token": 0.0000012
                },
                "openrouter/deepseek-flash": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000003
                }
            }"#,
    );
    let model = "deepseek-flash".to_string();
    let patch = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let overridden = pricing.with_user_overrides([(&model, &patch)]);
    for (instant, output_per_million) in
        [("2026-11-18T03:00:00Z", 1.2), ("2026-11-18T10:00:00Z", 0.6)]
    {
        let hosted = pricing
            .find_at("openrouter/deepseek-flash", timestamp(instant))
            .unwrap();
        assert!((hosted.input * 1e6 - 2.0).abs() < 1e-12);
        assert!((hosted.output * 1e6 - 3.0).abs() < 1e-12);
        let direct = overridden.find_at(&model, timestamp(instant)).unwrap();
        assert!((direct.input * 1e6 - 9.0).abs() < 1e-12);
        assert!((direct.output * 1e6 - output_per_million).abs() < 1e-12);
    }
    assert!(pricing.find_handle(&model).unwrap().is_time_dependent());
    assert!(
        !pricing
            .find_handle("openrouter/deepseek-flash")
            .unwrap()
            .is_time_dependent()
    );
    assert!((pricing.find(&model).unwrap().input * 1e6 - 0.3).abs() < 1e-12);
}

const LEGACY_V4_FLASH_BEFORE: [f64; 3] = [0.14, 0.28, 0.0028];
const LEGACY_V4_FLASH_PEAK: [f64; 3] = [0.44, 1.32, 0.014];
const LEGACY_V4_FLASH_OFF_PEAK: [f64; 3] = [0.22, 0.66, 0.007];
const LEGACY_FLASH_PEAK: [f64; 3] = [0.3, 1.2, 0.006];
const LEGACY_FLASH_OFF_PEAK: [f64; 3] = [0.15, 0.6, 0.003];

const LEGACY_DEEPSEEK_FLASH_IDS: [&str; 4] = [
    "deepseek-v4-flash",
    "deepseek/deepseek-v4-flash",
    "deepseek-v4-flash-vision-exp",
    "deepseek/deepseek-v4-flash-vision-exp",
];

fn assert_legacy_flash_rates(pricing: &PricingMap, model: &str, instant: &str, expected: [f64; 3]) {
    let rates = pricing.find_at(model, timestamp(instant)).unwrap();
    for (actual, expected) in [
        (rates.input, expected[0]),
        (rates.output, expected[1]),
        (rates.cache_create.unwrap(), expected[0]),
        (rates.cache_read.unwrap(), expected[2]),
    ] {
        assert!(
            (actual * 1e6 - expected).abs() < 1e-12,
            "{model} at {instant}: {rates:?}"
        );
    }
}

#[test]
fn legacy_deepseek_flash_ids_keep_the_v4_schedule_before_september_10() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in LEGACY_DEEPSEEK_FLASH_IDS {
        for (instant, expected) in [
            ("2026-08-16T15:59:59Z", LEGACY_V4_FLASH_BEFORE),
            ("2026-08-17T01:00:00Z", LEGACY_V4_FLASH_PEAK),
            ("2026-09-09T05:00:00Z", LEGACY_V4_FLASH_OFF_PEAK),
            ("2026-09-10T03:59:59Z", LEGACY_V4_FLASH_PEAK),
        ] {
            assert_legacy_flash_rates(&pricing, model, instant, expected);
        }
    }
}

#[test]
fn legacy_deepseek_flash_ids_bill_at_flash_rates_from_september_10() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in LEGACY_DEEPSEEK_FLASH_IDS {
        for (instant, expected) in [
            ("2026-09-10T04:00:00Z", LEGACY_FLASH_OFF_PEAK),
            ("2026-09-10T07:00:00Z", LEGACY_FLASH_PEAK),
            ("2026-09-12T02:00:00Z", LEGACY_FLASH_OFF_PEAK),
            ("2026-09-14T02:00:00Z", LEGACY_FLASH_PEAK),
            ("2026-11-18T09:00:00+08:00", LEGACY_FLASH_PEAK),
            ("2026-11-18T10:00:00Z", LEGACY_FLASH_OFF_PEAK),
        ] {
            assert_legacy_flash_rates(&pricing, model, instant, expected);
        }
    }
}

#[test]
fn deepseek_v4_pro_keeps_its_own_schedule_after_the_flash_launch() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (instant, expected) in [
        ("2026-09-14T02:00:00Z", [1.32, 3.96, 0.044]),
        ("2026-11-17T10:00:00Z", [0.66, 1.98, 0.022]),
    ] {
        let rates = pricing
            .find_at("deepseek-v4-pro", timestamp(instant))
            .unwrap();
        for (actual, expected) in [
            (rates.input, expected[0]),
            (rates.output, expected[1]),
            (rates.cache_read.unwrap(), expected[2]),
        ] {
            assert!(
                (actual * 1e6 - expected).abs() < 1e-12,
                "deepseek-v4-pro at {instant}: {rates:?}"
            );
        }
    }
}

#[test]
fn retired_deepseek_flash_ids_keep_hosted_rates_after_the_switch() {
    let pricing = direct_deepseek_pricing();
    for model in [
        "openrouter/deepseek-v4-flash",
        "openrouter/deepseek-v4-flash-vision-exp",
    ] {
        for instant in ["2026-09-10T04:00:00Z", "2026-11-17T03:00:00Z"] {
            let reseller = pricing.find_at(model, timestamp(instant)).unwrap();
            assert_eq!(reseller.input * 1e6, 9.0, "{model} {instant}");
            assert_eq!(reseller.output * 1e6, 10.0, "{model} {instant}");
        }
    }
}

#[test]
fn retired_deepseek_flash_override_keeps_flash_rates_for_unspecified_fields() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);

    let before = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();
    assert_eq!(before.input * 1e6, 9.0);
    assert_eq!(before.output * 1e6, 1.32);
    assert_eq!(before.cache_read.unwrap() * 1e6, 0.014);

    let peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-09-14T02:00:00Z"))
        .unwrap();
    assert_eq!(peak.input * 1e6, 9.0);
    assert_eq!(peak.output * 1e6, 1.2);
    assert_eq!(peak.cache_read.unwrap() * 1e6, 0.006);
    assert_eq!(peak.cache_create.unwrap() * 1e6, 0.3);

    let off_peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-09-14T04:00:00Z"))
        .unwrap();
    assert_eq!(off_peak.input * 1e6, 9.0);
    assert_eq!(off_peak.output * 1e6, 0.6);
    assert_eq!(off_peak.cache_read.unwrap() * 1e6, 0.003);
    assert_eq!(off_peak.cache_create.unwrap() * 1e6, 0.15);
}

#[test]
fn applies_deepseek_v4_rates_by_effective_period_for_both_models() {
    let pricing = direct_deepseek_pricing();
    let cases = [
        (
            "2026-08-16T15:59:59Z",
            [0.14, 0.28, 0.0028],
            [0.435, 0.87, 0.003625],
        ),
        (
            "2026-08-16T16:00:00Z",
            [0.22, 0.66, 0.007],
            [0.66, 1.98, 0.022],
        ),
        (
            "2026-08-17T01:00:00Z",
            [0.44, 1.32, 0.014],
            [1.32, 3.96, 0.044],
        ),
    ];

    for (timestamp_text, flash_expected, pro_expected) in cases {
        let flash = pricing
            .find_at("deepseek-v4-flash", timestamp(timestamp_text))
            .unwrap();
        for (actual, expected) in [flash.input, flash.output, flash.cache_read.unwrap()]
            .map(|rate| rate * 1e6)
            .into_iter()
            .zip(flash_expected)
        {
            assert!((actual - expected).abs() < 1e-12);
        }

        let pro = pricing
            .find_at("deepseek-v4-pro", timestamp(timestamp_text))
            .unwrap();
        for (actual, expected) in [pro.input, pro.output, pro.cache_read.unwrap()]
            .map(|rate| rate * 1e6)
            .into_iter()
            .zip(pro_expected)
        {
            assert!((actual - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn deepseek_v4_peak_windows_are_utc_weekday_half_open_and_offset_aware() {
    let pricing = direct_deepseek_pricing();
    let cases = [
        ("2026-08-17T00:59:59Z", 0.22),
        ("2026-08-17T01:00:00Z", 0.44),
        ("2026-08-17T03:59:59Z", 0.44),
        ("2026-08-17T04:00:00Z", 0.22),
        ("2026-08-17T05:59:59Z", 0.22),
        ("2026-08-17T06:00:00Z", 0.44),
        ("2026-08-17T09:59:59Z", 0.44),
        ("2026-08-17T10:00:00Z", 0.22),
        ("2026-08-22T02:00:00Z", 0.22),
        ("2026-08-17T09:00:00+08:00", 0.44),
    ];

    for (timestamp_text, expected_input_per_million) in cases {
        let flash = pricing
            .find_at("deepseek-v4-flash", timestamp(timestamp_text))
            .unwrap();
        assert!((flash.input * 1e6 - expected_input_per_million).abs() < 1e-12);
    }
}

#[test]
fn deepseek_v4_schedule_requires_exact_direct_model_names_and_normalizes_cache_creation() {
    let pricing = direct_deepseek_pricing();
    let direct = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();
    assert_eq!(direct.cache_create.unwrap() * 1e6, 0.44);

    let reseller = pricing
        .find_at(
            "openrouter/deepseek-v4-flash",
            timestamp("2026-08-17T01:00:00Z"),
        )
        .unwrap();
    let dotted_reseller = pricing
        .find_at(
            "openrouter/deepseek.v4.flash",
            timestamp("2026-08-17T01:00:00Z"),
        )
        .unwrap();
    assert_eq!(reseller.input * 1e6, 9.0);
    assert_eq!(reseller.output * 1e6, 10.0);
    assert_eq!(reseller.cache_read.unwrap() * 1e6, 8.0);
    assert_eq!(reseller.cache_create.unwrap() * 1e6, 7.0);
    assert_eq!(dotted_reseller.input * 1e6, 9.0);
    assert_eq!(dotted_reseller.output * 1e6, 10.0);
}

#[test]
fn find_at_applies_deepseek_schedule_after_model_alias_resolution() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "deepseek-latest",
        "deepseek-v4-flash",
    )]);
    let pricing = direct_deepseek_pricing();

    let resolved = pricing
        .find_at("deepseek-latest", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();

    assert_eq!(resolved.input * 1e6, 0.44);
}

#[test]
fn find_at_applies_deepseek_schedule_to_separator_spellings_of_direct_models() {
    let pricing = direct_deepseek_pricing();

    for model in ["deepseek.v4.flash", "deepseek@v4@flash"] {
        let resolved = pricing
            .find_at(model, timestamp("2026-08-17T01:00:00Z"))
            .unwrap();
        assert!((resolved.input * 1e6 - 0.44).abs() < 1e-12, "{model}");
    }
}

#[test]
fn resolver_table_schedules_only_a_vendor_identity_hit() {
    crate::test_support::install_pricing_snapshots();
    let mut map = PricingMap::default();
    map.load_json(r#"{
            "deepseek-v4-flash": {"litellm_provider":"bedrock","input_cost_per_token":0.00000062,"output_cost_per_token":0.00000185},
            "deepseek/deepseek-v4-pro": {"litellm_provider":"deepseek","input_cost_per_token":0.00000132,"output_cost_per_token":0.00000396},
            "deepseek-v4-pro": {"litellm_provider":"deepseek","input_cost_per_token":0.00000132,"output_cost_per_token":0.00000396}
        }"#);
    for (case, model, peak, off_peak) in [
        (
            "hosted_exact_is_static",
            "deepseek.v4.flash",
            (0.62, 1.85),
            (0.62, 1.85),
        ),
        (
            "39_vendor_identity",
            "deepseek.v4.pro",
            (1.32, 3.96),
            (0.66, 1.98),
        ),
        (
            "89_path_suffix_is_static",
            "host/deepseek/deepseek-v4-pro",
            (1.32, 3.96),
            (1.32, 3.96),
        ),
        (
            "87_longer_fuzzy_is_static",
            "deepseek-v4-pro-20260812",
            (1.32, 3.96),
            (1.32, 3.96),
        ),
    ] {
        for (instant, expected) in [
            ("2026-11-17T03:00:00Z", peak),
            ("2026-11-17T10:00:00Z", off_peak),
        ] {
            let rates = map.find_at(model, timestamp(instant)).expect(case);
            assert!(
                (rates.input * 1e6 - expected.0).abs() < 1e-12,
                "{case}: {rates:?}"
            );
            assert!(
                (rates.output * 1e6 - expected.1).abs() < 1e-12,
                "{case}: {rates:?}"
            );
        }
    }
}

#[test]
fn time_dependent_pricing_excludes_reseller_models() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "deepseek-latest",
        "deepseek-v4-flash",
    )]);

    let pricing = direct_deepseek_pricing();
    for model in [
        "deepseek-v4-flash",
        "deepseek.v4.flash",
        "deepseek@v4@flash",
        "deepseek-latest",
        "deepseek/deepseek-v4-flash",
    ] {
        assert!(
            pricing
                .find_handle(model)
                .is_some_and(|handle| handle.is_time_dependent())
        );
    }
    for model in [
        "openrouter/deepseek-v4-flash",
        "openrouter/deepseek.v4.flash",
        "gpt-5",
    ] {
        assert!(
            !pricing
                .find_handle(model)
                .is_some_and(|handle| handle.is_time_dependent())
        );
    }
}

#[test]
fn find_at_replaces_stale_deepseek_long_context_rates() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.00000014,
                    "cache_read_input_token_cost": 0.0000000028,
                    "input_cost_per_token_above_200k_tokens": 0.000009,
                    "output_cost_per_token_above_200k_tokens": 0.000010,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.000011,
                    "cache_read_input_token_cost_above_200k_tokens": 0.000012
                }
            }"#,
    );

    let peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();

    assert_eq!(
        peak.long_context.and_then(|context| context.rates.input),
        Some(0.44e-6)
    );
    assert_eq!(
        peak.long_context.and_then(|context| context.rates.output),
        Some(1.32e-6)
    );
    assert_eq!(
        peak.long_context
            .and_then(|context| context.rates.cache_create),
        Some(0.44e-6)
    );
    assert_eq!(
        peak.long_context
            .and_then(|context| context.rates.cache_read),
        Some(0.014e-6)
    );
}

#[test]
fn offline_embedded_pricing_uses_the_deepseek_v4_schedule() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_with_overrides(
        true,
        false,
        std::iter::empty::<(&String, &ccusage_cli::PricingOverride)>(),
    );
    let flash = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();

    assert!((flash.input * 1e6 - 0.44).abs() < 1e-12);
    assert!((flash.output * 1e6 - 1.32).abs() < 1e-12);
    assert!((flash.cache_read.unwrap() * 1e6 - 0.014).abs() < 1e-12);
}

#[test]
fn user_deepseek_v4_overrides_remain_authoritative() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        output_cost_per_token: Some(10e-6),
        cache_creation_input_token_cost: Some(7e-6),
        cache_read_input_token_cost: Some(8e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);
    let flash = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();

    assert_eq!(flash.input * 1e6, 9.0);
    assert_eq!(flash.output * 1e6, 10.0);
    assert_eq!(flash.cache_read.unwrap() * 1e6, 8.0);
    assert_eq!(flash.cache_create.unwrap() * 1e6, 7.0);
}

#[test]
fn partial_deepseek_v4_override_keeps_schedule_for_unspecified_fields() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);

    let peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();
    assert_eq!(peak.input * 1e6, 9.0);
    assert_eq!(peak.output * 1e6, 1.32);
    assert_eq!(peak.cache_read.unwrap() * 1e6, 0.014);
    assert_eq!(peak.cache_create.unwrap() * 1e6, 0.44);

    let long_context = peak.long_context.unwrap().rates;
    assert_eq!(long_context.input, Some(9e-6));
    assert_eq!(long_context.output, Some(1.32e-6));
    assert_eq!(long_context.cache_create, Some(0.44e-6));
    assert_eq!(long_context.cache_read, Some(0.014e-6));

    let off_peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T04:00:00Z"))
        .unwrap();
    assert_eq!(off_peak.input * 1e6, 9.0);
    assert_eq!(off_peak.output * 1e6, 0.66);
    assert_eq!(off_peak.cache_read.unwrap() * 1e6, 0.007);
    assert_eq!(off_peak.cache_create.unwrap() * 1e6, 0.22);
}

#[test]
fn base_only_scheduled_override_replaces_the_schedule_above_the_long_context_split() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let override_value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        output_cost_per_token: Some(10e-6),
        cache_creation_input_token_cost: Some(7e-6),
        cache_read_input_token_cost: Some(8e-6),
        ..Default::default()
    };
    let pricing = PricingMap::load_with_overrides(true, false, [(&model, &override_value)]);

    let peak = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();
    let rates = peak.long_context.unwrap().rates;
    assert_eq!(rates.input, Some(9e-6));
    assert_eq!(rates.output, Some(10e-6));
    assert_eq!(rates.cache_create, Some(7e-6));
    assert_eq!(rates.cache_read, Some(8e-6));
}

#[test]
fn scheduled_override_precedence_selects_one_patch_without_inheriting_its_base_patch() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "scheduled-alias",
        "deepseek.v4.flash",
    )]);
    let base = direct_deepseek_pricing();
    let overrides = std::collections::BTreeMap::from([
        (
            "deepseek-v4-flash".to_string(),
            ccusage_cli::PricingOverride {
                output_cost_per_token: Some(19.0),
                ..Default::default()
            },
        ),
        (
            "deepseek.v4.flash".to_string(),
            ccusage_cli::PricingOverride {
                cache_read_input_token_cost: Some(0.0),
                ..Default::default()
            },
        ),
        (
            "scheduled-alias".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(7.0),
                ..Default::default()
            },
        ),
    ]);
    let pricing = base.with_user_overrides(&overrides);
    let instant = timestamp("2026-08-17T01:00:00Z");
    let alias = pricing.find_at("scheduled-alias", instant).unwrap();
    assert_eq!(alias.input, 7.0);
    assert_eq!(alias.output, 1.32e-6);
    assert_eq!(alias.cache_read, Some(0.014e-6));
    let resolved = pricing.find_at("deepseek.v4.flash", instant).unwrap();
    assert_eq!(resolved.input, 0.44e-6);
    assert_eq!(resolved.cache_read, Some(0.0));
    let direct = pricing.find_at("deepseek-v4-flash", instant).unwrap();
    assert_eq!(direct.output, 19.0);
    assert_eq!(resolved.output, 19.0);
    assert_eq!(direct.cache_read, Some(0.0));
    assert!(pricing.has_user_override("scheduled-alias"));
    assert!(!base.has_user_override("deepseek-v4-flash"));
}

#[test]
fn timestamped_candidates_keep_exact_provider_rates_and_schedule_direct_names() {
    let pricing = direct_deepseek_pricing();
    let instant = timestamp("2026-08-17T01:00:00Z");
    let (matched, provider) = pricing
        .resolve_at(
            None,
            &[
                (
                    "openrouter/deepseek.v4.flash",
                    super::super::MatchKind::Exact,
                ),
                ("deepseek-v4-flash", super::super::MatchKind::Fuzzy),
            ],
            Some(instant),
        )
        .unwrap();
    assert_eq!(matched, "openrouter/deepseek.v4.flash");
    assert_eq!(provider.input, 9e-6);
    let (matched, direct) = pricing
        .resolve_at(
            None,
            &[
                ("unknown/deepseek-v4-flash", super::super::MatchKind::Exact),
                ("deepseek-v4-flash", super::super::MatchKind::Fuzzy),
            ],
            Some(instant),
        )
        .unwrap();
    assert_eq!(matched, "deepseek-v4-flash");
    assert_eq!(direct.input, 0.44e-6);
}

#[test]
fn scheduled_alias_override_preserves_duration_and_speed_fields_from_the_selected_patch() {
    let _aliases =
        crate::model_aliases::set_model_aliases_for_tests([("deepseek-v4-flash", "custom-base")]);
    let base = direct_deepseek_pricing();
    let model = "custom-base".to_string();
    let value = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(1.0),
        output_cost_per_token: Some(2.0),
        cache_creation_input_token_cost: Some(3.0),
        cache_read_input_token_cost: Some(4.0),
        input_cost_per_token_above_200k_tokens: Some(5.0),
        output_cost_per_token_above_200k_tokens: Some(6.0),
        cache_creation_input_token_cost_above_200k_tokens: Some(7.0),
        cache_read_input_token_cost_above_200k_tokens: Some(8.0),
        cache_creation_input_token_cost_above_1hr: Some(13.0),
        fast_multiplier: Some(3.0),
        flex_multiplier: Some(0.4),
        ..Default::default()
    };
    let pricing = base.with_user_overrides([(&model, &value)]);
    let scheduled = pricing
        .find_at("deepseek-v4-flash", timestamp("2026-08-17T01:00:00Z"))
        .unwrap();
    assert_eq!(scheduled.input, 1.0);
    assert_eq!(scheduled.output, 2.0);
    assert_eq!(scheduled.cache_create, Some(3.0));
    assert_eq!(scheduled.cache_read, Some(4.0));
    let rates = scheduled.long_context.unwrap().rates;
    assert_eq!(rates.input, Some(5.0));
    assert_eq!(rates.output, Some(6.0));
    assert_eq!(rates.cache_create, Some(7.0));
    assert_eq!(rates.cache_read, Some(8.0));
    assert_eq!(scheduled.cache_create_1h, Some(13.0));
    assert_eq!(scheduled.fast_multiplier, 3.0);
    assert_eq!(scheduled.flex_multiplier, 0.4);
}

fn input_per_million_on_route(
    pricing: &PricingMap,
    model: &str,
    candidates: &[&str],
    route: BillingRoute,
    instant: &str,
) -> f64 {
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| (*name, MatchKind::Fuzzy))
        .collect();
    let (_, rates) = pricing
        .resolve_on_route_at(Some(model), &candidates, route, Some(timestamp(instant)))
        .unwrap();
    rates.input * 1e6
}

#[test]
fn billing_route_decides_deepseek_schedule_eligibility() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "deepseek-latest",
        "deepseek-v4-flash",
    )]);
    crate::test_support::install_pricing_snapshots();
    let mut qualified_only = PricingMap::default();
    qualified_only.load_json(
        r#"{
                "deepseek/deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.0000003,
                    "output_cost_per_token": 0.0000012
                }
            }"#,
    );
    let mut bare_only = PricingMap::default();
    bare_only.load_json(
        r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.0000003,
                    "output_cost_per_token": 0.0000012
                }
            }"#,
    );
    let direct = BillingRoute::Direct("deepseek");
    let foreign = BillingRoute::Foreign;
    for (case, pricing, model, candidates, route, instant, expected) in [
        (
            "direct_bare_before_cutoff",
            &bare_only,
            "deepseek-v4-flash",
            &["deepseek-v4-flash", "deepseek/deepseek-v4-flash"][..],
            direct,
            "2026-08-16T15:59:59Z",
            0.14,
        ),
        (
            "direct_bare_peak",
            &bare_only,
            "deepseek-v4-flash",
            &["deepseek-v4-flash", "deepseek/deepseek-v4-flash"][..],
            direct,
            "2026-08-17T01:00:00Z",
            0.44,
        ),
        (
            "direct_falls_back_to_qualified_row_off_peak",
            &qualified_only,
            "deepseek-v4-flash",
            &["deepseek-v4-flash", "deepseek/deepseek-v4-flash"][..],
            direct,
            "2026-08-17T04:00:00Z",
            0.22,
        ),
        (
            "direct_qualified_first_peak",
            &bare_only,
            "deepseek-v4-flash",
            &["deepseek/deepseek-v4-flash", "deepseek-v4-flash"][..],
            direct,
            "2026-08-17T01:00:00Z",
            0.44,
        ),
        (
            "direct_alias_peak",
            &bare_only,
            "deepseek-latest",
            &["deepseek-latest", "deepseek/deepseek-latest"][..],
            direct,
            "2026-08-17T01:00:00Z",
            0.44,
        ),
        (
            "foreign_missing_qualified_price_is_static_at_peak",
            &bare_only,
            "deepseek-v4-flash",
            &["openrouter/deepseek-v4-flash", "deepseek-v4-flash"][..],
            foreign,
            "2026-08-17T01:00:00Z",
            0.3,
        ),
        (
            "foreign_missing_qualified_price_is_static_off_peak",
            &bare_only,
            "deepseek-v4-flash",
            &["openrouter/deepseek-v4-flash", "deepseek-v4-flash"][..],
            foreign,
            "2026-08-17T04:00:00Z",
            0.3,
        ),
        (
            "foreign_missing_qualified_price_is_static_before_cutoff",
            &bare_only,
            "deepseek-v4-flash",
            &["openrouter/deepseek-v4-flash", "deepseek-v4-flash"][..],
            foreign,
            "2026-08-16T15:59:59Z",
            0.3,
        ),
    ] {
        let actual = input_per_million_on_route(pricing, model, candidates, route, instant);
        assert!((actual - expected).abs() < 1e-12, "{case}: {actual}");
    }
}

#[test]
fn user_overrides_take_precedence_over_the_route_schedule() {
    crate::test_support::install_pricing_snapshots();
    let model = "deepseek-v4-flash".to_string();
    let patch = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(9e-6),
        ..Default::default()
    };
    let pricing = direct_deepseek_pricing().with_user_overrides([(&model, &patch)]);
    let candidates = [("deepseek-v4-flash", MatchKind::Fuzzy)];
    let instant = Some(timestamp("2026-08-17T01:00:00Z"));

    let (_, direct) = pricing
        .resolve_on_route_at(
            Some(&model),
            &candidates,
            BillingRoute::Direct("deepseek"),
            instant,
        )
        .unwrap();
    assert_eq!(direct.input * 1e6, 9.0);
    assert!((direct.output * 1e6 - 1.32).abs() < 1e-12);

    let (_, foreign) = pricing
        .resolve_on_route_at(Some(&model), &candidates, BillingRoute::Foreign, instant)
        .unwrap();
    assert_eq!(foreign.input * 1e6, 9.0);
    assert!((foreign.output * 1e6 - 0.28).abs() < 1e-12);
}

#[test]
fn google_spellings_follow_the_announced_gemini_price_changes() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (model, instant, input, output) in [
        ("gemini-4-argon", "2026-11-30T23:59:59Z", 2.0, 10.0),
        ("gemini-4-argon", "2026-12-01T00:00:00Z", 4.0, 20.0),
        ("google/gemini-4-argon", "2026-11-30T23:59:59Z", 2.0, 10.0),
        ("google/gemini-4-argon", "2026-12-01T00:00:00Z", 4.0, 20.0),
        ("google/gemini-4-carbon", "2026-12-01T00:00:00Z", 4.0, 20.0),
        ("gemini-3.8-flash", "2026-12-31T23:59:59Z", 0.75, 3.75),
        ("google/gemini-3.8-flash", "2027-01-01T00:00:00Z", 1.5, 7.5),
        ("gemini-3.6-flash", "2027-01-01T00:00:00Z", 1.5, 7.5),
        ("gemini-3.7-flash", "2026-12-31T23:59:59Z", 0.75, 3.75),
        ("google/gemini-3.7-flash", "2027-01-01T00:00:00Z", 1.5, 7.5),
        ("gemini-2.5-pro", "2026-11-30T23:59:59Z", 1.25, 10.0),
        ("gemini-2.5-pro", "2026-12-02T00:00:00Z", 0.75, 3.75),
        ("google/gemini-2.5-pro", "2027-01-02T00:00:00Z", 1.5, 7.5),
        ("gemini-2.5-flash", "2026-12-02T00:00:00Z", 0.3, 2.5),
        ("gemini-3.8-flash-tts", "2026-12-31T23:59:59Z", 0.5, 9.0),
        ("gemini-3.8-flash-tts", "2027-01-01T00:00:00Z", 1.0, 18.0),
        (
            "gemini-3.8-flash-lite-tts",
            "2027-01-01T00:00:00Z",
            1.0,
            12.0,
        ),
        (
            "gemini-robotics-er-2-preview",
            "2027-01-01T00:00:00Z",
            2.0,
            10.0,
        ),
        (
            "gemini-robotics-er-2-streaming-preview",
            "2026-12-31T23:59:59Z",
            1.0,
            5.0,
        ),
        (
            "gemini-robotics-er-2-streaming-preview",
            "2027-01-01T00:00:00Z",
            2.0,
            10.0,
        ),
    ] {
        let rates = pricing.find_at(model, timestamp(instant)).unwrap();
        assert_eq!(
            (rates.input * 1e6, rates.output * 1e6),
            (input, output),
            "{model} at {instant}"
        );
    }
}

#[test]
fn usage_billed_by_google_follows_the_gemini_price_change_and_foreign_hosts_stay_flat() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let route = |provider| {
        super::super::direct_billing_vendor(provider)
            .map_or(BillingRoute::Foreign, BillingRoute::Direct)
    };
    for (provider, instant, input) in [
        ("google", "2026-11-30T23:59:59Z", 2.0),
        ("google", "2026-12-01T00:00:00Z", 4.0),
        ("gemini", "2026-12-01T00:00:00Z", 4.0),
        ("openrouter", "2026-12-01T00:00:00Z", 2.0),
    ] {
        let candidates = [
            (format!("{provider}/gemini-4-argon"), MatchKind::Exact),
            ("gemini-4-argon".to_owned(), MatchKind::Fuzzy),
        ];
        let candidates: Vec<_> = candidates
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        let (_, rates) = pricing
            .resolve_on_route_at(
                Some("gemini-4-argon"),
                &candidates,
                route(provider),
                Some(timestamp(instant)),
            )
            .unwrap();
        assert_eq!(rates.input * 1e6, input, "{provider} at {instant}");
    }
}

#[test]
fn gemini_2_5_pro_keeps_its_long_context_tier_until_it_is_rebilled() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let tier = |instant| {
        pricing
            .find_at("gemini-2.5-pro", timestamp(instant))
            .unwrap()
            .long_context
            .and_then(|context| context.rates.input)
            .map(|rate| rate * 1e6)
    };

    assert_eq!(tier("2026-11-30T23:59:59Z"), Some(2.5));
    assert_eq!(tier("2026-12-02T00:00:00Z"), Some(0.75));
}

#[test]
fn a_scheduled_override_bills_one_hour_writes_above_the_split_at_its_base_rate() {
    let model = "deepseek-v4-flash".to_owned();
    let patch = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(50e-6),
        cache_creation_input_token_cost_above_1hr: Some(10e-6),
        ..Default::default()
    };
    let pricing = direct_deepseek_pricing().with_user_overrides([(&model, &patch)]);
    let handle = pricing.find_handle(&model).unwrap();
    assert!(handle.is_time_dependent());
    let rate = handle.with_rates_at(Some(timestamp("2026-11-17T10:00:00Z")), |rates| {
        rates.one_hour_cache_creation_rate_above_200k()
    });
    assert_eq!(rate, Some(10e-6));
}

#[test]
fn a_scheduled_override_tier_bills_one_hour_writes_at_the_scheduled_base_rate() {
    let model = "deepseek-v4-flash".to_owned();
    let patch = ccusage_cli::PricingOverride {
        output_cost_per_token: Some(1e-6),
        input_cost_per_token_above_200k_tokens: Some(1e-6),
        ..Default::default()
    };
    let pricing = direct_deepseek_pricing().with_user_overrides([(&model, &patch)]);
    let handle = pricing.find_handle(&model).unwrap();
    let (input, rate) = handle.with_rates_at(Some(timestamp("2026-08-20T12:00:00Z")), |rates| {
        (rates.input, rates.one_hour_cache_creation_rate_above_200k())
    });
    assert_eq!(input, 0.22e-6);
    assert_eq!(rate, Some(0.22e-6));
}

#[test]
fn an_inherited_override_tier_bills_one_hour_writes_at_the_scheduled_base_rate() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([(
        "scheduled-tier-child",
        "deepseek-v4-flash",
    )]);
    let overrides = std::collections::BTreeMap::from([
        (
            "deepseek-v4-flash".to_string(),
            ccusage_cli::PricingOverride {
                output_cost_per_token: Some(1e-6),
                input_cost_per_token_above_200k_tokens: Some(1e-6),
                ..Default::default()
            },
        ),
        (
            "scheduled-tier-child".to_string(),
            ccusage_cli::PricingOverride {
                max_input_tokens: Some(500_000),
                ..Default::default()
            },
        ),
    ]);
    let pricing = direct_deepseek_pricing().with_user_overrides(&overrides);
    let handle = pricing.find_handle("scheduled-tier-child").unwrap();
    let (input, long_input, rate) =
        handle.with_rates_at(Some(timestamp("2026-08-20T12:00:00Z")), |rates| {
            (
                rates.input,
                rates.long_context.and_then(|tier| tier.rates.input),
                rates.one_hour_cache_creation_rate_above_200k(),
            )
        });
    assert_eq!((input, long_input), (0.22e-6, Some(1e-6)));
    assert_eq!(rate, Some(0.22e-6));
}

#[test]
fn gemini_2_5_keeps_its_published_rates_before_the_rebilling_whatever_the_row_says() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gemini/gemini-2.5-pro": {"litellm_provider": "gemini", "input_cost_per_token": 7.5e-7, "output_cost_per_token": 3.75e-6},
            "gemini/gemini-2.5-flash": {"litellm_provider": "gemini", "input_cost_per_token": 9e-7, "output_cost_per_token": 9e-6}
        }"#,
    );
    let rates = |model, instant| {
        let found = pricing.find_at(model, timestamp(instant)).unwrap();
        let tier = found
            .long_context
            .and_then(|context| context.rates.input)
            .map(|rate| rate * 1e6);
        (found.input * 1e6, found.output * 1e6, tier)
    };

    assert_eq!(
        rates("gemini-2.5-pro", "2026-11-30T23:59:59Z"),
        (1.25, 10.0, Some(2.5))
    );
    assert_eq!(
        rates("gemini-2.5-pro", "2026-12-01T00:00:00Z"),
        (0.75, 3.75, Some(0.75))
    );
    assert_eq!(rates("gemini-2.5-flash", "2026-11-30T23:59:59Z").0, 0.3);
    assert_eq!(rates("gemini-2.5-flash", "2026-12-01T00:00:00Z").0, 0.3);
}

#[test]
fn a_vendors_own_provider_route_applies_only_that_vendors_schedule() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let model = "grok-code-fast-1";
    for (route, before, after) in [
        (BillingRoute::Direct("xai"), 0.2, 1.0),
        (BillingRoute::Direct("deepseek"), 1.0, 1.0),
        (BillingRoute::Foreign, 1.0, 1.0),
    ] {
        for (instant, expected) in [
            ("2026-05-15T14:59:11Z", before),
            ("2026-05-15T14:59:12Z", after),
        ] {
            let input = input_per_million_on_route(&pricing, model, &[model], route, instant);
            assert!(
                (input - expected).abs() < 1e-9,
                "{route:?} {instant}: {input} != {expected}"
            );
        }
    }
}

fn grok_with_override(
    override_value: &ccusage_cli::PricingOverride,
) -> (PricingMap, [(&'static str, MatchKind); 1]) {
    crate::test_support::install_pricing_snapshots();
    let key = "grok-code-fast-1".to_string();
    (
        PricingMap::load_embedded().with_user_overrides([(&key, override_value)]),
        [("grok-code-fast-1", MatchKind::Fuzzy)],
    )
}

#[test]
fn an_explicit_base_rate_override_wins_over_a_scheduled_tier_at_every_size() {
    let (pricing, candidates) = grok_with_override(&ccusage_cli::PricingOverride {
        input_cost_per_token: Some(0.5e-6),
        ..Default::default()
    });
    for (instant, expected) in [
        (
            "2026-05-15T14:59:11Z",
            250_000.0 * 0.5e-6 + 1_000.0 * 1.5e-6,
        ),
        ("2026-05-15T14:59:12Z", 250_000.0 * 0.5e-6 + 1_000.0 * 2e-6),
    ] {
        let cost = crate::cost::price_entry(
            crate::cost::EntryPricing {
                granularity: crate::cost::UsageGranularity::Request,
                usage: crate::TokenUsageRaw {
                    input_tokens: 250_000,
                    output_tokens: 1_000,
                    ..Default::default()
                },
                reasoning_tokens: 0,
                residual_tokens: 0,
                reported_model: Some("grok-code-fast-1"),
                candidates: &candidates,
                stored_cost: None,
                timestamp: Some(timestamp(instant)),
            },
            crate::cost::PriceTable::Calculate(&pricing),
        )
        .cost;
        assert!(
            (cost - expected).abs() < 1e-9,
            "{instant}: {cost} != {expected}"
        );
    }
}

#[test]
fn a_multiplier_only_override_keeps_the_scheduled_tier() {
    let (pricing, candidates) = grok_with_override(&ccusage_cli::PricingOverride {
        fast_multiplier: Some(2.0),
        ..Default::default()
    });
    let plain = PricingMap::load_embedded();
    let instant = Some(timestamp("2026-05-16T00:00:00Z"));
    let (_, overridden) = pricing
        .resolve_at(Some("grok-code-fast-1"), &candidates, instant)
        .unwrap();
    let (_, scheduled) = plain
        .resolve_at(Some("grok-code-fast-1"), &candidates, instant)
        .unwrap();
    assert_eq!(overridden.long_context, scheduled.long_context);
    assert_eq!(overridden.fast_multiplier, 2.0);
}
