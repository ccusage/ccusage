use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator, agent_summary_json,
    cli::{AgentReportKind, SortOrder, WeekDay},
    sort_summaries, summarize_by_key, summarize_summaries_by_bucket, summary_period, totals_json,
};

pub fn report_json(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
    order: &SortOrder,
) -> Result<Value> {
    let mut rows = summarize_entries(entries, kind)?;
    sort_summaries(&mut rows, order, |row| summary_period(row));
    Ok(report_from_rows(&rows, kind))
}

fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| agent_summary_json(row, kind, false))
        .collect::<Vec<_>>();
    json!({
        rows_key(kind): rows_json,
        "totals": totals_json(rows),
    })
}

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
) -> Result<Vec<crate::UsageSummary>> {
    match kind {
        AgentReportKind::Daily => summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        ),
        AgentReportKind::Weekly => {
            let daily = summarize_by_key(
                entries,
                |entry| entry.date.clone(),
                |date| (date.to_string(), None),
            )?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Monday,
            ))
        }
        AgentReportKind::Monthly => {
            let daily = summarize_by_key(
                entries,
                |entry| entry.date.clone(),
                |date| (date.to_string(), None),
            )?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Session => {
            let mut grouped: Vec<SessionAccumulator> = Vec::new();
            let mut group_indexes = std::collections::HashMap::new();
            for entry in entries {
                let key = &entry.session_id;
                let index = *group_indexes.entry(key.clone()).or_insert_with(|| {
                    let index = grouped.len();
                    grouped.push(SessionAccumulator::default());
                    index
                });
                grouped[index].add_entry(entry);
            }
            let mut rows = Vec::with_capacity(grouped.len());
            for group in grouped {
                rows.push(group.into_summary()?);
            }
            Ok(rows)
        }
    }
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "sessions",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        LoadedEntry, ModelBreakdown, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
        UsageSummary, cli::AgentReportKind, format_rfc3339_millis,
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
            "dailyReport": report_from_rows(std::slice::from_ref(&daily), AgentReportKind::Daily),
            "sessionReport": report_from_rows(&[session], AgentReportKind::Session),
        }));
    }

    #[test]
    fn summarize_session_entries_preserves_session_id_and_activity_bounds() {
        let entries = vec![
            loaded_entry("session-a", 1_767_316_800_000, 100),
            loaded_entry("session-a", 1_767_402_000_000, 20),
        ];

        let rows = summarize_entries(&entries, AgentReportKind::Session).unwrap();

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
                extra_total_tokens: 7,
                cost: 0.25,
                missing_pricing: false,
            }],
            project: None,
            versions: Some(vec!["1.0.0".to_string()]),
        }
    }

    fn loaded_entry(session_id: &str, timestamp_millis: i64, input_tokens: u64) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(timestamp_millis);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format_rfc3339_millis(timestamp),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens,
                        output_tokens: 0,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        cache_creation: None,
                        speed: None,
                    },
                    model: Some("gpt-5.2-codex".to_string()),
                    id: Some(format!("msg-{timestamp_millis}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: "2026-01-02".to_string(),
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
        }
    }
}
