mod compaction;
mod compression;
mod loader;

use std::collections::BTreeMap;

use super::report::{calculate_codex_model_cost, codex_model_missing_pricing, report_from_groups};
use super::*;
use crate::cli::{AgentReportKind, CostMode, SharedArgs};
use crate::test_support::{load_groups_from_directory, load_rollouts, token_count_line};
use crate::{CodexModelUsage, CodexServiceTier, CodexUsageBucket};
use ccusage_test_support::fs_fixture;
use serde_json::{Value, json};

fn rollout_report(
    lines: &[String],
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
) -> Value {
    let groups = load_rollouts(&[("session.jsonl", lines)], kind);
    report_from_groups(&groups, kind, Some(pricing), speed, CostMode::Calculate)
}

fn usage_line(
    timestamp: &str,
    model: &str,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
) -> String {
    token_count_line(
        timestamp,
        model,
        json!({
            "input_tokens": input_tokens,
            "cached_input_tokens": cached_input_tokens,
            "output_tokens": output_tokens,
            "reasoning_output_tokens": reasoning_output_tokens,
            "total_tokens": input_tokens + output_tokens,
        }),
    )
}

#[test]
fn builds_codex_daily_json_report() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let lines = [usage_line(
        "2026-01-02T00:00:01.000Z",
        "gpt-5",
        100,
        10,
        50,
        0,
    )];

    let report = rollout_report(
        &lines,
        AgentReportKind::Daily,
        &pricing,
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    assert_eq!(report["daily"][0]["date"], "2026-01-02");
    assert_eq!(report["daily"][0]["inputTokens"], 90);
    assert_eq!(report["daily"][0]["cacheCreationTokens"], 0);
    assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["reasoningOutputTokens"], 0);
    assert_eq!(report["daily"][0]["totalTokens"], 150);
    assert_eq!(report["daily"][0]["models"]["gpt-5"]["inputTokens"], 90);
    assert_eq!(
        report["daily"][0]["models"]["gpt-5"]["cacheCreationTokens"],
        0
    );
    assert_eq!(report["daily"][0]["models"]["gpt-5"]["cacheReadTokens"], 10);
    assert_eq!(report["daily"][0]["costUSD"], json!(0.00061375));
    assert_eq!(report["totals"]["costUSD"], json!(0.00061375));
}

#[test]
fn prices_codex_versioned_and_fast_models_from_embedded_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let cost = |line: String, tier: CodexServiceTier| {
        rollout_report(
            &[line],
            AgentReportKind::Daily,
            &pricing,
            CodexSpeedPolicy::Forced(tier),
        )["daily"][0]["costUSD"]
            .as_f64()
            .unwrap()
    };
    let versioned = usage_line("2026-01-02T00:00:01.000Z", "gpt-5.3-codex", 120, 30, 11, 3);
    let gpt_5_4 = usage_line("2026-03-18T00:00:01.000Z", "gpt-5.4", 100, 40, 10, 0);

    assert!((cost(versioned, CodexServiceTier::Standard) - 0.00031675).abs() < f64::EPSILON);
    assert!((cost(gpt_5_4.clone(), CodexServiceTier::Standard) - 0.00031).abs() < f64::EPSILON);
    assert!((cost(gpt_5_4, CodexServiceTier::Fast) - 0.00062).abs() < f64::EPSILON);
}

#[test]
fn applies_codex_fast_speed_multiplier_to_report_costs() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "cache_read_input_token_cost": 0.0000005,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let lines = [usage_line(
        "2026-01-02T00:00:01.000Z",
        "gpt-test",
        10,
        2,
        5,
        0,
    )];
    let cost = |tier| {
        rollout_report(
            &lines,
            AgentReportKind::Daily,
            &pricing,
            CodexSpeedPolicy::Forced(tier),
        )["daily"][0]["costUSD"]
            .clone()
    };

    assert_eq!(cost(CodexServiceTier::Standard), json!(0.000019));
    assert_eq!(cost(CodexServiceTier::Fast), json!(0.000038));
}

fn session_groups(ids: &[&str]) -> BTreeMap<String, CodexGroup> {
    ids.iter()
        .map(|id| ((*id).to_string(), CodexGroup::default()))
        .collect()
}

