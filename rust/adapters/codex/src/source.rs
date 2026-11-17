use ccusage_core::Diagnostics;
use serde_json::json;

use ccusage_adapter_common::{
    rows::{RowUsage, SourceRow},
    source::{LoadRequest, LoadedRows, UsageSource},
};
use ccusage_core::{ModelBreakdown, PricingMap, Result, cli::CostMode, progress::UsageLoadAgent};

use crate::{
    CodexGroup, calculate_codex_model_cost, calculate_group_cost, codex_model_missing_pricing,
    non_cached_input_tokens, resolve_codex_speed,
};

pub struct Codex;

pub static SOURCE: Codex = Codex;

impl UsageSource for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows> {
        let load = || load_rows(request);
        request
            .progress
            .run(UsageLoadAgent("Codex"), request.shared.json, load)
    }
}

fn load_rows(request: &LoadRequest<'_>) -> Result<LoadedRows> {
    let diagnostics = Diagnostics::new("codex");
    let shared = request.shared;
    let pending = request.pricing_or_load()?;
    let sources = crate::paths::codex_usage_sources()?;
    let pricing = pending.get()?;
    let speed = resolve_codex_speed(request.options.codex_speed);

    let (groups, detected) = crate::aggregate::load_groups_from_sources_with_diagnostics(
        &sources,
        shared,
        request.kind.report_kind(),
        pricing,
        &diagnostics,
    )?;
    let rows = groups
        .iter()
        .map(|(period, group)| unified_row_from_group(period, group, pricing, speed, shared.mode))
        .collect();

    Ok(LoadedRows {
        rows,
        detected,
        diagnostics,
    })
}

