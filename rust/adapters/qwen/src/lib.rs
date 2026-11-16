use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date,
    filter_session_summaries_by_last_activity, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result,
    cli::{AgentCommandArgs, AgentReportKind},
    print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub use loader::load_entries;
pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let mut entries = load_entries(&args.shared)?;
    let mut rows = if args.kind == AgentReportKind::Session {
        summarize_entries(&entries, args.kind)?
    } else {
        filter_loaded_entries_by_date(&mut entries, &args.shared);
        summarize_entries(&entries, args.kind)?
    };
    if args.kind == AgentReportKind::Session {
        filter_session_summaries_by_last_activity(&mut rows, &args.shared);
    }
    sort_summaries(&mut rows, &args.shared.order, |row| {
        ccusage_core::summary_period(row)
    });
    if wants_json(&args.shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            args.shared.jq.as_deref(),
            args.shared.no_cost,
        );
    }
    print_usage_table(
        "Qwen Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &args.shared,
        false,
        None,
    )?;
    Ok(())
}

pub fn has_data() -> bool {
    paths::discover_chat_files().is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
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
    fn loads_archived_chat_files_alongside_live_ones() {
        let fixture = fs_fixture!({
            "projects/myProject/chats/live.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:00:00.000Z","sessionId":"live-0001","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50}}"#,
            "projects/myProject/chats/archive/archived.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T03:00:00.000Z","sessionId":"arch-0002","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
            // Only the flat `chats/archive/` location is picked up.
            "projects/myProject/chats/other/ignored.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T04:00:00.000Z","sessionId":"other-0003","usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":500}}"#,
            "projects/myProject/chats/archive/nested/deep.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T05:00:00.000Z","sessionId":"deep-0004","usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":500}}"#,
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
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.root());
        let entries = load_entries(&shared).unwrap();

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
}