#[test]
fn resolves_codex_session_from_supported_id_forms() {
    let full_id = "2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0";
    let groups = session_groups(&[full_id]);

    for requested in [
        full_id,
        "rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0",
        "rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl",
        "rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl.zst",
        "01a0bb8c-c7c0-7630-9d82-860b875930f0",
        "codex://threads/01a0bb8c-c7c0-7630-9d82-860b875930f0",
    ] {
        assert_eq!(resolve_session_id(&groups, requested).unwrap(), full_id);
    }
}

#[test]
fn rejects_ambiguous_codex_session_uuid() {
    let uuid = "01a0bb8c-c7c0-7630-9d82-860b875930f0";
    let groups = session_groups(&[
        "2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0",
        "2026/09/20/rollout-2026-09-20T10-00-00-01a0bb8c-c7c0-7630-9d82-860b875930f0",
    ]);

    let error = resolve_session_id(&groups, uuid).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("Codex session ID '{uuid}' is ambiguous and matches 2 sessions.")
    );
}

#[test]
fn rejects_unknown_codex_session_id() {
    let groups = session_groups(&["2026/09/19/rollout-known"]);

    let error = resolve_session_id(&groups, "missing").unwrap_err();

    assert_eq!(error.to_string(), "No Codex session found with ID: missing");
}

#[test]
fn loads_directory_groups_with_date_filter_without_global_event_vector() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/session.jsonl": [
            r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#,
            r#"{"timestamp":"2026-01-03T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":200,"cached_input_tokens":20,"output_tokens":75,"reasoning_output_tokens":5,"total_tokens":280}}}}"#,
        ]
        .join("\n"),
    });
    let sessions_dir = fixture.path("sessions");
    let shared = SharedArgs {
        since: Some("20260103".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let groups =
        load_groups_from_directory(&sessions_dir, &shared, AgentReportKind::Daily).unwrap();

    assert_eq!(groups.len(), 1);
    let group = groups.get("2026-01-03").unwrap();
    assert_eq!(group.input_tokens, 200);
    assert_eq!(group.cached_input_tokens, 20);
    assert_eq!(group.output_tokens, 75);
    assert_eq!(group.reasoning_output_tokens, 5);
    assert_eq!(group.total_tokens, 280);
}

#[test]
fn dedupes_matching_grouped_codex_usage_events_from_distinct_sessions() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#;
    let fixture = fs_fixture!({
        "sessions/session-a.jsonl": usage_line,
        "sessions/session-b.jsonl": usage_line,
    });
    let sessions_dir = fixture.path("sessions");
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let groups =
        load_groups_from_directory(&sessions_dir, &shared, AgentReportKind::Daily).unwrap();

    assert_eq!(groups.len(), 1);
    let group = groups.get("2026-01-02").unwrap();
    assert_eq!(group.input_tokens, 100);
    assert_eq!(group.cached_input_tokens, 10);
    assert_eq!(group.output_tokens, 50);
    assert_eq!(group.total_tokens, 150);
}

#[test]
fn reports_non_cached_codex_input_separately_from_cached_input() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::default();
    let report = rollout_report(
        &[usage_line(
            "2026-01-02T00:00:00.000Z",
            "gpt-5",
            100,
            90,
            5,
            0,
        )],
        AgentReportKind::Daily,
        &pricing,
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    assert_eq!(report["daily"][0]["inputTokens"], 10);
    assert_eq!(report["daily"][0]["cacheCreationTokens"], 0);
    assert_eq!(report["daily"][0]["cacheReadTokens"], 90);
    assert_eq!(report["daily"][0]["totalTokens"], 105);
    assert_eq!(report["totals"]["inputTokens"], 10);
    assert_eq!(report["totals"]["cacheCreationTokens"], 0);
    assert_eq!(report["totals"]["cacheReadTokens"], 90);
    assert_eq!(report["totals"]["totalTokens"], 105);
    assert_eq!(report["daily"][0]["models"]["gpt-5"]["inputTokens"], 10);
    assert_eq!(
        report["daily"][0]["models"]["gpt-5"]["cacheCreationTokens"],
        0
    );
    assert_eq!(report["daily"][0]["models"]["gpt-5"]["cacheReadTokens"], 90);
}

