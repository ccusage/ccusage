use std::sync::Arc;

use ccusage_core::{
    LoadedEntry, TokenUsageRaw, UsageEntry, UsageMessage, cli::AgentReportKind, parse_ts_timestamp,
};
use serde_json::json;

#[test]
fn builds_amp_daily_json_report() {
    let entry = LoadedEntry {
        data: UsageEntry {
            session_id: Some("thread-a".to_string()),
            timestamp: "2026-05-01T01:02:03.000Z".to_string(),
            version: None,
            message: UsageMessage {
                usage: TokenUsageRaw {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 20,
                    cache_read_input_tokens: 10,
                    speed: None,
                    cache_creation: None,
                },
                model: Some("claude-sonnet-4-20250514".to_string()),
                id: Some("event-a".to_string()),
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        },
        timestamp: parse_ts_timestamp("2026-05-01T01:02:03.000Z").unwrap(),
        date: "2026-05-01".to_string(),
        project: Arc::from("amp"),
        session_id: Arc::from("thread-a"),
        project_path: Arc::from("Amp"),
        cost: 0.02,
        extra_total_tokens: 0,
        credits: Some(1.25),
        message_count: None,
        model: Some("claude-sonnet-4-20250514".to_string()),
        usage_limit_reset_time: None,
        missing_pricing_model: None,
        stored_cost: None,
    };

    let rows = ccusage_adapter_common::source::EntrySource::summarize(
        &ccusage_adapter_amp::SOURCE,
        &[entry],
        ccusage_adapter_common::source::SummaryKind::for_report(
            AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Monday,
        ),
    )
    .unwrap();
    let report = ccusage_adapter_common::source::EntrySource::report_json(
        &ccusage_adapter_amp::SOURCE,
        &rows,
        AgentReportKind::Daily,
    );

    assert_eq!(report["daily"][0]["date"], "2026-05-01");
    assert_eq!(report["daily"][0]["inputTokens"], 100);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
    assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
    assert_eq!(report["daily"][0]["totalTokens"], 180);
    assert_eq!(report["daily"][0]["credits"], json!(1.25));
    assert_eq!(report["daily"][0]["totalCost"], json!(0.02));
    assert_eq!(report["totals"]["credits"], json!(1.25));
}
