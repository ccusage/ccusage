use std::sync::Arc;

use ccusage_adapter_common::source::SourceOptions;
use ccusage_core::{
    LoadedEntry, TokenUsageRaw,
    cli::{AgentReportKind, CostMode, SharedArgs},
    parse_ts_timestamp,
};
use ccusage_test_support::fs_fixture;
use serde_json::json;

#[test]
fn loads_pi_agent_jsonl_usage_entries() {
    let fixture = fs_fixture!({
        "sessions/project-a/prefix_session-a.jsonl": [
            r#"{"type":"message","timestamp":"2026-04-22T01:02:02.000Z","message":{"role":"user","usage":{"input":999,"output":999}}}"#,
            r#"{"type":"message","timestamp":"2026-04-22T01:02:03.000Z","message":{"role":"assistant","model":"gpt-5.4","usage":{"input":100,"output":50,"cacheRead":10,"cacheWrite":20,"totalTokens":180,"cost":{"total":0.05}}}}"#,
        ]
        .join("\n"),
    });
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        mode: CostMode::Display,
        ..SharedArgs::default()
    };
    let sessions_path = fixture.path("sessions");

    let (entries, _) = ccusage_adapter_common::testing::load_entries(
        &ccusage_adapter_pi::SOURCE,
        &shared,
        SourceOptions {
            pi_path: sessions_path.to_str(),
            ..Default::default()
        },
        None,
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date.to_string(), "2026-04-22");
    assert_eq!(entries[0].project.as_ref(), "project-a");
    assert_eq!(entries[0].session_id.as_ref(), "session-a");
    assert_eq!(entries[0].model.as_deref(), Some("[pi] gpt-5.4"));
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.output_tokens, 50);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
    assert_eq!(entries[0].cost, 0.05);
}

#[test]
fn builds_pi_daily_json_report() {
    let entry = LoadedEntry {
        usage: TokenUsageRaw {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
            speed: None,
            cache_creation: None,
        },
        message_id: None,
        version: None,
        recorded_cost: Some(0.05),
        timestamp: parse_ts_timestamp("2026-04-22T01:02:03.000Z").unwrap(),
        date: ccusage_core::cli::IsoDate::parse("2026-04-22").unwrap(),
        project: Arc::from("project-a"),
        session_id: Arc::from("session-a"),
        project_path: Arc::from("project-a"),
        cost: 0.05,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some("[pi] gpt-5.4".to_string()),
        usage_limit_reset_time: None,
        missing_pricing_model: None,
        stored_cost: None,
        estimated_pricing: false,
    };

    let rows = ccusage_adapter_common::source::EntrySource::summarize(
        &ccusage_adapter_pi::SOURCE,
        &[entry],
        ccusage_adapter_common::source::SummaryKind::for_report(
            AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Sunday,
        ),
    )
    .unwrap();
    let report = ccusage_adapter_common::source::EntrySource::report_json(
        &ccusage_adapter_pi::SOURCE,
        &rows,
        AgentReportKind::Daily,
    );

    assert_eq!(report["daily"][0]["date"], "2026-04-22");
    assert_eq!(report["daily"][0]["inputTokens"], 100);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
    assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
    assert_eq!(report["daily"][0]["totalTokens"], 180);
    assert_eq!(report["daily"][0]["totalCost"], json!(0.05));
    assert_eq!(report["daily"][0]["modelsUsed"], json!(["[pi] gpt-5.4"]));
    assert_eq!(
        report["daily"][0]["modelBreakdowns"],
        json!([{
            "modelName": "[pi] gpt-5.4",
            "inputTokens": 100,
            "outputTokens": 50,
            "cacheCreationTokens": 20,
            "cacheReadTokens": 10,
            "cost": 0.05
        }])
    );
}