#[test]
fn prices_mixed_deepseek_timestamps_in_model_totals() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                }
            }"#,
    );
    let line = |timestamp: &str| usage_line(timestamp, "deepseek-v4-flash", 1_000_000, 0, 0, 0);
    let lines = [
        line("2026-08-16T15:59:59.000Z"),
        line("2026-08-16T16:00:00.000Z"),
        line("2026-08-17T01:00:00.000Z"),
    ];

    let report = rollout_report(
        &lines,
        AgentReportKind::Daily,
        &pricing,
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    assert!((report["totals"]["costUSD"].as_f64().unwrap() - 0.80).abs() < 1e-12);
    assert!((report["daily"][0]["costUSD"].as_f64().unwrap() - 0.36).abs() < 1e-12);
    assert!((report["daily"][1]["costUSD"].as_f64().unwrap() - 0.44).abs() < 1e-12);
}

#[test]
fn reports_codex_cache_write_tokens_and_cost_for_gpt_5_6_terra() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "session.jsonl": [
            json!({
                "timestamp": "2026-08-20T05:49:00.000Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-terra" },
            })
            .to_string(),
            json!({
                "timestamp": "2026-08-20T05:49:12.034Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 935_040,
                            "cached_input_tokens": 875_306,
                            "cache_write_input_tokens": 57_610,
                            "output_tokens": 11_150,
                            "reasoning_output_tokens": 1_141,
                            "total_tokens": 946_190,
                        },
                        "total_token_usage": {
                            "input_tokens": 935_040,
                            "cached_input_tokens": 875_306,
                            "cache_write_input_tokens": 57_610,
                            "output_tokens": 11_150,
                            "reasoning_output_tokens": 1_141,
                            "total_tokens": 946_190,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });
    let shared = SharedArgs {
        single_thread: true,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let groups =
        load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Daily).unwrap();
    let report = report_from_groups(
        &groups,
        AgentReportKind::Daily,
        Some(&PricingMap::load_embedded()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CostMode::Calculate,
    );
    let daily = &report["daily"][0];
    let model = &daily["models"]["gpt-5.6-terra"];

    assert_eq!(daily["inputTokens"], 2_124);
    assert_eq!(daily["cacheCreationTokens"], 57_610);
    assert_eq!(daily["cacheReadTokens"], 875_306);
    assert_eq!(daily["totalTokens"], 946_190);
    assert_eq!(model["inputTokens"], 2_124);
    assert_eq!(model["cacheCreationTokens"], 57_610);
    assert_eq!(model["cacheReadTokens"], 875_306);

    let expected_cost = 2_124.0 * 4e-6 + 875_306.0 * 0.4e-6 + 57_610.0 * 5e-6 + 11_150.0 * 18e-6;
    let actual_cost = daily["costUSD"].as_f64().unwrap();
    assert!((actual_cost - expected_cost).abs() < 1e-12);
    assert!((report["totals"]["costUSD"].as_f64().unwrap() - expected_cost).abs() < 1e-12);
}

#[test]
fn prices_gpt_reserve_as_gpt_5_6_luna() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let usage = CodexModelUsage {
        total_tokens: 1_100,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 1_000,
                output_tokens: 100,
                ..Default::default()
            },
            Default::default(),
        )
    };

    let reserve_cost = calculate_codex_model_cost(
        "gpt-reserve",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let luna_cost = calculate_codex_model_cost(
        "gpt-5.6-luna",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    assert!(reserve_cost > 0.0);
    assert_eq!(reserve_cost, luna_cost);
    assert!(!codex_model_missing_pricing(
        "gpt-reserve",
        &usage,
        Some(&pricing)
    ));
}

#[test]
fn a_zero_rate_model_is_priced_and_usage_without_tokens_is_not_missing() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{ "free-model": { "litellm_provider": "openai", "input_cost_per_token": 0, "output_cost_per_token": 0 } }"#,
    );
    let usage = CodexModelUsage {
        total_tokens: 100,
        ..CodexModelUsage::default()
    };
    assert!(!codex_model_missing_pricing(
        "free-model",
        &usage,
        Some(&pricing)
    ));
    assert!(codex_model_missing_pricing(
        "unknown-model",
        &usage,
        Some(&pricing)
    ));
    assert!(!codex_model_missing_pricing(
        "unknown-model",
        &CodexModelUsage::default(),
        Some(&pricing)
    ));
}

