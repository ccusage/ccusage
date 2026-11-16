use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
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
        EntryDescriptor::standard("kilo", "Kilo")
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

#[cfg(test)]
pub(crate) fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> crate::Result<Vec<crate::LoadedEntry>> {
    ccusage_adapter_common::testing::load_entries(
        &SOURCE,
        shared,
        ccusage_adapter_common::source::SourceOptions::default(),
        Some(pricing),
    )
    .map(|(entries, _)| entries)
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
