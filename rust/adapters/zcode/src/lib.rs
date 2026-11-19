use ccusage_adapter_common::source::{ALL_REPORTS, EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    LoadedEntry, Result, UsageTableOptions,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
};

pub struct Zcode;
pub static SOURCE: Zcode = Zcode;

impl EntrySource for Zcode {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            reports: ALL_REPORTS,
            ..EntryDescriptor::standard("zcode", "ZCode")
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
        let table_options = UsageTableOptions {
            show_cache_creation: rows.iter().any(|row| row.cache_creation_tokens > 0),
        };
        crate::print_usage_table_with_options(
            "ZCode Token Usage Report",
            ccusage_core::first_column(kind),
            rows,
            shared,
            false,
            None,
            table_options,
        )
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_standard_cache_creation_table_option() {
        let rows = [UsageSummary {
            date: Some("2026-08-16".to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_tokens: 2,
            cache_read_tokens: 3,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: vec!["GLM-5.3".to_string()],
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }];

        assert!(rows.iter().any(|row| row.cache_creation_tokens > 0));
    }
}
