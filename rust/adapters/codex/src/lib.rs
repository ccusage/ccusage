use ccusage_adapter_common::{chunk_file_indexes_by_size, collect_usage_files};
use ccusage_core::*;

mod aggregate;
mod loader;
mod parser;
mod paths;
mod replay;
mod report;
mod source;
mod speed;
mod types;

use crate::{PricingMap, Result, cli::AgentCommandArgs, log_level, print_json_or_jq, wants_json};

pub use aggregate::{aggregate_events, filter_events_by_date, load_groups};
#[doc(hidden)]
pub use loader::load_codex_events_from_directory;
pub use loader::load_codex_events_with_detection;
pub use report::{
    calculate_codex_model_cost, calculate_codex_source_cost, calculate_group_cost,
    codex_model_missing_pricing, non_cached_input_tokens,
};
pub use speed::{CodexSpeedPolicy, resolve_codex_speed};
pub use types::{
    CodexGroup, CodexModelUsage, CodexServiceTier, CodexSourceUsage, CodexTimestampedUsage,
    CodexTokenUsageEvent, CodexUsageBucket,
};
pub(crate) use types::{CodexRawUsage, merge_codex_service_tiers};

use report::{print_table_from_groups_with_source, report_from_groups_with_source};

use crate::cli::{AgentReportKind, CodexSpeed};

use serde_json::Value;

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let groups = load_groups(&shared, args.kind)?;
    let speed = resolve_codex_speed(args.codex_speed);
    if wants_json(&shared) {
        let output = if args.by_source {
            report_from_groups_with_source(&groups, args.kind, &pricing, speed, true)
        } else {
            report::report_from_groups(&groups, args.kind, &pricing, speed)
        };
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    if args.by_source {
        print_table_from_groups_with_source(&groups, args.kind, &pricing, speed, &shared, true)
    } else {
        report::print_table_from_groups(&groups, args.kind, &pricing, speed, &shared)
    }
}

