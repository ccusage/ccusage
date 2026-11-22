use std::sync::Arc;

use ccusage_core::{
    LoadedEntry, TokenUsageRaw,
    cli::{AgentReportKind, SortOrder, WeekDay},
    parse_ts_timestamp,
};
use serde_json::json;

#[test]
fn builds_opencode_daily_json_report() {
    let entry = LoadedEntry {
        usage: TokenUsageRaw {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
            speed: None,
            cache_creation: None,
        },
        message_id: Some("msg-1".to_string()),
        version: None,
        recorded_cost: Some(0.02),
        timestamp: parse_ts_timestamp("2026-01-02T00:00:00.000Z").unwrap(),
        date: ccusage_core::cli::IsoDate::parse("2026-01-02").unwrap(),
        project: Arc::from("opencode"),
        session_id: Arc::from("opencode-session"),
        project_path: Arc::from("OpenCode"),
        cost: 0.02,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some("claude-sonnet-4-20250514".to_string()),
        usage_limit_reset_time: None,
        missing_pricing_model: None,
        stored_cost: None,
        estimated_pricing: false,
    };

    let report = ccusage_adapter_opencode::report_json(
        &[entry],
        AgentReportKind::Daily,
        &SortOrder::Asc,
        WeekDay::Monday,
    )
    .unwrap();

    assert_eq!(report["daily"][0]["date"], "2026-01-02");
    assert_eq!(report["daily"][0]["inputTokens"], 100);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
    assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
    assert_eq!(report["daily"][0]["totalTokens"], 180);
    assert_eq!(report["daily"][0]["totalCost"], json!(0.02));
    assert_eq!(
        report["daily"][0]["modelsUsed"],
        json!(["claude-sonnet-4-20250514"])
    );
}
