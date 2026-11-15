use ccusage_adapter_common::source::{Detection, EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;
use serde_json::Value;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    LoadedEntry, Result, UsageSummary, UsageTableOptions,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

pub use loader::{has_data, load_entries};
pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub struct Grok;

pub static SOURCE: Grok = Grok;

impl EntrySource for Grok {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            detection: Detection::NonEmptyOrHasData(has_data),
            ..EntryDescriptor::standard("grok", "Grok")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        summarize_entries(entries, kind)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        crate::print_usage_table_with_options(
            "Grok Token Usage Report",
            ccusage_core::first_column(kind),
            rows,
            shared,
            false,
            None,
            table_options(rows),
        )?;
        Ok(())
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

fn table_options(rows: &[UsageSummary]) -> UsageTableOptions {
    UsageTableOptions {
        show_cache_creation: rows.iter().any(|row| row.cache_creation_tokens > 0),
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use crate::cli::SharedArgs;

    #[test]
    #[ignore = "requires real Grok home with sessions/**/updates.jsonl"]
    fn smoke_real_grok_home_loads_without_error() {
        let shared = SharedArgs {
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_embedded();
        let entries = load_entries(&shared, &pricing).unwrap();
        let _ = entries.len();
    }
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{ProgressOwner, SourceOptions, UnifiedRequest, UsageSource},
    };
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn hides_cache_creation_when_selected_rows_are_zero() {
        assert!(!table_options(&[summary(0), summary(0)]).show_cache_creation);
    }

    #[test]
    fn shows_cache_creation_when_any_selected_row_is_nonzero() {
        assert!(table_options(&[summary(0), summary(25)]).show_cache_creation);
    }

    #[test]
    fn focused_reports_match_the_unified_slice() {
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
        let _guard = EnvVarsGuard::set_many([(
            crate::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);
        let shared = crate::cli::SharedArgs {
            mode: crate::cli::CostMode::Display,
            timezone: Some("UTC".to_string()),
            offline: true,
            ..crate::cli::SharedArgs::default()
        };

        assert_focused_matches_unified(&SOURCE, &shared, &[]);
    }

    #[test]
    fn malformed_selected_update_is_reported_and_detected_outside_the_window() {
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
        let _guard = EnvVarsGuard::set_many([(
            crate::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);
        let shared = crate::cli::SharedArgs {
            mode: crate::cli::CostMode::Display,
            debug: true,
            timezone: Some("UTC".to_string()),
            offline: true,
            since: Some("20270101".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let rows = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &shared,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
            })
            .unwrap();

        assert!(rows.detected);
        assert!(rows.rows.is_empty());
        assert!(rows.warnings.iter().any(|warning| {
            warning.contains("invalid data")
                && warning.contains("updates.jsonl:1")
                && !warning.contains("SECRET_CANARY")
        }));
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
