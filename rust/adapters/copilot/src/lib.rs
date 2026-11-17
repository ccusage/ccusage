use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::cli::{AgentCommandArgs, AgentReportKind, SharedArgs};
use crate::{LoadedEntry, Result};

pub use report::summarize_entries;

pub struct Copilot;
pub static SOURCE: Copilot = Copilot;

impl EntrySource for Copilot {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            ..EntryDescriptor::standard("copilot", "GitHub Copilot CLI")
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
        if rows.is_empty() {
            eprintln!("{}", empty_usage_message());
            return Ok(());
        }
        crate::print_usage_table(
            "GitHub Copilot CLI Token Usage Report",
            ccusage_core::first_column(kind),
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

fn empty_usage_message() -> &'static str {
    "No GitHub Copilot CLI usage data found.\nSession-state events are read from ${COPILOT_HOME:-~/.copilot}/session-state/<session-id>/events.jsonl; OpenTelemetry file export remains supported.\nSee https://ccusage.com/guide/copilot/#data-source"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_usage_message_links_to_copilot_docs() {
        let message = empty_usage_message();
        assert!(message.contains("https://ccusage.com/guide/copilot/#data-source"));
    }
}