#[test]
fn reports_codex_model_aliases_without_raw_model_names() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("private-codex-alpha", "gpt-5.5"),
        ("private-codex-beta", "gpt-5.5"),
    ]);
    let pricing = PricingMap::default();
    let report = rollout_report(
        &[
            usage_line(
                "2026-01-02T00:00:00.000Z",
                "private-codex-alpha",
                100,
                10,
                5,
                0,
            ),
            usage_line(
                "2026-01-02T00:00:01.000Z",
                "private-codex-beta",
                50,
                5,
                3,
                0,
            ),
        ],
        AgentReportKind::Daily,
        &pricing,
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    let models = report["daily"][0]["models"].as_object().unwrap();
    assert!(models.contains_key("gpt-5.5"));
    assert!(!models.contains_key("private-codex-alpha"));
    assert!(!models.contains_key("private-codex-beta"));
    assert_eq!(models["gpt-5.5"]["inputTokens"], 135);
    assert_eq!(models["gpt-5.5"]["cacheReadTokens"], 15);
    assert_eq!(models["gpt-5.5"]["outputTokens"], 8);
}

#[test]
fn absent_cache_read_rates_preserve_the_shared_read_fallback() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "litellm_provider": "openai"
                },
                "other-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
    );
    let usage = CodexModelUsage {
        reasoning_output_tokens: 0,
        total_tokens: 105,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 100,
                cached_input_tokens: 40,
                output_tokens: 5,
                ..Default::default()
            },
            Default::default(),
        )
    };
    let cost = |model| {
        calculate_codex_model_cost(
            model,
            &usage,
            Some(&pricing),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        )
    };

    assert!((cost("gpt-test") - (60e-6 + 40e-6 * 0.1 + 50e-6)).abs() < 1e-15);
    assert!((cost("other-test") - (60e-6 + 40e-6 * 0.1 + 50e-6)).abs() < 1e-15);
}

#[test]
fn bills_long_context_codex_requests_at_long_context_rates() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-long": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.00003,
                    "cache_read_input_token_cost": 0.0000005,
                    "input_cost_per_token_above_200k_tokens": 0.00001,
                    "output_cost_per_token_above_200k_tokens": 0.000045,
                    "cache_read_input_token_cost_above_200k_tokens": 0.000001
                }
            }"#,
    );
    let usage = CodexModelUsage {
        total_tokens: 351_000,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 350_000,
                cached_input_tokens: 50_000,
                output_tokens: 1_000,
                long_context_input_tokens: 300_000,
                long_context_cached_input_tokens: 40_000,
                long_context_output_tokens: 800,
                ..Default::default()
            },
            Default::default(),
        )
    };

    let cost = calculate_codex_model_cost(
        "gpt-long",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    let expected = 40_000.0 * 5e-6
        + 10_000.0 * 0.5e-6
        + 200.0 * 30e-6
        + 260_000.0 * 10e-6
        + 40_000.0 * 1e-6
        + 800.0 * 45e-6;
    assert!((cost - expected).abs() < 1e-9);
}

#[test]
fn prices_mixed_speed_and_long_context_buckets_independently() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-long": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.00003,
                    "cache_read_input_token_cost": 0.0000005,
                    "input_cost_per_token_above_200k_tokens": 0.00001,
                    "output_cost_per_token_above_200k_tokens": 0.000045,
                    "cache_read_input_token_cost_above_200k_tokens": 0.000001,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let usage = CodexModelUsage {
        total_tokens: 351_000,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 350_000,
                cached_input_tokens: 50_000,
                output_tokens: 1_000,
                long_context_input_tokens: 300_000,
                long_context_cached_input_tokens: 40_000,
                long_context_output_tokens: 800,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Standard] = CodexUsageBucket {
                    input_tokens: 50_000,
                    cached_input_tokens: 10_000,
                    output_tokens: 200,
                    ..CodexUsageBucket::default()
                };
                tiers[crate::CodexServiceTier::Fast] = CodexUsageBucket {
                    input_tokens: 300_000,
                    cached_input_tokens: 40_000,
                    cache_creation_tokens: 0,
                    output_tokens: 800,
                    long_context_input_tokens: 300_000,
                    long_context_cached_input_tokens: 40_000,
                    long_context_cache_creation_tokens: 0,
                    long_context_output_tokens: 800,
                };
                tiers
            },
        )
    };

    let cost = calculate_codex_model_cost(
        "gpt-long",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
    );

    let standard_cost = 40_000.0 * 5e-6 + 10_000.0 * 0.5e-6 + 200.0 * 30e-6;
    let fast_base_cost = 260_000.0 * 10e-6 + 40_000.0 * 1e-6 + 800.0 * 45e-6;
    assert!((cost - (standard_cost + fast_base_cost * 2.0)).abs() < 1e-9);
}

