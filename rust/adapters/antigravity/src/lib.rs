use ccusage_adapter_common::source::{ALL_REPORTS, EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    LoadedEntry, Result,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

pub use loader::load_entries;
pub use loader::load_entries_with_warnings;
pub use report::summarize_entries;

pub struct Antigravity;
pub static SOURCE: Antigravity = Antigravity;

impl EntrySource for Antigravity {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            reports: ALL_REPORTS,
            ..EntryDescriptor::standard("antigravity", "Antigravity")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_for_context(cx)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        report::summarize_entries(entries, kind)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> serde_json::Value {
        report::report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        crate::print_usage_table(
            "Antigravity Token Usage Report",
            ccusage_core::first_column(kind),
            rows,
            shared,
            false,
            None,
        )
    }
}

/// Runs an Antigravity usage report for the requested report kind.
pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