pub fn unified_row_from_group(
    period: &str,
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: crate::CodexSpeedPolicy,
    mode: CostMode,
) -> SourceRow {
    let pricing = pricing.filter(|_| mode != CostMode::Display);
    let mut model_breakdowns = group
        .models
        .iter()
        .map(|(model, usage)| {
            let input = non_cached_input_tokens(
                usage.usage.input_tokens,
                usage.usage.cached_input_tokens,
                usage.usage.cache_creation_tokens,
            );
            ModelBreakdown {
                model_name: model.clone(),
                input_tokens: input,
                output_tokens: usage.usage.output_tokens,
                cache_creation_tokens: usage.usage.cache_creation_tokens,
                cache_read_tokens: usage.usage.cached_input_tokens,
                cache_creation_1h_tokens: None,
                extra_total_tokens: 0,
                cost: calculate_codex_model_cost(model, usage, pricing, speed),
                missing_pricing: mode != CostMode::Display
                    && codex_model_missing_pricing(model, usage, pricing),
                stored_cost: None,
            }
        })
        .collect::<Vec<_>>();
    model_breakdowns.sort_by(|left, right| right.cost.total_cmp(&left.cost));

    SourceRow {
        agent: "codex",
        metadata: Some(json!({
            "lastActivity": group.last_activity,
            "reasoningOutputTokens": group.reasoning_output_tokens,
        })),
        usage: RowUsage {
            period: period.to_string(),
            models_used: group.models.keys().cloned().collect(),
            input_tokens: non_cached_input_tokens(
                group.input_tokens,
                group.cached_input_tokens,
                group.cache_creation_tokens,
            ),
            output_tokens: group.output_tokens,
            cache_creation_tokens: group.cache_creation_tokens,
            cache_read_tokens: group.cached_input_tokens,
            total_tokens: group.total_tokens,
            total_cost: calculate_group_cost(group, pricing, speed),
            model_breakdowns,
        },
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use crate::aggregate::load_groups_with_diagnostics;
    use ccusage_adapter_common::source::{ProgressOwner, SourceOptions};
    use ccusage_core::cli::{AgentReportKind, SharedArgs};
    use ccusage_test_support::{EnvVarsGuard, Fixture};

    #[test]
    fn pending_pricing_is_resolved_before_codex_tier_aggregation() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let _ = fixture.write_file("sessions/long.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.5","last_token_usage":{"input_tokens":250000,"output_tokens":10,"total_tokens":250010}}}}"#);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().into())),
            ("LOG_LEVEL", Some("0".into())),
        ]);
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some("UTC".into()),
            ..Default::default()
        };
        for kind in [
            ccusage_adapter_common::source::LoadKind::Day,
            ccusage_adapter_common::source::LoadKind::Session,
        ] {
            let loaded = SOURCE
                .load_rows(&LoadRequest {
                    kind,
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert_eq!(loaded.rows.len(), 1);
            assert_eq!(loaded.rows[0].total_tokens, 250010);
            assert!((loaded.rows[0].total_cost - 1.2503).abs() < 1e-12);
        }
    }

    fn unified_codex_rows(shared: &SharedArgs, kind: AgentReportKind) -> Vec<SourceRow> {
        SOURCE
            .load_rows(&LoadRequest {
                kind: ccusage_adapter_common::source::LoadKind::for_report(kind),
                shared,
                options: SourceOptions::default(),
                pricing: Some(&PricingMap::load_embedded()),
                progress: ProgressOwner::Caller,
            })
            .unwrap()
            .rows
    }

    #[test]
    fn display_reports_tokens_without_calculating_cost_from_supplied_rates() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let _ = fixture.write_file("sessions/tiny.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}"#);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("0".into())),
        ]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };

        let rows = unified_codex_rows(&shared, AgentReportKind::Daily);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total_tokens, 110);
        assert_eq!(rows[0].total_cost, 0.0);
        assert_eq!(rows[0].model_breakdowns[0].cost, 0.0);
        assert!(!rows[0].model_breakdowns[0].missing_pricing);
    }

    #[test]
    fn loads_pricing_when_the_caller_does_not_supply_it() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let usage = json!({"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}).to_string();
        let _ = fixture.write_file("sessions/session.jsonl", usage);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("0".into())),
        ]);
        let shared = SharedArgs {
            offline: true,
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let loaded = SOURCE
            .load_rows(&LoadRequest {
                kind: ccusage_adapter_common::source::LoadKind::for_report(AgentReportKind::Daily),
                shared: &shared,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
            })
            .unwrap();
        assert_eq!(loaded.rows.len(), 1);
        assert_eq!(loaded.rows[0].total_tokens, 110);
        assert!(loaded.rows[0].total_cost > 0.0);
    }

    #[test]
    fn bounded_unified_session_report_keeps_identical_requests_of_two_sessions() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let usage = json!({"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}).to_string();
        let _ = fixture.write_file("sessions/session-a.jsonl", &usage);
        let _ = fixture.write_file("sessions/session-b.jsonl", &usage);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("0".into())),
        ]);
        let shared = SharedArgs {
            offline: true,
            timezone: Some("UTC".to_owned()),
            since: Some("20260101".to_owned()),
            ..Default::default()
        };

        let rows = unified_codex_rows(&shared, AgentReportKind::Session);

        assert_eq!(
            rows.iter()
                .map(|row| row.period.as_str())
                .collect::<Vec<_>>(),
            ["session-a", "session-b"]
        );
        assert!(rows.iter().all(|row| row.total_tokens == 110));
    }

    #[test]
    fn bounded_base_loads_match_focused_codex_daily_and_session_rows() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let usage = |timestamp: &str| {
            json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}).to_string()
        };
        let _ = fixture.write_file("sessions/original.jsonl", usage("2026-01-02T00:00:00.000Z"));
        let _ = fixture.write_file("sessions/copy.jsonl", usage("2026-01-02T00:00:00Z"));
        let _ = fixture.write_file("sessions/later.jsonl", usage("2026-01-03T00:00:00.000Z"));
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("0".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        let speed = crate::CodexSpeedPolicy::Forced(crate::CodexServiceTier::Standard);
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            let shared = SharedArgs {
                offline: true,
                timezone: Some("UTC".to_owned()),
                since: Some("20260102".to_owned()),
                until: Some("20260102".to_owned()),
                ..Default::default()
            };

            let focused = crate::load_groups(&shared, kind)
                .unwrap()
                .iter()
                .map(|(period, group)| {
                    unified_row_from_group(period, group, Some(&pricing), speed, shared.mode)
                })
                .map(|row| (row.usage.period, row.usage.total_tokens))
                .collect::<Vec<_>>();
            let unified = unified_codex_rows(&shared, kind)
                .into_iter()
                .map(|row| (row.usage.period, row.usage.total_tokens))
                .collect::<Vec<_>>();

            assert_eq!(unified, focused, "{kind:?}");
            if kind != AgentReportKind::Session {
                assert_eq!(unified, [(focused[0].0.clone(), 110)], "{kind:?}");
            }
        }
    }

    #[test]
    fn malformed_compaction_usage_warns_while_missing_and_zero_usage_stay_quiet() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let mut records = [
            ("array", json!([])),
            ("string", json!("SECRET_CANARY")),
            ("null", serde_json::Value::Null),
            ("zero", json!({})),
            ("healthy", json!({"input_tokens":10,"output_tokens":2,"total_tokens":12})),
        ].into_iter().flat_map(|(response_id, usage)| [
            json!({"type":"token_usage_record","timestamp":"2026-01-02T00:00:00Z","payload":{"response_id":response_id,"usage":usage}}),
            json!({"type":"compacted","payload":{"compaction_response_id":response_id}}),
        ]).collect::<Vec<_>>();
        records.extend([
            json!({"type":"token_usage_record","timestamp":"2026-01-02T00:00:00Z","payload":{"response_id":"missing"}}),
            json!({"type":"compacted","payload":{"compaction_response_id":"missing"}}),
        ]);
        let contents = records
            .into_iter()
            .map(|record| record.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fixture.write_file("sessions/compaction.jsonl", &contents);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        for since in [None, Some("20260101".to_owned())] {
            let shared = SharedArgs {
                offline: true,
                debug: true,
                timezone: Some("UTC".to_owned()),
                since,
                ..Default::default()
            };
            let result = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: Some(&pricing),
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0].input_tokens, 10);
            assert_eq!(result.rows[0].output_tokens, 2);
            assert_eq!(result.rows[0].total_tokens, 12);
            assert!(
                result
                    .diagnostics
                    .warnings(&shared)
                    .iter()
                    .any(|line| line == "WARN  codex: skipped 2 records in 1 input (invalid data)"),
                "{:?}",
                result.diagnostics.warnings(&shared)
            );
            assert!(
                !result
                    .diagnostics
                    .warnings(&shared)
                    .join("\n")
                    .contains("SECRET_CANARY")
            );
        }
    }

    #[test]
    fn rejected_timestamps_advance_cumulative_usage_without_recounting_tokens() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        for (invalid_timestamp, category) in [
            (json!("SECRET_CANARY"), "invalid timestamp"),
            (serde_json::Value::Null, "invalid timestamp"),
            (json!({"SECRET_CANARY": true}), "invalid data"),
        ] {
            let records = [
                json!({"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}}),
                json!({"timestamp":invalid_timestamp,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"output_tokens":22,"total_tokens":132}}}}),
                json!({"timestamp":"2026-01-02T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":115,"output_tokens":23,"total_tokens":138}}}}),
            ].map(|record| record.to_string()).join("\n");
            let _ = fixture.write_file("sessions/cumulative.jsonl", &records);
            for since in [None, Some("20260101".to_owned())] {
                let shared = SharedArgs {
                    offline: true,
                    debug: true,
                    timezone: Some("UTC".to_owned()),
                    since,
                    ..Default::default()
                };
                let result = SOURCE
                    .load_rows(&LoadRequest {
                        kind: ccusage_adapter_common::source::LoadKind::for_report(
                            AgentReportKind::Daily,
                        ),
                        shared: &shared,
                        options: SourceOptions::default(),
                        pricing: Some(&pricing),
                        progress: ProgressOwner::Caller,
                    })
                    .unwrap();
                assert_eq!(result.rows.len(), 1);
                assert_eq!(result.rows[0].input_tokens, 15);
                assert_eq!(result.rows[0].output_tokens, 3);
                assert_eq!(result.rows[0].total_tokens, 18);
                assert!(result.diagnostics.warnings(&shared).iter().any(|line| line
                    == &format!("WARN  codex: skipped 1 record in 1 input ({category})")));
                assert!(
                    !result
                        .diagnostics
                        .warnings(&shared)
                        .join("\n")
                        .contains("SECRET_CANARY")
                );
            }
        }
    }

    #[test]
    fn discovered_nonusage_files_remain_detected_in_all_load_paths() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        for file in ["sessions/header.jsonl", "archived_sessions/header.jsonl"] {
            let _ = fixture.write_file(
                file,
                r#"{"type":"session_meta","payload":{"id":"metadata-only"}}"#,
            );
            for since in [None, Some("20260101".to_owned())] {
                let shared = SharedArgs {
                    offline: true,
                    json: true,
                    timezone: Some("UTC".to_owned()),
                    since,
                    ..Default::default()
                };
                let result = SOURCE
                    .load_rows(&LoadRequest {
                        kind: ccusage_adapter_common::source::LoadKind::for_report(
                            AgentReportKind::Daily,
                        ),
                        shared: &shared,
                        options: SourceOptions::default(),
                        pricing: Some(&pricing),
                        progress: ProgressOwner::Caller,
                    })
                    .unwrap();
                assert!(result.detected);
                assert!(result.rows.is_empty());
                assert!(result.diagnostics.warnings(&shared).is_empty());
            }
        }
    }

    #[test]
    fn invalid_session_timestamps_keep_healthy_usage_with_and_without_date_bounds() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let records = [
            json!({"timestamp":"SECRET_CANARY","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":999,"output_tokens":99,"total_tokens":1098}}}}),
            json!({"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}}}}),
            json!({"timestamp":"SECRET_CANARY","type":"token_usage_record","payload":{"response_id":"invalid-response","usage":{"input_tokens":999,"output_tokens":99,"total_tokens":1098}}}),
        ].map(|record| record.to_string()).join("\n");
        let _ = fixture.write_file("sessions/timestamps.jsonl", &records);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        for since in [None, Some("20260101".to_owned())] {
            let shared = SharedArgs {
                offline: true,
                debug: true,
                timezone: Some("UTC".to_owned()),
                since,
                ..Default::default()
            };
            let result = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: Some(&pricing),
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0].input_tokens, 10);
            assert_eq!(result.rows[0].output_tokens, 1);
            assert_eq!(result.rows[0].total_tokens, 11);
            assert!(
                result
                    .diagnostics
                    .warnings(&shared)
                    .iter()
                    .any(|line| line
                        == "WARN  codex: skipped 2 records in 1 input (invalid timestamp)"),
                "{:?}",
                result.diagnostics.warnings(&shared)
            );
            assert!(
                !result
                    .diagnostics
                    .warnings(&shared)
                    .join("\n")
                    .contains("SECRET_CANARY")
            );
        }
    }

    #[test]
    fn specialized_diagnostics_keep_healthy_codex_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let _ = fixture.write_file("sessions/good.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":1}}}}"#);
        let _ = fixture.write_file("sessions/bad.jsonl", r#"{"timestamp":{"unexpected":"SECRET_CANARY"},"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10}}}}"#);
        let _ = fixture.write_file("sessions/corrupt.jsonl.zst", "SECRET_CANARY");
        let good = std::fs::read_to_string(fixture.path("sessions/good.jsonl"))
            .unwrap()
            .replace("00:00:00Z", "00:01:00Z")
            .replace("input_tokens\":10", "input_tokens\":20")
            .into_bytes();
        let mut interrupted =
            zstd::stream::encode_all([good.as_slice(), b"\n"].concat().as_slice(), 0).unwrap();
        interrupted.extend_from_slice(b"SECRET_CANARY");
        std::fs::write(fixture.path("sessions/interrupted.jsonl.zst"), interrupted).unwrap();
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        for since in [None, Some("2026-01-01".to_owned())] {
            let shared = SharedArgs {
                offline: true,
                debug: true,
                since,
                ..Default::default()
            };
            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: Some(&pricing),
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert_eq!(rows.rows.len(), 1);
            let diagnostics = Diagnostics::new("codex");
            let (groups, _) = load_groups_with_diagnostics(
                &shared,
                AgentReportKind::Daily,
                Some(&crate::PricingMap::load_embedded()),
                &diagnostics,
            )
            .unwrap();
            assert_eq!(
                groups.values().map(|group| group.input_tokens).sum::<u64>(),
                30
            );
            assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::ReadInterrupted),
                2
            );
            assert!(
                rows.diagnostics
                    .warnings(&shared)
                    .iter()
                    .any(|line| line.contains("invalid data")),
                "{:?}",
                rows.diagnostics.warnings(&shared)
            );
            assert!(
                rows.diagnostics
                    .warnings(&shared)
                    .iter()
                    .any(|line| line.contains("read interrupted") || line.contains("unreadable"))
            );
            assert!(
                !rows
                    .diagnostics
                    .warnings(&shared)
                    .join("\n")
                    .contains("SECRET_CANARY")
            );
        }
    }

    #[test]
    fn headless_usage_decode_failures_warn_without_dropping_healthy_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        let records = [
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T00:00:00Z",
                "usage": "SECRET_CANARY",
            })
            .to_string(),
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T00:00:01Z",
                "model": { "unexpected": "shape" },
                "usage": "SECRET_CANARY",
            })
            .to_string(),
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T00:00:02Z",
                "usage": "SECRET_CANARY",
                "data": {
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 2,
                        "total_tokens": 12,
                    }
                },
            })
            .to_string(),
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T00:00:03Z",
                "usage": {},
            })
            .to_string(),
            json!({
                "type": "turn.completed",
                "timestamp": "2026-01-02T00:00:04Z",
                "usage": {
                    "input_tokens": 30,
                    "output_tokens": 3,
                    "total_tokens": 33,
                },
            })
            .to_string(),
        ]
        .join("\n");
        let _ = fixture.write_file("sessions/headless.jsonl", &records);
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.root().as_os_str().to_owned())),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            offline: true,
            debug: true,
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };

        let rows = SOURCE
            .load_rows(&LoadRequest {
                kind: ccusage_adapter_common::source::LoadKind::for_report(AgentReportKind::Daily),
                shared: &shared,
                options: SourceOptions::default(),
                pricing: Some(&pricing),
                progress: ProgressOwner::Caller,
            })
            .unwrap();

        assert_eq!(
            rows.rows.len(),
            1,
            "{:?}",
            rows.diagnostics.warnings(&shared)
        );
        assert_eq!(rows.rows[0].input_tokens, 40);
        assert_eq!(rows.rows[0].output_tokens, 5);
        assert_eq!(rows.rows[0].total_tokens, 45);
        assert!(
            rows.diagnostics
                .warnings(&shared)
                .iter()
                .any(|line| { line == "WARN  codex: skipped 2 records in 1 input (invalid data)" }),
            "{:?}",
            rows.diagnostics.warnings(&shared)
        );
        let warnings = rows.diagnostics.warnings(&shared).join("\n");
        assert!(warnings.contains("headless.jsonl:1:"), "{warnings}");
        assert!(warnings.contains("headless.jsonl:2:"), "{warnings}");
        assert!(!warnings.contains("SECRET_CANARY"), "{warnings}");
    }
}
