use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at, fast,
    model_aliases, parse_ts_timestamp, stored_cost_for_mode, total_usage_tokens,
};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

pub use paths::{named_store_paths, paths as default_paths};

pub struct Pi;

pub static SOURCE: Pi = Pi;

impl EntrySource for Pi {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            unified_project_path: true,
            session_scope: ccusage_adapter_common::source::SessionScope::Project,
            ..EntryDescriptor::standard("pi", "pi-agent")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

/// A `pi.stores` entry: a pi-format store the unified report lists as its own
/// agent, under the store's name.
pub struct NamedStore {
    name: &'static str,
    paths: Vec<std::path::PathBuf>,
}

impl NamedStore {
    /// `paths` are the store's session directories, as
    /// [`named_store_paths`] resolves them.
    pub fn new(name: &'static str, paths: Vec<std::path::PathBuf>) -> Self {
        Self { name, paths }
    }
}

impl EntrySource for NamedStore {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            unified_project_path: true,
            session_scope: ccusage_adapter_common::source::SessionScope::Project,
            ..EntryDescriptor::standard(self.name, self.name)
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_named_store_entries(cx, &self.paths, self.name)
    }
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{LoadRequest, ProgressOwner, SourceOptions, UsageSource},
    };
    use ccusage_core::cli::AgentReportKind;
    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;

    #[test]
    fn focused_and_unified_sessions_keep_projects_with_the_same_session_id() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_same-session.jsonl": [
                r#"{"type":"session","id":"same-session","timestamp":"2026-01-01T00:00:00.000Z"}"#,
                r#"{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            ]
            .join("\n"),
            "sessions/project-b/agent_same-session.jsonl": [
                r#"{"type":"session","id":"same-session","timestamp":"2026-01-01T00:00:00.000Z"}"#,
                r#"{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
            ]
            .join("\n"),
        });
        Environment::with_var("PI_AGENT_DIR", fixture.path("sessions")).run(|| {
            let shared = crate::cli::SharedArgs {
                mode: crate::cli::CostMode::Display,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..crate::cli::SharedArgs::default()
            };

            assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Pi, &shared);
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                ccusage_adapter_common::source::SourceOptions {
                    pi_path: None,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
            let focused = ccusage_adapter_common::source::EntrySource::summarize(
                &crate::SOURCE,
                &entries,
                ccusage_adapter_common::source::SummaryKind::for_report(
                    AgentReportKind::Session,
                    ccusage_core::cli::WeekDay::Sunday,
                ),
            )
            .unwrap();

            assert_eq!(focused.len(), 2);
            assert_eq!(focused[0].input_tokens, 10);
            assert_eq!(focused[1].input_tokens, 20);
            let json = SOURCE.report_json(&focused, AgentReportKind::Session);
            assert_eq!(json["sessions"][0]["sessionId"], "same-session");
            assert_eq!(json["sessions"][0]["projectPath"], "project-a");
            assert_eq!(json["sessions"][1]["sessionId"], "same-session");
            assert_eq!(json["sessions"][1]["projectPath"], "project-b");
            assert_eq!(json["totals"]["inputTokens"], 30);
            assert_eq!(
                focused
                    .iter()
                    .filter_map(|row| row.project_path.as_deref())
                    .collect::<std::collections::BTreeSet<_>>(),
                ["project-a", "project-b"].into()
            );
        })
    }

    #[test]
    fn session_report_leaves_out_what_a_child_replays_from_a_stale_parent() {
        ccusage_test_support::install_pricing_snapshots();
        let usage = |timestamp: &str, input: u32| {
            format!(
                r#"{{"type":"message","timestamp":"{timestamp}","message":{{"role":"assistant","model":"gpt-5","usage":{{"input":{input},"output":1}}}}}}"#
            )
        };
        let fixture = fs_fixture!({
            "sessions/project-a/root.jsonl": [
                r#"{"type":"session","id":"root","timestamp":"2098-12-01T00:00:00.000Z"}"#.to_owned(),
                usage("2098-12-01T01:00:00.000Z", 100),
            ]
            .join("\n"),
        });
        let root = fixture.path("sessions/project-a/root.jsonl");
        let child = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                format!(
                    r#"{{"type":"session","id":"child","timestamp":"2099-01-10T12:00:00.000Z","parentSession":{}}}"#,
                    serde_json::Value::from(root.to_string_lossy())
                ),
                usage("2098-12-01T01:00:00.000Z", 100),
                usage("2099-01-10T12:00:00.000Z", 300),
            ]
            .join("\n"),
        );
        for (file, modified) in [
            (&root, "2099-01-05T00:00:00Z"),
            (&child, "2099-01-10T13:00:00Z"),
        ] {
            ccusage_test_support::set_modified(file, ccusage_test_support::instant(modified));
        }
        let shared = crate::cli::SharedArgs {
            mode: crate::cli::CostMode::Display,
            since: Some("20990110".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            clock: ccusage_core::cli::ReportClock::Fixed(
                ccusage_test_support::instant("2099-12-31T00:00:00Z").into(),
            ),
            offline: true,
            ..crate::cli::SharedArgs::default()
        };
        let sessions = fixture.path("sessions");

        let rows = SOURCE
            .load_rows(&LoadRequest {
                kind: ccusage_adapter_common::source::LoadKind::for_report(
                    AgentReportKind::Session,
                ),
                shared: &shared,
                options: SourceOptions {
                    pi_path: sessions.to_str(),
                    ..SourceOptions::default()
                },
                pricing: None,
                progress: ProgressOwner::Caller,
            })
            .unwrap();

        assert_eq!(
            rows.rows
                .iter()
                .map(|row| (row.period.as_str(), row.input_tokens))
                .collect::<Vec<_>>(),
            [("child", 300)]
        );
    }

    #[test]
    fn malformed_selected_message_is_diagnosed_without_dropping_following_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": [
                r#"{"type":"session","id":"session-a","timestamp":"2026-01-01T00:00:00.000Z"}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
                r#"{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            ]
            .join("\n"),
        });
        Environment::with_var("PI_AGENT_DIR", fixture.path("sessions")).run(|| {
            let shared = crate::cli::SharedArgs {
                mode: crate::cli::CostMode::Display,
                debug: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
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

            assert_eq!(rows.rows.len(), 1);
            assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
                warning.contains("invalid data")
                    && warning.contains("agent_session-a.jsonl:2")
                    && !warning.contains("SECRET_CANARY")
            }));
        })
    }
}
