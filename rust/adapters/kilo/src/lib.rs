use ccusage_adapter_common::source::{
    ALL_REPORTS, EntryDescriptor, EntrySource, LoadContext, SourceOptions,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{LoadedEntry, PricingMap, Result, UsageSummary, cli::AgentCommandArgs};

pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub struct Kilo;

pub static SOURCE: Kilo = Kilo;

impl EntrySource for Kilo {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            reports: ALL_REPORTS,
            ..EntryDescriptor::standard("kilo", "Kilo")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: crate::cli::AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        summarize_entries(entries, kind)
    }

    fn report_json(
        &self,
        rows: &[UsageSummary],
        kind: crate::cli::AgentReportKind,
    ) -> serde_json::Value {
        report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: crate::cli::AgentReportKind,
        shared: &crate::cli::SharedArgs,
    ) -> Result<()> {
        crate::print_usage_table(
            "Kilo Token Usage Report",
            crate::first_column(kind),
            rows,
            shared,
            false,
            None,
        )
    }
}

pub fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    ccusage_adapter_common::pipeline::load_entries_for_unmigrated_caller(
        &SOURCE,
        shared,
        SourceOptions::default(),
        Some(pricing),
    )
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
