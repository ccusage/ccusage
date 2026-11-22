use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    Diagnostics, LoadedEntry, Result, Skip, SkipKind, TimestampMs, TokenUsageRaw, cli,
    cli::AgentCommandArgs, date_at, model_aliases, parse_ts_timestamp,
};

mod loader;
mod parser;
mod paths;

pub struct Amp;

pub static SOURCE: Amp = Amp;

impl EntrySource for Amp {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            table: ccusage_adapter_common::source::TableStyle::Credits,
            ..EntryDescriptor::standard("amp", "Amp")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{LoadRequest, ProgressOwner, SourceOptions, UsageSource},
    };
    use ccusage_core::cli::{AgentReportKind, SharedArgs};
    use ccusage_test_support::{Environment, Fixture, fs_fixture};

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
            since: since.map(|date| date.parse().unwrap()),
            until: until.map(|date| date.parse().unwrap()),
            timezone: Some(timezone.parse().unwrap()),
            offline: true,
            ..SharedArgs::with_defaults()
        }
    }

    #[test]
    fn focused_totals_match_the_unified_slice() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fixture();
        Environment::with_var("AMP_DATA_DIR", fixture.root()).run(|| {
            for shared in [
                shared(None, None, "UTC"),
                shared(Some("20260131"), Some("20260201"), "UTC"),
                shared(Some("20260201"), Some("20260201"), "Asia/Tokyo"),
                shared(Some("20260101"), Some("20260131"), "UTC"),
                shared(Some("20270101"), None, "UTC"),
            ] {
                assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Amp, &shared);
            }
        })
    }

    #[test]
    fn a_thread_file_left_cut_off_is_skipped_and_the_rest_still_counts() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fixture();
        Environment::with_var("AMP_DATA_DIR", fixture.root()).run(|| {
            let quiet = shared(None, None, "UTC");
            let shared = SharedArgs {
                debug: true,
                ..quiet.clone()
            };

            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Session,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
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
            let summary = "WARN  amp: skipped 1 input (invalid data)".to_owned();
            assert_eq!(
                rows.diagnostics.warnings(&shared),
                [
                    summary.clone(),
                    format!(
                        "amp: invalid data: {}: JSON document is truncated at line 1, column 63",
                        truncated.display()
                    )
                ]
            );
            assert_eq!(rows.diagnostics.warnings(&quiet), [summary]);
        })
    }
}
