mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) use loader::load_entries;
pub(crate) use report::{report_from_rows, summarize_entries};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let mut entries = load_entries(&args.shared)?;
    let mut rows = if args.kind == AgentReportKind::Session {
        summarize_entries(&entries, args.kind)?
    } else {
        filter_loaded_entries_by_date(&mut entries, &args.shared);
        summarize_entries(&entries, args.kind)?
    };
    if args.kind == AgentReportKind::Session {
        filter_session_summaries(&mut rows, &args.shared);
    }
    sort_summaries(&mut rows, &args.shared.order, |row| {
        super::opencode::summary_period(row)
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
        super::opencode::first_column(args.kind),
        &rows,
        &args.shared,
        false,
        None,
    )?;
    Ok(())
}

fn filter_session_summaries(rows: &mut Vec<crate::UsageSummary>, shared: &SharedArgs) {
    if shared.since.is_some() || shared.until.is_some() {
        let since = shared.since.as_deref().map(|value| value.replace('-', ""));
        let until = shared.until.as_deref().map(|value| value.replace('-', ""));
        rows.retain(|row| {
            let date = row
                .last_activity
                .as_deref()
                .unwrap_or_default()
                .replace('-', "");
            since.as_ref().is_none_or(|bound| &date >= bound)
                && until.as_ref().is_none_or(|bound| &date <= bound)
        });
    }
}

pub(crate) fn has_data() -> bool {
    paths::discover_chat_files().is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{EnvVarGuard, fs_fixture};
    use serde_json::json;

    use super::*;
    use crate::UsageSummary;
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
    fn filters_session_summaries_with_iso_date_bounds() {
        let mut rows = vec![
            usage_summary("before", "2026-02-22"),
            usage_summary("inside", "2026-02-23"),
            usage_summary("after", "2026-02-24"),
        ];
        let shared = SharedArgs {
            since: Some("2026-02-23".to_string()),
            until: Some("2026-02-23".to_string()),
            ..SharedArgs::default()
        };

        filter_session_summaries(&mut rows, &shared);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("inside"));
    }

    fn usage_summary(session_id: &str, last_activity: &str) -> UsageSummary {
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: Some(session_id.to_string()),
            project_path: None,
            last_activity: Some(last_activity.to_string()),
            first_activity: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: Vec::new(),
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }
}
