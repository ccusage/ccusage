mod aggregate;
mod loader;
mod parser;
mod paths;
mod report;
mod speed;
mod types;

use crate::{cli::AgentCommandArgs, log_level, print_json_or_jq, wants_json, PricingMap, Result};

pub(crate) use aggregate::{aggregate_events, filter_events_by_date, load_groups};
pub(crate) use loader::load_codex_events;
#[cfg(test)]
pub(crate) use loader::load_codex_events_from_directory;
pub(crate) use report::{
    calculate_codex_model_cost, calculate_group_cost, codex_model_missing_pricing,
    non_cached_input_tokens,
};
pub(crate) use speed::resolve_codex_speed;

use report::{print_table_from_groups, report_from_groups};

#[cfg(test)]
use crate::{
    cli::{AgentReportKind, CodexSpeed},
    CodexTokenUsageEvent,
};

#[cfg(test)]
use serde_json::Value;

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load(shared.offline, log_level() != Some(0));
    let groups = load_groups(&shared, args.kind)?;
    let speed = resolve_codex_speed(args.codex_speed);
    if wants_json(&shared) {
        let output = report_from_groups(&groups, args.kind, &pricing, speed);
        return print_json_or_jq(output, shared.jq.as_deref());
    }
    print_table_from_groups(&groups, args.kind, &pricing, speed, &shared)
}

#[cfg(test)]
pub(crate) fn report_json(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<Value> {
    let groups = aggregate_events(events, kind, timezone)?;
    Ok(report_from_groups(&groups, kind, pricing, speed))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::aggregate::load_groups_from_directory;
    use super::*;
    use crate::cli::SharedArgs;
    use crate::{CodexModelUsage, CodexTokenUsageEvent};
    use ccusage_test_support::fs_fixture;

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
    fn keeps_matching_grouped_codex_usage_events_from_distinct_sessions() {
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
        assert_eq!(group.input_tokens, 200);
        assert_eq!(group.cached_input_tokens, 20);
        assert_eq!(group.output_tokens, 100);
        assert_eq!(group.total_tokens, 300);
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
                output_tokens: 5,
                reasoning_output_tokens: 0,
                total_tokens: 105,
                is_fallback_model: false,
            }],
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

        assert_eq!(report["daily"][0]["inputTokens"], 10);
        assert_eq!(report["daily"][0]["cachedInputTokens"], 90);
        assert_eq!(report["daily"][0]["totalTokens"], 105);
        assert_eq!(report["totals"]["inputTokens"], 10);
        assert_eq!(report["totals"]["cachedInputTokens"], 90);
        assert_eq!(report["totals"]["totalTokens"], 105);
        assert_eq!(report["daily"][0]["models"]["gpt-5"]["inputTokens"], 10);
        assert_eq!(
            report["daily"][0]["models"]["gpt-5"]["cachedInputTokens"],
            90
        );
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
            is_fallback: false,
        };

        let cost = calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Standard);

        assert!((cost - 0.00015).abs() < f64::EPSILON);
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
            is_fallback: false,
        };

        let standard =
            calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Standard);
        let fast = calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Fast);

        assert!((fast - (standard * 2.0)).abs() < f64::EPSILON);
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
    fn expands_multi_model_codex_rows_with_per_model_stats() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.4": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                },
                "gpt-5.5": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000020,
                    "cache_read_input_token_cost": 0.0000002
                }
            }"#,
        );
        let mut group = crate::CodexGroup {
            input_tokens: 300,
            cached_input_tokens: 100,
            output_tokens: 30,
            reasoning_output_tokens: 5,
            total_tokens: 335,
            last_activity: None,
            models: BTreeMap::new(),
        };
        group.models.insert(
            "gpt-5.4".to_string(),
            CodexModelUsage {
                input_tokens: 100,
                cached_input_tokens: 40,
                output_tokens: 10,
                reasoning_output_tokens: 2,
                total_tokens: 112,
                is_fallback: false,
            },
        );
        group.models.insert(
            "gpt-5.5".to_string(),
            CodexModelUsage {
                input_tokens: 200,
                cached_input_tokens: 60,
                output_tokens: 20,
                reasoning_output_tokens: 3,
                total_tokens: 223,
                is_fallback: false,
            },
        );
        let groups = BTreeMap::from([("2026-07-10".to_string(), group)]);
        let report = super::report::report_from_groups(
            &groups,
            AgentReportKind::Daily,
            &pricing,
            CodexSpeed::Standard,
        );

        assert_eq!(
            report["daily"][0]["models"]["gpt-5.5"]["costUSD"]
                .as_f64()
                .unwrap_or_default(),
            calculate_codex_model_cost(
                "gpt-5.5",
                groups["2026-07-10"].models.get("gpt-5.5").unwrap(),
                &pricing,
                CodexSpeed::Standard,
            )
        );
        assert_eq!(report["daily"][0]["models"]["gpt-5.4"]["inputTokens"], 60);
        assert_eq!(report["daily"][0]["models"]["gpt-5.5"]["inputTokens"], 140);

        let shared = SharedArgs {
            no_color: true,
            ..SharedArgs::default()
        };
        // Multi-model table expansion is exercised here so regressions panic
        // instead of silently collapsing per-model stats again.
        super::report::print_table_from_groups(
            &groups,
            AgentReportKind::Daily,
            &pricing,
            CodexSpeed::Standard,
            &shared,
        )
        .unwrap();
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
                output_tokens: 5,
                reasoning_output_tokens: 2,
                total_tokens: 147,
                is_fallback_model: false,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/api/session-a.jsonl".to_string(),
                timestamp: "2026-01-02T00:05:00.000Z".to_string(),
                model: Some("gpt-5.3-codex".to_string()),
                input_tokens: 70,
                cached_input_tokens: 70,
                output_tokens: 10,
                reasoning_output_tokens: 0,
                total_tokens: 80,
                is_fallback_model: true,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/web/session-b.jsonl".to_string(),
                timestamp: "2026-01-05T23:59:59.000Z".to_string(),
                model: Some("gpt-5-mini".to_string()),
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 12,
                is_fallback_model: false,
            },
            CodexTokenUsageEvent {
                session_id: "ignored-missing-model".to_string(),
                timestamp: "2026-01-06T00:00:00.000Z".to_string(),
                model: None,
                input_tokens: 999,
                cached_input_tokens: 0,
                output_tokens: 999,
                reasoning_output_tokens: 0,
                total_tokens: 1_998,
                is_fallback_model: false,
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
