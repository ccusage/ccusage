use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::cli::{AgentCommandArgs, AgentReportKind, SharedArgs};
use ccusage_core::*;
use serde_json::Value;

mod loader;
mod parser;
mod paths;
mod report;

pub use report::summarize_entries;

pub struct Droid;

pub static SOURCE: Droid = Droid;

impl EntrySource for Droid {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("droid", "Droid")
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        report::summarize_entries(entries, kind)
    }

    fn sort_key<'a>(&self, row: &'a UsageSummary) -> &'a str {
        summary_period(row)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        report::report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        crate::print_usage_table(
            "Droid Token Usage Report",
            first_column(kind),
            rows,
            shared,
            false,
            None,
        )
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
