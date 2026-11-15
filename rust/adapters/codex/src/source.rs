use crate::{
    aggregate::load_groups_with_diagnostics, loader::load_codex_events_with_detection_diagnostics,
};
use ccusage_core::Diagnostics;
use serde_json::json;

use ccusage_adapter_common::{
    rows::UnifiedRow,
    source::{ProgressOwner, STANDARD_REPORTS, SourceRows, UnifiedRequest, UsageSource},
};
use ccusage_core::{
    ModelBreakdown, PricingMap, Result,
    cli::{AgentReportKind, CostMode},
    log_level,
    progress::{UsageLoadAgent, track_usage_load},
};

use crate::{
    CodexGroup, aggregate_events, calculate_codex_model_cost, calculate_group_cost,
    codex_model_missing_pricing, filter_events_by_date, non_cached_input_tokens,
    resolve_codex_speed,
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

    fn reports(&self) -> &'static [AgentReportKind] {
        STANDARD_REPORTS
    }

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        let load = || unified_rows(request);
        match request.progress {
            ProgressOwner::Source => {
                track_usage_load(UsageLoadAgent("Codex"), request.shared.json, load)
            }
            ProgressOwner::Caller => load(),
        }
    }
}

fn unified_rows(request: &UnifiedRequest<'_>) -> Result<SourceRows> {
    let diagnostics = Diagnostics::new("codex");
    let shared = request.shared;
    let owned_pricing = request.pricing.is_none().then(|| {
        PricingMap::load_with_overrides(
            shared.offline,
            log_level() != Some(0),
            shared.pricing_overrides.iter(),
        )
    });
    let pricing = request
        .pricing
        .or(owned_pricing.as_ref())
        .expect("Codex pricing is available");
    let speed = resolve_codex_speed(request.options.codex_speed);

    let (rows, detected) = if shared.since.is_none() && shared.until.is_none() {
        let (groups, detected) = load_groups_with_diagnostics(
            shared,
            request.kind,
            request.multi_source_driver,
            &diagnostics,
        )?;
        let rows = groups
            .iter()
            .map(|(period, group)| {
                unified_row_from_group(period, group, pricing, speed, shared.mode)
            })
            .collect();
        (rows, detected)
    } else {
        let (mut events, detected) = load_codex_events_with_detection_diagnostics(
            shared,
            ProgressOwner::Caller,
            &diagnostics,
        )?;
        filter_events_by_date(&mut events, shared)?;
        let groups = aggregate_events(&events, request.kind, shared.timezone.as_deref())?;
        let rows = groups
            .iter()
            .map(|(period, group)| {
                unified_row_from_group(period, group, pricing, speed, shared.mode)
            })
            .collect();
        (rows, detected)
    };

    Ok(SourceRows {
        rows,
        detected,
        warnings: diagnostics.warnings(shared),
        claude_counted: None,
    })
}

