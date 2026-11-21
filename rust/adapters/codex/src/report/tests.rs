use super::*;
use crate::CodexServiceTier;

#[test]
fn report_totals_list_unpriced_codex_models_only_when_present() {
    ccusage_test_support::install_pricing_snapshots();
    let mut group = CodexGroup {
        input_tokens: 100,
        output_tokens: 5,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    group.models.insert(
        "gpt-unknown-preview".to_string(),
        CodexModelUsage {
            total_tokens: 105,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 100,
                    output_tokens: 5,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    let groups = BTreeMap::from([("2026-08-20".to_string(), group)]);

    let report = report_from_groups(
        &groups,
        AgentReportKind::Daily,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Calculate,
    );
    assert_eq!(
        report["totals"]["unpricedModels"],
        json!(["gpt-unknown-preview"])
    );
    assert_eq!(
        report["daily"][0]["models"]["gpt-unknown-preview"]["missingPricing"],
        true
    );

    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{ "gpt-free": { "litellm_provider": "openai", "input_cost_per_token": 0, "output_cost_per_token": 0 } }"#,
    );
    let mut free_group = CodexGroup {
        input_tokens: 100,
        output_tokens: 5,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    free_group.models.insert(
        "gpt-free".to_string(),
        CodexModelUsage {
            total_tokens: 105,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 100,
                    output_tokens: 5,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    let free = BTreeMap::from([("2026-08-20".to_string(), free_group)]);
    let report = report_from_groups(
        &free,
        AgentReportKind::Daily,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
        CostMode::Calculate,
    );
    assert_eq!(report["totals"]["costUSD"], 0.0);
    assert!(
        report["totals"].get("unpricedModels").is_none(),
        "a zero-rate price is a price"
    );
    assert!(
        report["daily"][0]["models"]["gpt-free"]
            .get("missingPricing")
            .is_none()
    );

    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{ "gpt-priced": { "input_cost_per_token": 0.000002, "output_cost_per_token": 0.000008 } }"#,
    );
    let mut priced_group = CodexGroup {
        input_tokens: 100,
        output_tokens: 5,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    priced_group.models.insert(
        "gpt-priced".to_string(),
        CodexModelUsage {
            total_tokens: 105,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 100,
                    output_tokens: 5,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    let priced = BTreeMap::from([("2026-08-20".to_string(), priced_group)]);
    let report = report_from_groups(
        &priced,
        AgentReportKind::Daily,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Calculate,
    );
    assert!(report["totals"]["costUSD"].as_f64().unwrap() > 0.0);
    assert!(report["totals"].get("unpricedModels").is_none());
}

#[test]
fn display_mode_ignores_even_supplied_pricing_and_omits_price_metadata() {
    ccusage_test_support::install_pricing_snapshots();
    let mut group = CodexGroup::default();
    group.models.insert(
        "gpt-unknown-preview".to_string(),
        CodexModelUsage {
            total_tokens: 100,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 100,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    let groups = BTreeMap::from([("2026-08-20".to_string(), group)]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"gpt-unknown-preview":{"input_cost_per_token":1,"output_cost_per_token":2}}"#,
    );

    let report = report_from_groups(
        &groups,
        AgentReportKind::Daily,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Display,
    );

    assert!(report["totals"].get("unpricedModels").is_none());
    assert_eq!(report["totals"]["costUSD"], 0.0);
    assert_eq!(report["daily"][0]["costUSD"], 0.0);
    assert!(
        report["daily"][0]["models"]["gpt-unknown-preview"]
            .get("missingPricing")
            .is_none()
    );
}

#[test]
fn non_cached_input_tokens_excludes_cache_reads_and_creation() {
    assert_eq!(non_cached_input_tokens(100, 60, 30), 10);
    assert_eq!(non_cached_input_tokens(100, 90, 20), 0);
}

#[test]
fn focused_report_marks_a_free_vendor_model_billed_through_a_paid_host_as_estimated() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let usage = || CodexModelUsage {
        total_tokens: 105,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 100,
                output_tokens: 5,
                ..Default::default()
            },
            Default::default(),
        )
    };
    let mut group = CodexGroup {
        input_tokens: 200,
        output_tokens: 10,
        total_tokens: 210,
        ..CodexGroup::default()
    };
    group
        .models
        .insert("kyma/glm-4.7-flash".to_string(), usage());
    group.models.insert("gpt-5.5".to_string(), usage());
    let groups = BTreeMap::from([("2026-08-20".to_string(), group)]);

    let report = report_from_groups(
        &groups,
        AgentReportKind::Daily,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Calculate,
    );
    let models = &report["daily"][0]["models"];
    assert_eq!(models["kyma/glm-4.7-flash"]["estimatedPricing"], true);
    assert!(models["gpt-5.5"].get("estimatedPricing").is_none());
    assert_eq!(
        report["totals"]["estimatedModels"],
        json!(["kyma/glm-4.7-flash"])
    );

    let display = report_from_groups(
        &groups,
        AgentReportKind::Daily,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Display,
    );
    assert!(display["totals"].get("estimatedModels").is_none());
    assert!(
        display["daily"][0]["models"]["kyma/glm-4.7-flash"]
            .get("estimatedPricing")
            .is_none()
    );
}
