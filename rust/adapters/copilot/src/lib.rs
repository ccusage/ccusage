use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, UsageSummary, cli,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
    date_at, fast, format_rfc3339_millis, model_aliases, parse_ts_timestamp, print_usage_table,
    stored_cost_for_mode,
};

mod loader;
mod parser;
mod paths;

pub struct Copilot;
pub static SOURCE: Copilot = Copilot;

impl EntrySource for Copilot {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            ..EntryDescriptor::standard("copilot", "GitHub Copilot CLI")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_for_context(cx)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        if rows.is_empty() {
            ccusage_core::errln!("{}", empty_usage_message());
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
