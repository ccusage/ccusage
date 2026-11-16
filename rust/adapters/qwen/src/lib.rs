use ccusage_adapter_common::source::{
    Detection, EntryDescriptor, EntrySource, LoadContext, PricingSupply, WindowMode,
};
use ccusage_core::*;
use serde_json::Value;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    LoadedEntry, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentReportKind},
};

#[cfg(test)]
pub(crate) use loader::load_entries;
pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub struct Qwen;

pub static SOURCE: Qwen = Qwen;

impl EntrySource for Qwen {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            window: WindowMode::SessionLastActivity,
            detection: Detection::NonEmptyOrHasData(has_data),
            pricing: PricingSupply::Source,
            ..EntryDescriptor::standard("qwen", "Qwen")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        summarize_entries(entries, kind)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        report_from_rows(rows, kind)
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &crate::cli::SharedArgs,
    ) -> Result<()> {
        crate::print_usage_table(
            "Qwen Token Usage Report",
            ccusage_core::first_column(kind),
            rows,
            shared,
            false,
            None,
        )?;
        Ok(())
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}

pub fn has_data() -> bool {
    paths::discover_chat_files(&ccusage_core::Diagnostics::new("qwen"))
        .is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::{
        parity::assert_focused_matches_unified,
        source::{ProgressOwner, SourceOptions, UnifiedRequest, UsageSource},
    };
    use ccusage_test_support::{EnvVarGuard, fs_fixture};
    use serde_json::json;

    use super::*;
    use crate::cli::{CostMode, SharedArgs};

    #[test]
    fn loads_qwen_jsonl_usage_entries() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/chat-a.jsonl": [
                r#"{"type":"user","text":"hello"}"#,
                r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-23T14:24:56.857Z","sessionId":"session-json","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":10,"cachedContentTokenCount":5}}"#,
            ]
            .join("\n"),
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.root());
        let entries = load_entries(&shared).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-02-23");
        assert_eq!(entries[0].session_id.as_ref(), "session-json");
        assert_eq!(entries[0].project_path.as_ref(), "myProject");
        assert_eq!(entries[0].model.as_deref(), Some("qwen3-coder-plus"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 5);
        assert_eq!(entries[0].extra_total_tokens, 10);
    }

    #[test]
    fn builds_qwen_daily_json_report_with_reasoning_in_total() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/chat-a.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-23T14:24:56.857Z","sessionId":"session-json","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":10,"cachedContentTokenCount":5}}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.root());
        let entries = load_entries(&shared).unwrap();
        let rows = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["date"], "2026-02-23");
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 5);
        assert_eq!(report["daily"][0]["totalTokens"], 165);
        assert_eq!(
            report["daily"][0]["modelsUsed"],
            json!(["qwen3-coder-plus"])
        );
        assert_eq!(
            report["daily"][0]["modelBreakdowns"],
            json!([{
                "modelName": "qwen3-coder-plus",
                "inputTokens": 100,
                "outputTokens": 50,
                "cacheCreationTokens": 0,
                "cacheReadTokens": 5,
                "cost": 0.0
            }])
        );
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
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            since: Some("20260131".to_string()),
            until: Some("20260131".to_string()),
            timezone: Some("UTC".to_string()),
            offline: true,
            ..SharedArgs::default()
        };

        assert_focused_matches_unified(&SOURCE, &shared, &[]);

        let outside_window = SharedArgs {
            since: Some("20270101".to_string()),
            ..shared.clone()
        };
        let rows = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &outside_window,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
            })
            .unwrap();
        assert!(rows.detected);
        assert!(rows.rows.is_empty());
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
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            debug: true,
            timezone: Some("UTC".to_string()),
            offline: true,
            ..SharedArgs::default()
        };

        let rows = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &shared,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
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
    }
}
