use serde_json::Value;

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

pub use report::{report_from_rows, summarize_entries};

/// The Amp usage source.
pub struct Amp;

pub static SOURCE: Amp = Amp;

impl EntrySource for Amp {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("amp", "Amp")
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

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        report::report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        report::print_table(kind, rows, shared)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{ProgressOwner, SourceOptions, UnifiedRequest, UsageSource},
    };
    use ccusage_test_support::{EnvVarGuard, Fixture, fs_fixture};

    use super::*;

    fn fixture() -> Fixture {
        fs_fixture!({
            "threads/T-ledger.json": r#"{"id":"T-ledger","usageLedger":{"events":[
                {"id":"e1","timestamp":"2026-01-30T23:30:00.000Z","model":"claude-sonnet-4-20250514","tokens":{"input":1200,"output":340},"toMessageId":1,"credits":1.25},
                {"id":"e2","timestamp":"2026-01-31T10:00:00.000Z","model":"gpt-5","tokens":{"input":900,"output":120,"total":1500}},
                {"id":"e3","timestamp":"2026-02-14T08:00:00.000Z","model":"unknown-model-x","tokens":{"total":345}}
            ]},"messages":[
                {"role":"assistant","messageId":1,"usage":{"cacheCreationInputTokens":500,"cacheReadInputTokens":7000}}
            ]}"#,
            "threads/T-messages.json": r#"{"id":"T-messages","messages":[
                {"role":"assistant","usage":{"model":"claude-haiku-4-5-20251001","inputTokens":10,"outputTokens":178,"cacheReadInputTokens":11372,"timestamp":"2026-01-31T11:42:10.652Z","credits":0.75}},
                {"role":"assistant","usage":{"model":"gpt-5","totalTokens":345,"timestamp":"2026-02-01T23:59:59.999Z"}}
            ]}"#,
            "threads/T-truncated.json": r#"{"id":"T-truncated","usageLedger":{"events":[{"id":"e1","timest"#,
        })
    }

    fn shared(since: Option<&str>, until: Option<&str>, timezone: &str) -> SharedArgs {
        SharedArgs {
            since: since.map(str::to_string),
            until: until.map(str::to_string),
            timezone: Some(timezone.to_string()),
            offline: true,
            ..SharedArgs::with_defaults()
        }
    }

    #[test]
    fn focused_totals_match_the_unified_slice() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fixture();
        let _guard = EnvVarGuard::set("AMP_DATA_DIR", fixture.root());

        for shared in [
            shared(None, None, "UTC"),
            shared(Some("20260131"), Some("20260201"), "UTC"),
            shared(Some("20260201"), Some("20260201"), "Asia/Tokyo"),
            shared(Some("2026"), Some("202602"), "UTC"),
            shared(Some("20270101"), None, "UTC"),
        ] {
            assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Amp, &shared, &[]);
        }
    }

    #[test]
    fn a_thread_file_cut_mid_write_is_noted_under_debug_and_the_rest_still_counts() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fixture();
        let _guard = EnvVarGuard::set("AMP_DATA_DIR", fixture.root());
        let quiet = shared(None, None, "UTC");
        let shared = SharedArgs {
            debug: true,
            ..quiet.clone()
        };

        let rows = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Session,
                shared: &shared,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                claude_counted: None,
            })
            .unwrap();

        let sessions = rows
            .rows
            .iter()
            .map(|row| row.period.as_str())
            .collect::<Vec<_>>();
        assert_eq!(sessions, ["T-ledger", "T-messages"]);
        assert!(rows.detected);
        let truncated = fixture.path("threads/T-truncated.json");
        assert_eq!(
            rows.diagnostics.warnings(&shared),
            [format!(
                "amp: {}: incomplete, read as still being written",
                truncated.display()
            )]
        );
        assert!(rows.diagnostics.warnings(&quiet).is_empty());
    }
}