pub fn unified_row_from_group<S>(
    period: &str,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: S,
    mode: CostMode,
) -> UnifiedRow
where
    S: Into<crate::CodexSpeedPolicy> + Copy,
{
    let speed = speed.into();
    let mut model_breakdowns = group
        .models
        .iter()
        .map(|(model, usage)| {
            let input = non_cached_input_tokens(
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_creation_tokens,
            );
            ModelBreakdown {
                model_name: model.clone(),
                input_tokens: input,
                output_tokens: usage.output_tokens,
                cache_creation_tokens: usage.cache_creation_tokens,
                cache_read_tokens: usage.cached_input_tokens,
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

    UnifiedRow {
        period: period.to_string(),
        agent: "codex",
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
        metadata: Some(json!({
            "lastActivity": group.last_activity,
            "reasoningOutputTokens": group.reasoning_output_tokens,
        })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns,
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_adapter_common::source::SourceOptions;
    use ccusage_core::cli::SharedArgs;
    use ccusage_test_support::{EnvVarsGuard, Fixture};
    #[test]
    fn rejected_timestamps_advance_cumulative_usage_without_recounting_tokens() {
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
                    .unified_rows(&UnifiedRequest {
                        kind: AgentReportKind::Daily,
                        shared: &shared,
                        options: SourceOptions::default(),
                        pricing: Some(&pricing),
                        progress: ProgressOwner::Caller,
                        multi_source_driver: true,
                        claude_counted: None,
                    })
                    .unwrap();
                assert_eq!(result.rows.len(), 1);
                assert_eq!(result.rows[0].input_tokens, 15);
                assert_eq!(result.rows[0].output_tokens, 3);
                assert_eq!(result.rows[0].total_tokens, 18);
                assert!(result.warnings.iter().any(|line| line
                    == &format!("WARN  codex: skipped 1 record in 1 input ({category})")));
                assert!(!result.warnings.join("\n").contains("SECRET_CANARY"));
            }
        }
    }

    #[test]
    fn discovered_nonusage_files_remain_detected_in_all_load_paths() {
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
                for multi_source_driver in [false, true] {
                    let result = SOURCE
                        .unified_rows(&UnifiedRequest {
                            kind: AgentReportKind::Daily,
                            shared: &shared,
                            options: SourceOptions::default(),
                            pricing: Some(&pricing),
                            progress: ProgressOwner::Caller,
                            multi_source_driver,
                            claude_counted: None,
                        })
                        .unwrap();
                    assert!(result.detected);
                    assert!(result.rows.is_empty());
                    assert!(result.warnings.is_empty());
                }
                let (events, detected) = crate::load_codex_events_with_detection(&shared).unwrap();
                assert!(detected);
                assert!(events.is_empty());
            }
        }
    }

    #[test]
    fn invalid_session_timestamps_keep_healthy_usage_with_and_without_date_bounds() {
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
                .unified_rows(&UnifiedRequest {
                    kind: AgentReportKind::Daily,
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: Some(&pricing),
                    progress: ProgressOwner::Caller,
                    multi_source_driver: true,
                    claude_counted: None,
                })
                .unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0].input_tokens, 10);
            assert_eq!(result.rows[0].output_tokens, 1);
            assert_eq!(result.rows[0].total_tokens, 11);
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|line| line
                        == "WARN  codex: skipped 2 records in 1 input (invalid timestamp)"),
                "{:?}",
                result.warnings
            );
            assert!(!result.warnings.join("\n").contains("SECRET_CANARY"));
        }
    }

    #[test]
    fn specialized_diagnostics_keep_healthy_codex_usage() {
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
                .unified_rows(&UnifiedRequest {
                    kind: AgentReportKind::Daily,
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: Some(&pricing),
                    progress: ProgressOwner::Caller,
                    multi_source_driver: true,
                    claude_counted: None,
                })
                .unwrap();
            assert_eq!(rows.rows.len(), 1);
            let diagnostics = Diagnostics::new("codex");
            let (events, _) = load_codex_events_with_detection_diagnostics(
                &shared,
                ProgressOwner::Caller,
                &diagnostics,
            )
            .unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events.iter().map(|event| event.input_tokens).sum::<u64>(),
                30
            );
            assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::ReadInterrupted),
                2
            );
            assert!(
                rows.warnings
                    .iter()
                    .any(|line| line.contains("invalid data")),
                "{:?}",
                rows.warnings
            );
            assert!(
                rows.warnings
                    .iter()
                    .any(|line| line.contains("read interrupted") || line.contains("unreadable"))
            );
            assert!(!rows.warnings.join("\n").contains("SECRET_CANARY"));
        }
    }

    #[test]
    fn headless_usage_decode_failures_warn_without_dropping_healthy_usage() {
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
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &shared,
                options: SourceOptions::default(),
                pricing: Some(&pricing),
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
            })
            .unwrap();

        assert_eq!(rows.rows.len(), 1, "{:?}", rows.warnings);
        assert_eq!(rows.rows[0].input_tokens, 40);
        assert_eq!(rows.rows[0].output_tokens, 5);
        assert_eq!(rows.rows[0].total_tokens, 45);
        assert!(
            rows.warnings
                .iter()
                .any(|line| { line == "WARN  codex: skipped 2 records in 1 input (invalid data)" }),
            "{:?}",
            rows.warnings
        );
        let warnings = rows.warnings.join("\n");
        assert!(warnings.contains("headless.jsonl:1:"), "{warnings}");
        assert!(warnings.contains("headless.jsonl:2:"), "{warnings}");
        assert!(!warnings.contains("SECRET_CANARY"), "{warnings}");
    }
}