#[test]
fn long_context_split_without_tier_rates_matches_flat_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.00001
                }
            }"#,
    );
    let flat = CodexModelUsage {
        total_tokens: 402_000,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 400_000,
                cached_input_tokens: 100_000,
                output_tokens: 2_000,
                ..Default::default()
            },
            Default::default(),
        )
    };
    let split = CodexModelUsage {
        usage: crate::CodexUsageBucket {
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 80_000,
            long_context_output_tokens: 1_500,
            ..flat.clone().usage
        },
        ..flat.clone()
    };

    let flat_cost = calculate_codex_model_cost(
        "gpt-test",
        &flat,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let split_cost = calculate_codex_model_cost(
        "gpt-test",
        &split,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );

    assert!((flat_cost - split_cost).abs() < f64::EPSILON);
}

#[test]
fn applies_speed_option_to_codex_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                }
            }"#,
    );
    let usage = CodexModelUsage {
        reasoning_output_tokens: 0,
        total_tokens: 105,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 100,
                cached_input_tokens: 40,
                output_tokens: 5,
                ..Default::default()
            },
            Default::default(),
        )
    };

    let standard = calculate_codex_model_cost(
        "gpt-5.3-codex",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let fast = calculate_codex_model_cost(
        "gpt-5.3-codex",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
    );

    assert!((fast - (standard * 2.0)).abs() < f64::EPSILON);
}

#[test]
fn applies_gpt_6_astra_fast_multiplier_without_changing_standard_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let usage = CodexModelUsage {
        total_tokens: 1_000_000,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 1_000_000,
                cached_input_tokens: 1_000_000,
                long_context_input_tokens: 1_000_000,
                long_context_cached_input_tokens: 1_000_000,
                ..Default::default()
            },
            Default::default(),
        )
    };

    let standard = calculate_codex_model_cost(
        "gpt-6-astra",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let fast = calculate_codex_model_cost(
        "gpt-6-astra",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
    );

    assert!((standard - 2.0).abs() < f64::EPSILON);
    assert!((fast - 4.0).abs() < f64::EPSILON);
}

#[test]
fn applies_documented_gpt_6_fast_multipliers() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let usage = CodexModelUsage {
        total_tokens: 1_100_000,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 1_000_000,
                cached_input_tokens: 500_000,
                output_tokens: 100_000,
                ..Default::default()
            },
            Default::default(),
        )
    };

    for model in ["gpt-6-sol", "gpt-6.1-sol", "gpt-6-luna"] {
        let standard = calculate_codex_model_cost(
            model,
            &usage,
            Some(&pricing),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        );
        let fast = calculate_codex_model_cost(
            model,
            &usage,
            Some(&pricing),
            CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
        );

        assert!(standard > 0.0, "{model} has embedded pricing");
        assert!((fast - (standard * 2.0)).abs() < 1e-9, "{model}");
    }
}

#[test]
fn uses_recorded_service_tiers_in_auto_mode() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let usage = CodexModelUsage {
        total_tokens: 20,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 20,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Standard] = CodexUsageBucket {
                    input_tokens: 10,
                    ..CodexUsageBucket::default()
                };
                tiers[crate::CodexServiceTier::Fast] = CodexUsageBucket {
                    input_tokens: 10,
                    ..CodexUsageBucket::default()
                };
                tiers
            },
        )
    };

    let auto = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
    );
    let forced_standard = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let forced_fast = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
    );

    assert!((auto - 30e-6).abs() < f64::EPSILON);
    assert!((forced_standard - 20e-6).abs() < f64::EPSILON);
    assert!((forced_fast - 40e-6).abs() < f64::EPSILON);
}

