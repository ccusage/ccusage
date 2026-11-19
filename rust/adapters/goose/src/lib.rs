use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::cli::{AgentCommandArgs, AgentReportKind, SharedArgs};
use ccusage_core::*;
use serde_json::Value;

mod loader;
mod parser;
mod paths;
mod report;

pub struct Goose;

pub static SOURCE: Goose = Goose;

impl EntrySource for Goose {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("goose", "Goose")
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
        report::summary_period(row)
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
        ccusage_adapter_common::print_table_for_agent("Goose", kind, rows, shared)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
