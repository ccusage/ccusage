use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;
use serde_json::Value;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    LoadedEntry, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

#[cfg(test)]
pub(crate) use loader::load_entries;
pub use paths::{named_store_paths, paths as default_paths};
pub use report::{report_from_rows, summarize_entries};

pub struct Pi;

pub static SOURCE: Pi = Pi;

impl EntrySource for Pi {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            unified_project_path: true,
            ..EntryDescriptor::standard("pi", "pi-agent")
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

    fn summarize_unified(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        report::summarize_unified_entries(entries, kind)
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
        crate::print_usage_table(
            "pi-agent Token Usage Report",
            ccusage_core::first_column(kind),
            rows,
            shared,
            false,
            None,
        )?;
        Ok(())
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
            unified_project_path: true,
            ..EntryDescriptor::standard(self.name, self.name)
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_named_store_entries(cx, &self.paths, self.name)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        SOURCE.summarize(entries, kind)
    }

    fn summarize_unified(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        SOURCE.summarize_unified(entries, kind)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        SOURCE.report_json(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        SOURCE.print_table(rows, kind, shared)
    }
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{ProgressOwner, SourceOptions, UnifiedRequest, UsageSource},
    };
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::*;

    #[test]
    fn unified_sessions_keep_project_paths_and_focused_totals() {
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
        let _guard = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("sessions"));
        let shared = crate::cli::SharedArgs {
            mode: crate::cli::CostMode::Display,
            timezone: Some("UTC".to_string()),
            offline: true,
            ..crate::cli::SharedArgs::default()
        };

        assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Pi, &shared, &[]);
        let entries = load_entries(&shared, None, None).unwrap();
        let focused = summarize_entries(&entries, AgentReportKind::Session).unwrap();
        let unified =
            report::summarize_unified_entries(&entries, AgentReportKind::Session).unwrap();

        assert_eq!(focused.len(), 1);
        assert_eq!(unified.len(), 2);
        assert_eq!(
            unified
                .iter()
                .filter_map(|row| row.project_path.as_deref())
                .collect::<std::collections::BTreeSet<_>>(),
            ["project-a", "project-b"].into()
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
        let _guard = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("sessions"));
        let shared = crate::cli::SharedArgs {
            mode: crate::cli::CostMode::Display,
            debug: true,
            timezone: Some("UTC".to_string()),
            offline: true,
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

        assert_eq!(rows.rows.len(), 1);
        assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
            warning.contains("invalid data")
                && warning.contains("agent_session-a.jsonl:2")
                && !warning.contains("SECRET_CANARY")
        }));
    }
}
