use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{cli::*, *};

mod loader;
mod parser;
mod paths;
mod sqlite;

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: crate::cli::AgentReportKind,
) -> Result<Vec<UsageSummary>> {
    SOURCE.summarize(
        entries,
        ccusage_adapter_common::source::SummaryKind::for_report(kind, WeekDay::Monday),
    )
}

pub struct OpenClaw;

pub static SOURCE: OpenClaw = OpenClaw;

impl EntrySource for OpenClaw {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            ..EntryDescriptor::standard("openclaw", "OpenClaw")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