#[test]
fn prices_recorded_flex_usage_at_flex_rate() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "flex": 0.5 }
                }
            }"#,
    );
    let usage = CodexModelUsage {
        total_tokens: 20,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 20,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Flex] = CodexUsageBucket {
                    input_tokens: 20,
                    ..CodexUsageBucket::default()
                };
                tiers
            },
        )
    };

    let cost = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
    );

    assert!((cost - 10e-6).abs() < f64::EPSILON);
}

#[test]
fn prices_recorded_ultrafast_usage_at_published_rates() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let ultrafast = |usage: CodexUsageBucket| CodexModelUsage {
        total_tokens: usage.input_tokens + usage.output_tokens,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                long_context_input_tokens: usage.long_context_input_tokens,
                long_context_cached_input_tokens: usage.long_context_cached_input_tokens,
                long_context_output_tokens: usage.long_context_output_tokens,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Ultrafast] = usage;
                tiers
            },
        )
    };
    let short = ultrafast(CodexUsageBucket {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 50,
        ..CodexUsageBucket::default()
    });
    let long = ultrafast(CodexUsageBucket {
        input_tokens: 300_000,
        cached_input_tokens: 250_000,
        output_tokens: 1_000,
        long_context_input_tokens: 300_000,
        long_context_cached_input_tokens: 250_000,
        long_context_output_tokens: 1_000,
        ..CodexUsageBucket::default()
    });

    // NOTE(2026-10-30, #1989): OpenAI published Ultrafast rates for GPT-6.1 Sol and GPT-6 Astra. The expected costs use those rates.
    for (model, short_cost, long_cost) in [
        ("gpt-6.1-sol", 0.01512, 1.59),
        ("gpt-6-astra", 0.0762, 9.45),
    ] {
        for (usage, expected) in [(&short, short_cost), (&long, long_cost)] {
            let cost = calculate_codex_model_cost(
                model,
                usage,
                Some(&pricing),
                CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
            );
            assert!((cost - expected).abs() < 1e-12, "{model}: {cost}");
        }
    }
}

#[test]
fn keeps_standard_pricing_for_ultrafast_without_a_published_rate() {
    let pricing = ultrafast_test_pricing();
    let usage = CodexModelUsage {
        total_tokens: 1_250,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 1_200,
                cached_input_tokens: 200,
                output_tokens: 50,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Ultrafast] = CodexUsageBucket {
                    input_tokens: 1_200,
                    cached_input_tokens: 200,
                    output_tokens: 50,
                    ..CodexUsageBucket::default()
                };
                tiers
            },
        )
    };

    for model in ["gpt-6-sol", "gpt-6.1-terra", "gpt-6-luna"] {
        let standard = calculate_codex_model_cost(
            model,
            &usage,
            Some(&pricing),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        );
        let auto = calculate_codex_model_cost(
            model,
            &usage,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
        );

        assert!(standard > 0.0, "{model} has pricing");
        assert!((auto - standard).abs() < f64::EPSILON, "{model}");
    }
}

fn ultrafast_test_pricing() -> PricingMap {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    let entry = |input: f64, output: f64| {
        json!({
            "input_cost_per_token": input,
            "output_cost_per_token": output,
            "cache_read_input_token_cost": input / 10.0,
        })
    };
    let loaded = pricing.load_json(
        &json!({
            "gpt-6.1-sol": entry(2e-6, 10e-6),
            "gpt-6.1-terra": entry(1e-6, 5e-6),
            "gpt-6-sol": entry(1.5e-6, 7.5e-6),
            "gpt-6-luna": entry(0.5e-6, 2.5e-6),
        })
        .to_string(),
    );
    assert_eq!(loaded, 4);
    pricing
}

