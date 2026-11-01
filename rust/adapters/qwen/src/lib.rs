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
