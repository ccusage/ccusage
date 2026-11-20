use ccusage_adapter_common::source::{Detection, EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    model_aliases, stored_cost_for_mode, total_usage_tokens,
};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

use loader::has_data;

pub struct Grok;

pub static SOURCE: Grok = Grok;

impl EntrySource for Grok {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            table: ccusage_adapter_common::source::TableStyle::AutoCacheCreation,
            detection: Detection::NonEmptyOrHasData(has_data),
            ..EntryDescriptor::standard("grok", "Grok")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

#[cfg(test)]
mod smoke_tests {
    use crate::cli::SharedArgs;
    use ccusage_core::PricingMap;

    #[test]
    #[ignore = "requires real Grok home with sessions/**/updates.jsonl"]
    fn smoke_real_grok_home_loads_without_error() {
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_embedded();
        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&pricing),
        )
        .unwrap();
        let _ = entries.len();
    }
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{LoadRequest, ProgressOwner, SourceOptions, UsageSource},
    };
    use ccusage_core::UsageSummary;
    use ccusage_core::cli::AgentReportKind;
    use ccusage_test_support::{Environment, fs_fixture};
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn hides_cache_creation_when_selected_rows_are_zero() {
        assert!(
            !ccusage_adapter_common::report::table_options(
                SOURCE.descriptor(),
                &[summary(0), summary(0)]
            )
            .show_cache_creation
        );
    }

    #[test]
    fn shows_cache_creation_when_any_selected_row_is_nonzero() {
        assert!(
            ccusage_adapter_common::report::table_options(
                SOURCE.descriptor(),
                &[summary(0), summary(25)]
            )
            .show_cache_creation
        );
    }

    #[test]
    fn focused_reports_match_the_unified_slice() {
        ccusage_test_support::install_pricing_snapshots();
        let line = serde_json::json!({
            "timestamp": 1_770_000_000_i64,
            "params": {
                "sessionId": "session-a",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "usage": {
                        "modelUsage": {
                            "grok-4.5-build": {
                                "inputTokens": 100,
                                "outputTokens": 20,
                                "cachedReadTokens": 40,
                            }
                        }
                    }
                }
            }
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/project-a/session-a/updates.jsonl": line,
        });
        Environment::with_vars([(
            crate::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let shared = crate::cli::SharedArgs {
                mode: crate::cli::CostMode::Display,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..crate::cli::SharedArgs::default()
            };

            assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Grok, &shared);
        })
    }

    #[test]
    fn malformed_selected_update_is_reported_and_detected_outside_the_window() {
        ccusage_test_support::install_pricing_snapshots();
        let good = serde_json::json!({
            "timestamp": 1_770_000_000_i64,
            "params": {
                "sessionId": "session-a",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "usage": {
                        "modelUsage": {
                            "grok-4.5-build": {"inputTokens": 100, "outputTokens": 20}
                        }
                    }
                }
            }
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/project-a/session-a/updates.jsonl": format!(
                "{{\"params\":{{\"update\":{{\"sessionUpdate\":\"turn_completed\",\"usage\":{{\"modelUsage\":\"SECRET_CANARY\"}}}}}}}}\n{good}"
            ),
        });
        Environment::with_vars([(
            crate::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let shared = crate::cli::SharedArgs {
                mode: crate::cli::CostMode::Display,
                debug: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                since: Some("20270101".parse().unwrap()),
                ..crate::cli::SharedArgs::default()
            };

            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();

            assert!(rows.detected);
            assert!(rows.rows.is_empty());
            assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
                warning.contains("invalid data")
                    && warning.contains("updates.jsonl:1")
                    && !warning.contains("SECRET_CANARY")
            }));
        })
    }

    fn summary(cache_creation_tokens: u64) -> UsageSummary {
        UsageSummary {
            date: Some("2026-08-15".to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens,
            cache_read_tokens: 40,
            extra_total_tokens: 0,
            total_cost: 0.01,
            credits: None,
            message_count: None,
            models_used: vec!["grok-4.5-build".to_string()],
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }
}