#[test]
fn prices_ultrafast_from_the_same_entry_as_standard_rates() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("private-sol", "gpt-6.1-sol"),
        ("gpt-6.1-terra", "gpt-6.1-sol"),
    ]);
    let pricing = ultrafast_test_pricing();
    let usage = CodexModelUsage {
        total_tokens: 1_250,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 1_200,
                cached_input_tokens: 200,
                output_tokens: 50,
                ..Default::default()
            },
            {
                let mut tiers = crate::TierBuckets::default();
                tiers[crate::CodexServiceTier::Ultrafast] = CodexUsageBucket {
                    input_tokens: 1_200,
                    cached_input_tokens: 200,
                    output_tokens: 50,
                    ..CodexUsageBucket::default()
                };
                tiers
            },
        )
    };
    let cost = |model: &str, speed: CodexSpeedPolicy| {
        calculate_codex_model_cost(model, &usage, Some(&pricing), speed)
    };

    let private_sol = cost(
        "private-sol",
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
    );
    let standard_sol = cost(
        "gpt-6.1-sol",
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    assert!(standard_sol > 0.0);
    assert!(
        (private_sol - standard_sol * 6.0).abs() < 1e-12,
        "{private_sol}"
    );

    let terra = cost(
        "gpt-6.1-terra",
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
    );
    let standard_terra = cost(
        "gpt-6.1-terra",
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    assert!(
        (standard_terra - standard_sol).abs() < f64::EPSILON,
        "{standard_terra}"
    );
    assert!((terra - private_sol).abs() < f64::EPSILON, "{terra}");
}

#[test]
fn prices_rollout_service_tiers_across_loading_and_report_modes() {
    ccusage_test_support::install_pricing_snapshots();
    for (model, expected_costs) in [
        ("gpt-test", [55e-6, 50e-6, 65e-6, 25e-6, 50e-6, 100e-6]),
        (
            "deepseek-v4-flash",
            [12.1e-6, 11e-6, 14.3e-6, 5.5e-6, 11e-6, 22e-6],
        ),
        (
            "gpt-6.1-sol",
            [105e-6, 100e-6, 115e-6, 25e-6, 50e-6, 100e-6],
        ),
    ] {
        let rollout = |second: u8, tier: &str| {
            let timestamp = format!("2026-08-16T17:00:{second:02}.000Z");
            [
                json!({
                    "timestamp": timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": tier },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": model,
                            "last_token_usage": {
                                "input_tokens": 10,
                                "output_tokens": 0,
                                "total_tokens": 10,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n")
        };
        let fixture = fs_fixture!({
            "sessions/standard.jsonl": rollout(1, "default"),
            "sessions/flex.jsonl": rollout(2, "flex"),
            "sessions/fast.jsonl": rollout(3, "priority"),
            "sessions/unclassified.jsonl": rollout(4, "unknown"),
            "sessions/ultrafast.jsonl": rollout(5, "ultrafast"),
        });
        let mut pricing = PricingMap::default();
        pricing.load_json(
            &json!({
                model: {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "litellm_provider": if model == "deepseek-v4-flash" { "deepseek" } else { "openai" },
                    "provider_specific_entry": { "fast": 2, "flex": 0.5 },
                },
            })
            .to_string(),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            for kind in [
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ] {
                let groups =
                    load_groups_from_directory(&fixture.path("sessions"), &shared, kind).unwrap();
                for (speed, expected) in [
                    (
                        CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
                        expected_costs[0],
                    ),
                    (
                        CodexSpeedPolicy::Auto(CodexServiceTier::Flex),
                        expected_costs[1],
                    ),
                    (
                        CodexSpeedPolicy::Auto(CodexServiceTier::Fast),
                        expected_costs[2],
                    ),
                    (
                        CodexSpeedPolicy::Forced(CodexServiceTier::Flex),
                        expected_costs[3],
                    ),
                    (
                        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
                        expected_costs[4],
                    ),
                    (
                        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
                        expected_costs[5],
                    ),
                ] {
                    let report = report_from_groups(
                        &groups,
                        kind,
                        Some(&pricing),
                        speed,
                        CostMode::Calculate,
                    );
                    assert_eq!(report["totals"]["totalTokens"], 50);
                    let actual = report["totals"]["costUSD"].as_f64().unwrap();
                    assert!(
                        (actual - expected).abs() < 1e-12,
                        "{model}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn config_fallback_applies_only_to_unclassified_usage() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let mut recorded = TierBuckets::default();
    recorded[CodexServiceTier::Standard].input_tokens = 10;
    recorded[CodexServiceTier::Fast].input_tokens = 10;
    let usage = CodexModelUsage {
        total_tokens: 30,
        ..CodexModelUsage::from_buckets(
            CodexUsageBucket {
                input_tokens: 30,
                ..CodexUsageBucket::default()
            },
            recorded,
        )
    };
    let speed = CodexSpeedPolicy::Auto(CodexServiceTier::Fast);

    let cost = calculate_codex_model_cost("gpt-test", &usage, Some(&pricing), speed);

    assert!((cost - 50e-6).abs() < f64::EPSILON);
}

#[test]
fn standard_config_fallback_leaves_unclassified_usage_at_standard_rate() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let mut recorded = TierBuckets::default();
    recorded[CodexServiceTier::Standard].input_tokens = 10;
    recorded[CodexServiceTier::Fast].input_tokens = 10;
    let usage = CodexModelUsage {
        total_tokens: 30,
        ..CodexModelUsage::from_buckets(
            CodexUsageBucket {
                input_tokens: 30,
                ..CodexUsageBucket::default()
            },
            recorded,
        )
    };
    let speed = CodexSpeedPolicy::Auto(CodexServiceTier::Standard);

    let cost = calculate_codex_model_cost("gpt-test", &usage, Some(&pricing), speed);

    assert!((cost - 40e-6).abs() < f64::EPSILON);
}

#[test]
fn does_not_assume_fast_pricing_without_a_model_multiplier() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002
                }
            }"#,
    );
    let usage = CodexModelUsage {
        reasoning_output_tokens: 0,
        total_tokens: 105,
        ..CodexModelUsage::from_buckets(
            crate::CodexUsageBucket {
                input_tokens: 100,
                cached_input_tokens: 40,
                output_tokens: 5,
                ..Default::default()
            },
            Default::default(),
        )
    };

    let standard = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    let fast = calculate_codex_model_cost(
        "gpt-test",
        &usage,
        Some(&pricing),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
    );

    assert!((fast - standard).abs() < f64::EPSILON);
}

#[test]
fn identifies_codex_models_missing_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-known": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
    );
    let mut group = crate::CodexGroup::default();
    group.models.insert(
        "gpt-known".to_string(),
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
    group.models.insert(
        "gpt-unknown".to_string(),
        CodexModelUsage {
            total_tokens: 210,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 200,
                    output_tokens: 10,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    let groups = BTreeMap::from([("2026-01-02".to_string(), group)]);

    assert_eq!(
        report::codex_missing_pricing_models(&groups, Some(&pricing)),
        vec!["gpt-unknown".to_string()]
    );
}

#[test]
fn snapshots_codex_reports_for_periods_sessions_costs_and_fallback_models() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                },
                "gpt-5-mini": {
                    "input_cost_per_token": 0.00000025,
                    "output_cost_per_token": 0.000002
                }
            }"#,
    );
    let usage = |input: u64, cached: u64, output: u64, reasoning: u64, total: u64| {
        json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
            "reasoning_output_tokens": reasoning,
            "total_tokens": total,
        })
    };
    let session_a = [
        token_count_line(
            "2026-02-06T00:00:00.000Z",
            "gpt-5.3-codex",
            usage(140, 40, 5, 2, 147),
        ),
        token_count_line(
            "2026-02-06T00:05:00.000Z",
            "codex-auto-review",
            usage(70, 70, 10, 0, 80),
        ),
    ];
    let session_b = [token_count_line(
        "2026-02-09T23:59:59.000Z",
        "gpt-5-mini",
        usage(10, 0, 2, 0, 12),
    )];
    let report = |kind, speed| {
        let groups = load_rollouts(
            &[
                ("workspace/api/session-a.jsonl", &session_a),
                ("workspace/web/session-b.jsonl", &session_b),
            ],
            kind,
        );
        report_from_groups(&groups, kind, Some(&pricing), speed, CostMode::Calculate)
    };
    let standard = CodexSpeedPolicy::Forced(CodexServiceTier::Standard);

    insta::assert_json_snapshot!(serde_json::json!({
        "daily": report(AgentReportKind::Daily, standard),
        "weekly": report(AgentReportKind::Weekly, standard),
        "monthly": report(AgentReportKind::Monthly, standard),
        "sessionFast": report(
            AgentReportKind::Session,
            CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
        ),
    }));
}
