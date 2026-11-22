use ccusage_adapter_common::source::{
    Detection, EntryDescriptor, EntrySource, LoadContext, WindowMode,
};
use ccusage_core::{
    Diagnostics, LoadedEntry, Result, TimestampMs, TokenUsageRaw, UsageSummary, cli,
    cli::{AgentCommandArgs, AgentReportKind},
    date_at, fast, parse_ts_timestamp,
};

mod loader;
mod parser;
mod paths;

pub struct Qwen;

pub static SOURCE: Qwen = Qwen;

impl EntrySource for Qwen {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            window: WindowMode::SessionLastActivity,
            detection: Detection::NonEmptyOrHasData(has_data),
            ..EntryDescriptor::standard("qwen", "Qwen")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> serde_json::Value {
        let mut report = ccusage_adapter_common::report::report_json(rows, kind, true);
        if rows.is_empty() {
            report["totals"] = serde_json::Value::Null;
        }
        report
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

fn has_data() -> bool {
    paths::discover_chat_files(&ccusage_core::Diagnostics::new("qwen"))
        .is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{LoadRequest, ProgressOwner, SourceOptions, UsageSource},
    };
    use ccusage_test_support::{Environment, fs_fixture};
    use serde_json::json;

    use super::*;
    use crate::cli::{CostMode, SharedArgs};

    #[test]
    fn loads_qwen_jsonl_usage_entries() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/chat-a.jsonl": [
                r#"{"type":"user","text":"hello"}"#,
                r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-23T14:24:56.857Z","sessionId":"session-json","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":10,"cachedContentTokenCount":5,"totalTokenCount":160}}"#,
            ]
            .join("\n"),
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                None,
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-02-23");
            assert_eq!(entries[0].session_id.as_ref(), "session-json");
            assert_eq!(entries[0].project_path.as_ref(), "myProject");
            assert_eq!(entries[0].model.as_deref(), Some("qwen3-coder-plus"));
            assert_eq!(entries[0].usage.input_tokens, 95);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 5);
            assert_eq!(entries[0].extra_total_tokens, 10);
        })
    }

    #[test]
    fn loads_archived_chat_files_alongside_live_ones() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/live.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:00:00.000Z","sessionId":"live-0001","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50}}"#,
            "projects/myProject/chats/archive/archived.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T03:00:00.000Z","sessionId":"arch-0002","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
            "projects/myProject/chats/other/ignored.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T04:00:00.000Z","sessionId":"other-0003","usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":500}}"#,
            "projects/myProject/chats/archive/nested/deep.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T05:00:00.000Z","sessionId":"deep-0004","usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":500}}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                None,
            )
            .unwrap();
            let rows = ccusage_adapter_common::source::EntrySource::summarize(
                &crate::SOURCE,
                &entries,
                ccusage_adapter_common::source::SummaryKind::for_report(
                    AgentReportKind::Daily,
                    ccusage_core::cli::WeekDay::Monday,
                ),
            )
            .unwrap();
            let report = SOURCE.report_json(&rows, AgentReportKind::Daily);

            assert_eq!(entries.len(), 2);
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.project_path.as_ref() == "myProject"),
                "archived entries keep their project, got: {:?}",
                entries
                    .iter()
                    .map(|entry| entry.project_path.as_ref())
                    .collect::<Vec<_>>()
            );
            assert_eq!(report["daily"][0]["totalTokens"], 165);
        })
    }

    #[test]
    fn counts_a_session_in_both_live_and_archive_once() {
        let transcript = [
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:00:00.000Z","sessionId":"session-a","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50}}"#,
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:01:00.000Z","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "projects/myProject/chats/session-a.jsonl": transcript.as_str(),
            "projects/myProject/chats/archive/session-a.jsonl": transcript.as_str(),
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                None,
            )
            .unwrap();

            assert_eq!(
                entries
                    .iter()
                    .map(|entry| (entry.session_id.as_ref(), entry.project_path.as_ref()))
                    .collect::<Vec<_>>(),
                [
                    ("session-a", "myProject"),
                    ("myProject-session-a", "myProject")
                ]
            );
        })
    }

    #[test]
    fn builds_qwen_daily_json_report_with_reasoning_in_total() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/chat-a.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-23T14:24:56.857Z","sessionId":"session-json","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":10,"cachedContentTokenCount":5,"totalTokenCount":160}}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                None,
            )
            .unwrap();
            let rows = ccusage_adapter_common::source::EntrySource::summarize(
                &crate::SOURCE,
                &entries,
                ccusage_adapter_common::source::SummaryKind::for_report(
                    AgentReportKind::Daily,
                    ccusage_core::cli::WeekDay::Monday,
                ),
            )
            .unwrap();
            let report = SOURCE.report_json(&rows, AgentReportKind::Daily);

            assert_eq!(report["daily"][0]["date"], "2026-02-23");
            assert_eq!(report["daily"][0]["outputTokens"], 50);
            assert_eq!(report["daily"][0]["cacheReadTokens"], 5);
            assert_eq!(report["daily"][0]["totalTokens"], 160);
            assert_eq!(
                report["daily"][0]["modelsUsed"],
                json!(["qwen3-coder-plus"])
            );
            assert_eq!(
                report["daily"][0]["modelBreakdowns"],
                json!([{
                    "modelName": "qwen3-coder-plus",
                    "inputTokens": 95,
                    "outputTokens": 50,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 5,
                    "cost": 0.0
                }])
            );
        })
    }

    #[test]
    fn session_window_keeps_the_last_activity_policy_and_detection() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/chat.jsonl": [
                r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-01-31T23:30:00.000Z","sessionId":"same-session","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2}}"#,
                r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-01T00:30:00.000Z","sessionId":"same-session","usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":3}}"#,
            ]
            .join("\n"),
        });
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                since: Some("20260131".parse().unwrap()),
                until: Some("20260131".parse().unwrap()),
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..SharedArgs::default()
            };

            assert_focused_matches_unified(&SOURCE, ccusage_core::cli::AgentId::Qwen, &shared);

            let outside_window = SharedArgs {
                since: Some("20270101".parse().unwrap()),
                ..shared.clone()
            };
            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &outside_window,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert!(rows.detected);
            assert!(rows.rows.is_empty());
        })
    }

    #[test]
    fn malformed_selected_lines_are_classified_and_later_usage_survives() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/chat.jsonl": [
                r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-01T00:30:00.000Z","sessionId":"same-session","usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":3}}"#,
                r#"{"type":"assistant","usageMetadata":"SECRET_CANARY"}"#,
                r#"{"type":"user","usageMetadata":"SECRET_CANARY"}"#,
            ]
            .join("\n"),
        });
        Environment::with_var("QWEN_DATA_DIR", fixture.root()).run(|| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                debug: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..SharedArgs::default()
            };

            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();

            assert_eq!(rows.rows.len(), 1);
            assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
                warning.contains("invalid data")
                    && warning.contains("chat.jsonl:2")
                    && !warning.contains("SECRET_CANARY")
            }));
            assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
                warning.contains("undecoded usage")
                    && warning.contains("chat.jsonl:3")
                    && !warning.contains("SECRET_CANARY")
            }));
        })
    }
}