#[doc(hidden)]
pub fn report_json(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<Value> {
    report_json_with_source(events, kind, timezone, pricing, speed, false)
}

#[doc(hidden)]
fn report_json_with_source(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
    by_source: bool,
) -> Result<Value> {
    let groups = aggregate_events(events, kind, timezone)?;
    Ok(report_from_groups_with_source(
        &groups,
        kind,
        pricing,
        speed.into(),
        by_source,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::aggregate::load_groups_from_directory;
    use super::report::report_from_groups;
    use super::*;
    use crate::cli::SharedArgs;
    use crate::{CodexModelUsage, CodexServiceTier, CodexTokenUsageEvent, CodexUsageBucket};
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    #[test]
    fn loads_directory_groups_with_date_filter_without_global_event_vector() {
        let fixture = fs_fixture!({
            "sessions/session.jsonl": [
                r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#,
                r#"{"timestamp":"2026-01-03T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":200,"cached_input_tokens":20,"output_tokens":75,"reasoning_output_tokens":5,"total_tokens":280}}}}"#,
            ]
            .join("\n"),
        });
        let sessions_dir = fixture.path("sessions");
        let shared = SharedArgs {
            since: Some("20260103".to_string()),
            timezone: Some("UTC".to_string()),
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
        let usage_line = r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#;
        let fixture = fs_fixture!({
            "sessions/session-a.jsonl": usage_line,
            "sessions/session-b.jsonl": usage_line,
        });
        let sessions_dir = fixture.path("sessions");
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
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
        let pricing = PricingMap::default();
        let report = report_json(
            &[CodexTokenUsageEvent {
                session_id: "session-1".to_string(),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                model: Some("gpt-5".to_string()),
                input_tokens: 100,
                cached_input_tokens: 90,
                cache_creation_tokens: 0,
                output_tokens: 5,
                reasoning_output_tokens: 0,
                total_tokens: 105,
                is_fallback_model: false,
                service_tier: None,
                source: None,
            }],
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

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
    fn reports_opt_in_source_breakdowns_that_conserve_totals() {
        let event = |source: Option<&str>,
                     timestamp: &str,
                     input: u64,
                     cached: u64,
                     creation: u64,
                     output: u64,
                     reasoning: u64,
                     total: u64| CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            timestamp: timestamp.to_string(),
            model: Some("gpt-5".to_string()),
            input_tokens: input,
            cached_input_tokens: cached,
            cache_creation_tokens: creation,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            total_tokens: total,
            is_fallback_model: false,
            service_tier: None,
            source: source.map(str::to_string),
        };
        let events = vec![
            event(
                Some("CLI"),
                "2026-01-02T00:00:00.000Z",
                100,
                20,
                0,
                10,
                1,
                110,
            ),
            event(Some("Exec"), "2026-01-02T00:01:00.000Z", 50, 0, 0, 5, 0, 55),
            event(
                Some("Desktop App"),
                "2026-01-02T00:02:00.000Z",
                25,
                0,
                5,
                2,
                0,
                32,
            ),
            event(None, "2026-01-02T00:03:00.000Z", 10, 0, 0, 1, 0, 11),
            event(
                Some("future-client"),
                "2026-01-02T00:04:00.000Z",
                15,
                0,
                0,
                3,
                2,
                18,
            ),
        ];
        let pricing = PricingMap::default();
        let default = report_json(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();
        let opt_in_default = report_json_with_source(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
            false,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&default).unwrap(),
            serde_json::to_string(&opt_in_default).unwrap()
        );
        assert!(default["daily"][0].get("sourceBreakdowns").is_none());

        let report = report_json_with_source(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
            true,
        )
        .unwrap();
        let row = &report["daily"][0];
        let sources = row["sourceBreakdowns"].as_array().unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|source| source["source"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "CLI",
                "Desktop App",
                "Exec",
                "Uncategorized",
                "future-client"
            ]
        );
        for key in [
            "inputTokens",
            "cacheCreationTokens",
            "cacheReadTokens",
            "outputTokens",
            "reasoningOutputTokens",
            "totalTokens",
        ] {
            assert_eq!(
                sources
                    .iter()
                    .map(|source| source[key].as_u64().unwrap())
                    .sum::<u64>(),
                row[key].as_u64().unwrap(),
                "source totals must conserve {key}"
            );
            assert_eq!(
                sources
                    .iter()
                    .map(|source| source[key].as_u64().unwrap())
                    .sum::<u64>(),
                report["totals"][key].as_u64().unwrap(),
                "row and report totals must agree for {key}"
            );
        }
        insta::assert_json_snapshot!(report);
    }

    #[test]
    fn prices_mixed_deepseek_timestamps_in_model_and_originator_totals() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028,
                    "cache_creation_input_token_cost": 0.000000123,
                    "cache_read_input_token_cost": 0.0000000028
                }
            }"#,
        );
        let event = |timestamp: &str, source: &str| CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            timestamp: timestamp.to_string(),
            model: Some("deepseek-v4-flash".to_string()),
            input_tokens: 1_000_000,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
            is_fallback_model: false,
            service_tier: None,
            source: Some(source.to_string()),
        };
        let events = vec![
            event("2026-08-16T15:59:59.000Z", "codex-tui"),
            event("2026-08-16T16:00:00.000Z", "codex-tui"),
            event("2026-08-17T01:00:00.000Z", "codex_exec"),
        ];

        let report = report_json_with_source(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
            true,
        )
        .unwrap();

        assert!((report["totals"]["costUSD"].as_f64().unwrap() - 0.80).abs() < 1e-12);
        assert!((report["daily"][0]["costUSD"].as_f64().unwrap() - 0.36).abs() < 1e-12);
        assert!((report["daily"][1]["costUSD"].as_f64().unwrap() - 0.44).abs() < 1e-12);

        let sources = report["totals"]["sourceBreakdowns"].as_array().unwrap();
        let cli = sources
            .iter()
            .find(|source| source["source"] == "CLI")
            .unwrap();
        let exec = sources
            .iter()
            .find(|source| source["source"] == "Exec")
            .unwrap();
        assert!((cli["costUSD"].as_f64().unwrap() - 0.36).abs() < 1e-12);
        assert!((exec["costUSD"].as_f64().unwrap() - 0.44).abs() < 1e-12);
    }

    #[test]
    fn reports_codex_cache_write_tokens_and_cost_for_gpt_5_6_terra() {
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
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Daily).unwrap();
        let report = report_from_groups(
            &groups,
            AgentReportKind::Daily,
            &PricingMap::load_embedded(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
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

        let expected_cost =
            2_124.0 * 4e-6 + 875_306.0 * 0.4e-6 + 57_610.0 * 5e-6 + 11_150.0 * 18e-6;
        let actual_cost = daily["costUSD"].as_f64().unwrap();
        assert!((actual_cost - expected_cost).abs() < 1e-12);
        assert!((report["totals"]["costUSD"].as_f64().unwrap() - expected_cost).abs() < 1e-12);
    }

    #[test]
    fn reports_codex_model_aliases_without_raw_model_names() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-codex-alpha", "gpt-5.5"),
            ("private-codex-beta", "gpt-5.5"),
        ]);
        let pricing = PricingMap::default();
        let report = report_json(
            &[
                CodexTokenUsageEvent {
                    session_id: "session-1".to_string(),
                    timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                    model: Some("private-codex-alpha".to_string()),
                    input_tokens: 100,
                    cached_input_tokens: 10,
                    cache_creation_tokens: 0,
                    output_tokens: 5,
                    reasoning_output_tokens: 0,
                    total_tokens: 105,
                    is_fallback_model: false,
                    service_tier: None,
                    source: None,
                },
                CodexTokenUsageEvent {
                    session_id: "session-1".to_string(),
                    timestamp: "2026-01-02T00:00:01.000Z".to_string(),
                    model: Some("private-codex-beta".to_string()),
                    input_tokens: 50,
                    cached_input_tokens: 5,
                    cache_creation_tokens: 0,
                    output_tokens: 3,
                    reasoning_output_tokens: 0,
                    total_tokens: 53,
                    is_fallback_model: false,
                    service_tier: None,
                    source: None,
                },
            ],
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

        let models = report["daily"][0]["models"].as_object().unwrap();
        assert!(models.contains_key("gpt-5.5"));
        assert!(!models.contains_key("private-codex-alpha"));
        assert!(!models.contains_key("private-codex-beta"));
        assert_eq!(models["gpt-5.5"]["inputTokens"], 135);
        assert_eq!(models["gpt-5.5"]["cacheReadTokens"], 15);
        assert_eq!(models["gpt-5.5"]["outputTokens"], 8);
    }

    #[test]
    fn charges_cached_input_at_input_rate_when_codex_pricing_omits_cache_read_rate() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let usage = CodexModelUsage {
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 105,
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Standard);

        assert!((cost - 0.00015).abs() < f64::EPSILON);
    }

    #[test]
    fn bills_long_context_codex_requests_at_long_context_rates() {
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
            input_tokens: 350_000,
            cached_input_tokens: 50_000,
            output_tokens: 1_000,
            total_tokens: 351_000,
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 40_000,
            long_context_output_tokens: 800,
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost("gpt-long", &usage, &pricing, CodexSpeed::Standard);

        // Short bucket: 40K non-cached input, 10K cached, 200 output tokens.
        // Long bucket: 260K non-cached input, 40K cached, 800 output tokens.
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
            input_tokens: 350_000,
            cached_input_tokens: 50_000,
            output_tokens: 1_000,
            total_tokens: 351_000,
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 40_000,
            long_context_output_tokens: 800,
            recorded_standard_usage: CodexUsageBucket {
                input_tokens: 50_000,
                cached_input_tokens: 10_000,
                output_tokens: 200,
                ..CodexUsageBucket::default()
            },
            recorded_fast_usage: CodexUsageBucket {
                input_tokens: 300_000,
                cached_input_tokens: 40_000,
                cache_creation_tokens: 0,
                output_tokens: 800,
                long_context_input_tokens: 300_000,
                long_context_cached_input_tokens: 40_000,
                long_context_cache_creation_tokens: 0,
                long_context_output_tokens: 800,
            },
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost("gpt-long", &usage, &pricing, CodexSpeed::Auto);

        let standard_cost = 40_000.0 * 5e-6 + 10_000.0 * 0.5e-6 + 200.0 * 30e-6;
        let fast_base_cost = 260_000.0 * 10e-6 + 40_000.0 * 1e-6 + 800.0 * 45e-6;
        assert!((cost - (standard_cost + fast_base_cost * 2.0)).abs() < 1e-9);
    }

    #[test]
    fn long_context_split_without_tier_rates_matches_flat_pricing() {
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
            input_tokens: 400_000,
            cached_input_tokens: 100_000,
            output_tokens: 2_000,
            total_tokens: 402_000,
            ..CodexModelUsage::default()
        };
        let split = CodexModelUsage {
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 80_000,
            long_context_output_tokens: 1_500,
            ..flat.clone()
        };

        let flat_cost =
            calculate_codex_model_cost("gpt-test", &flat, &pricing, CodexSpeed::Standard);
        let split_cost =
            calculate_codex_model_cost("gpt-test", &split, &pricing, CodexSpeed::Standard);

        assert!((flat_cost - split_cost).abs() < f64::EPSILON);
    }

    #[test]
    fn prices_gpt_5_6_long_context_usage_from_embedded_pricing() {
        let pricing = PricingMap::load_embedded();
        let usage = CodexModelUsage {
            input_tokens: 300_000,
            cached_input_tokens: 100_000,
            output_tokens: 1_000,
            total_tokens: 301_000,
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 100_000,
            long_context_output_tokens: 1_000,
            ..CodexModelUsage::default()
        };

        let cost =
            calculate_codex_model_cost("gpt-5.6-sol", &usage, &pricing, CodexSpeed::Standard);

        // The whole request is billed at long-context rates: 200K non-cached
        // input at $10/M, 100K cached at $1/M, 1K output at $45/M.
        let expected = 200_000.0 * 10e-6 + 100_000.0 * 1e-6 + 1_000.0 * 45e-6;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn applies_speed_option_to_codex_cost() {
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
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 105,
            ..CodexModelUsage::default()
        };

        let standard =
            calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Standard);
        let fast = calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Fast);

        assert!((fast - (standard * 2.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn uses_recorded_service_tiers_in_auto_mode() {
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
            input_tokens: 20,
            total_tokens: 20,
            recorded_standard_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            recorded_fast_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            ..CodexModelUsage::default()
        };

        let auto = calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Auto);
        let forced_standard =
            calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Standard);
        let forced_fast =
            calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Fast);

        assert!((auto - 30e-6).abs() < f64::EPSILON);
        assert!((forced_standard - 20e-6).abs() < f64::EPSILON);
        assert!((forced_fast - 40e-6).abs() < f64::EPSILON);
    }

    #[test]
    fn config_fallback_applies_only_to_unclassified_usage() {
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
            input_tokens: 30,
            total_tokens: 30,
            recorded_standard_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            recorded_fast_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            ..CodexModelUsage::default()
        };
        let speed = CodexSpeedPolicy::Auto(CodexServiceTier::Fast);

        let cost = calculate_codex_model_cost("gpt-test", &usage, &pricing, speed);

        assert!((cost - 50e-6).abs() < f64::EPSILON);
    }

    #[test]
    fn standard_config_fallback_leaves_unclassified_usage_at_standard_rate() {
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
            input_tokens: 30,
            total_tokens: 30,
            recorded_standard_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            recorded_fast_usage: CodexUsageBucket {
                input_tokens: 10,
                ..CodexUsageBucket::default()
            },
            ..CodexModelUsage::default()
        };
        let speed = CodexSpeedPolicy::Auto(CodexServiceTier::Standard);

        let cost = calculate_codex_model_cost("gpt-test", &usage, &pricing, speed);

        assert!((cost - 40e-6).abs() < f64::EPSILON);
    }

    #[test]
    fn does_not_assume_fast_pricing_without_a_model_multiplier() {
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
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 105,
            ..CodexModelUsage::default()
        };

        let standard =
            calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Standard);
        let fast = calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Fast);

        assert!((fast - standard).abs() < f64::EPSILON);
    }

    #[test]
    fn identifies_codex_models_missing_pricing() {
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
                input_tokens: 100,
                output_tokens: 5,
                total_tokens: 105,
                ..CodexModelUsage::default()
            },
        );
        group.models.insert(
            "gpt-unknown".to_string(),
            CodexModelUsage {
                input_tokens: 200,
                output_tokens: 10,
                total_tokens: 210,
                ..CodexModelUsage::default()
            },
        );
        let groups = BTreeMap::from([("2026-01-02".to_string(), group)]);

        assert_eq!(
            report::codex_missing_pricing_models(&groups, &pricing),
            vec!["gpt-unknown".to_string()]
        );
    }

    #[test]
    fn snapshots_codex_reports_for_periods_sessions_costs_and_fallback_models() {
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
        let events = vec![
            CodexTokenUsageEvent {
                session_id: "/workspace/api/session-a.jsonl".to_string(),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                model: Some("gpt-5.3-codex".to_string()),
                input_tokens: 140,
                cached_input_tokens: 40,
                cache_creation_tokens: 0,
                output_tokens: 5,
                reasoning_output_tokens: 2,
                total_tokens: 147,
                is_fallback_model: false,
                service_tier: None,
                source: None,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/api/session-a.jsonl".to_string(),
                timestamp: "2026-01-02T00:05:00.000Z".to_string(),
                model: Some("gpt-5.3-codex".to_string()),
                input_tokens: 70,
                cached_input_tokens: 70,
                cache_creation_tokens: 0,
                output_tokens: 10,
                reasoning_output_tokens: 0,
                total_tokens: 80,
                is_fallback_model: true,
                service_tier: None,
                source: None,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/web/session-b.jsonl".to_string(),
                timestamp: "2026-01-05T23:59:59.000Z".to_string(),
                model: Some("gpt-5-mini".to_string()),
                input_tokens: 10,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 12,
                is_fallback_model: false,
                service_tier: None,
                source: None,
            },
            CodexTokenUsageEvent {
                session_id: "ignored-missing-model".to_string(),
                timestamp: "2026-01-06T00:00:00.000Z".to_string(),
                model: None,
                input_tokens: 999,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                output_tokens: 999,
                reasoning_output_tokens: 0,
                total_tokens: 1_998,
                is_fallback_model: false,
                service_tier: None,
                source: None,
            },
        ];

        insta::assert_json_snapshot!(serde_json::json!({
            "daily": report_json(
                &events,
                AgentReportKind::Daily,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "weekly": report_json(
                &events,
                AgentReportKind::Weekly,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "monthly": report_json(
                &events,
                AgentReportKind::Monthly,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "sessionFast": report_json(
                &events,
                AgentReportKind::Session,
                Some("UTC"),
                &pricing,
                CodexSpeed::Fast,
            )
            .unwrap(),
        }));
    }
}
