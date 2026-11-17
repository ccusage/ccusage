use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{cli::*, *};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: crate::cli::AgentReportKind,
) -> Result<Vec<UsageSummary>> {
    SOURCE.summarize(
        entries,
        ccusage_adapter_common::source::SummaryKind::for_report(kind, WeekDay::Monday),
    )
}

#[cfg(test)]
pub(crate) use ccusage_adapter_common::report::activity_report_json as report_from_rows;

pub struct Antigravity;
pub static SOURCE: Antigravity = Antigravity;

impl EntrySource for Antigravity {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            ..EntryDescriptor::standard("antigravity", "Antigravity")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_for_context(cx)
    }
}

/// Runs an Antigravity usage report for the requested report kind.
pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
