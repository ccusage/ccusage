use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

pub use report::summarize_entries;

pub struct Cline;
pub static SOURCE: Cline = Cline;

impl EntrySource for Cline {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            ..EntryDescriptor::standard("cline", "Cline")
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
        ccusage_adapter_common::print_table_for_agent("Cline", kind, rows, shared)
    }
}

/// Runs a Cline usage report for the requested report kind.
pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
