use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

pub struct Dsh;

pub static SOURCE: Dsh = Dsh;

impl EntrySource for Dsh {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            ..EntryDescriptor::standard("dsh", "DeepSeek Harness")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
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
            "DeepSeek Harness Token Usage Report",
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
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<(Vec<LoadedEntry>, Diagnostics)> {
    ccusage_adapter_common::testing::load_entries(
        &SOURCE,
        shared,
        ccusage_adapter_common::source::SourceOptions::default(),
        Some(pricing),
    )
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
