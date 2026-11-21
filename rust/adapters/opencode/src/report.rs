use serde_json::Value;

use crate::{
    LoadedEntry, Result,
    cli::{AgentReportKind, SortOrder, WeekDay},
    sort_summaries, summary_period,
};

use ccusage_adapter_common::{
    rows,
    source::{SessionScope, SessionShape, SummaryKind},
};

pub fn report_json(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
    order: &SortOrder,
    start_of_week: WeekDay,
) -> Result<Value> {
    let mut rows = summarize_entries(entries, kind, start_of_week)?;
    sort_summaries(&mut rows, order, &summary_period);
    Ok(ccusage_adapter_common::report::report_json(
        &rows, kind, false,
    ))
}

pub(crate) fn summarize_entries(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
    start_of_week: WeekDay,
) -> Result<Vec<crate::UsageSummary>> {
    if kind == AgentReportKind::Session {
        return rows::summarize_source_sessions_in_order(entries);
    }
    rows::summarize_entries(
        entries,
        SummaryKind::for_report(kind, start_of_week),
        SessionShape::WithActivity,
        SessionScope::Source,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use ccusage_core::{
        LoadedEntry, ModelBreakdown, TimestampMs, TokenUsageRaw, UsageSummary, agent_summary_json,
        cli::AgentReportKind,
    };

    #[test]
    fn snapshots_agent_summary_json_period_keys_and_session_metadata() {
        let daily = snapshot_row();
        let mut weekly = snapshot_row();
        weekly.date = None;
        weekly.month = None;
        let mut monthly = snapshot_row();
        monthly.date = None;
        monthly.week = None;
        let mut session = snapshot_row();
        session.date = None;
        session.week = None;
        session.month = None;

        insta::assert_json_snapshot!(serde_json::json!({
            "daily": agent_summary_json(&daily, AgentReportKind::Daily, false),
            "weekly": agent_summary_json(&weekly, AgentReportKind::Weekly, false),
            "monthly": agent_summary_json(&monthly, AgentReportKind::Monthly, false),
            "session": agent_summary_json(&session, AgentReportKind::Session, true),
            "dailyReport": ccusage_adapter_common::report::report_json(std::slice::from_ref(&daily), AgentReportKind::Daily, false),
            "sessionReport": ccusage_adapter_common::report::report_json(&[session], AgentReportKind::Session, false),
        }));
    }

    #[test]
    fn summarize_session_entries_preserves_session_id_and_activity_bounds() {
        let entries = vec![
            loaded_entry("session-a", 1_767_316_800_000, 100),
            loaded_entry("session-a", 1_767_402_000_000, 20),
        ];

        let rows = summarize_entries(&entries, AgentReportKind::Session, WeekDay::Monday).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("session-a"));
        assert_eq!(
            rows[0].first_activity.as_deref(),
            Some("2026-01-02T01:20:00.000Z")
        );
        assert_eq!(
            rows[0].last_activity.as_deref(),
            Some("2026-01-03T01:00:00.000Z")
        );
    }

    #[test]
    fn weekly_report_uses_the_requested_week_start() {
        let entries = vec![loaded_entry("session-a", 1_767_316_800_000, 100)];
        for (start, expected) in [
            (WeekDay::Monday, "2025-12-29"),
            (WeekDay::Sunday, "2025-12-28"),
        ] {
            let rows = summarize_entries(&entries, AgentReportKind::Weekly, start).unwrap();
            assert_eq!(rows[0].week.as_deref(), Some(expected));
            assert_eq!(rows[0].input_tokens, 100);
        }
    }

    fn snapshot_row() -> UsageSummary {
        UsageSummary {
            date: Some("2026-01-02".to_string()),
            month: Some("2026-01".to_string()),
            week: Some("2025-12-29".to_string()),
            session_id: Some("session-a".to_string()),
            project_path: Some("/workspace/api".to_string()),
            last_activity: Some("2026-01-02T12:34:56.000Z".to_string()),
            first_activity: Some("2026-01-01T10:30:00.000Z".to_string()),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            extra_total_tokens: 7,
            total_cost: 0.25,
            credits: Some(1.5),
            message_count: Some(3),
            models_used: vec![
                "gpt-5.2-codex".to_string(),
                "claude-sonnet-4-20250514".to_string(),
            ],
            model_breakdowns: vec![ModelBreakdown {
                model_name: "gpt-5.2-codex".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 5,
                cache_creation_1h_tokens: None,
                extra_total_tokens: 7,
                cost: 0.25,
                missing_pricing: false,
                stored_cost: None,
                estimated_pricing: false,
            }],
            project: None,
            versions: Some(vec!["1.0.0".to_string()]),
        }
    }

    fn loaded_entry(session_id: &str, timestamp_millis: i64, input_tokens: u64) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(timestamp_millis);
        LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                cache_creation: None,
                speed: None,
            },
            message_id: Some(format!("msg-{timestamp_millis}")),
            version: None,
            recorded_cost: None,
            timestamp,
            date: ccusage_core::cli::IsoDate::parse("2026-01-02").unwrap(),
            project: Arc::from("opencode"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("/workspace/api"),
            cost: 0.0,
            extra_total_tokens: 0,
            credits: None,
            message_count: Some(1),
            model: Some("gpt-5.2-codex".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            stored_cost: None,
            estimated_pricing: false,
        }
    }
}
