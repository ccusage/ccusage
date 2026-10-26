use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, UsageSummary,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub fn report_from_rows(rows: &[UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| ccusage_core::agent_summary_json(row, kind, false))
        .collect::<Vec<_>>();
    json!({
        rows_key(kind): rows_json,
        "totals": totals_json(rows),
    })
}

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
) -> Result<Vec<UsageSummary>> {
    match kind {
        AgentReportKind::Daily => summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        ),
        AgentReportKind::Monthly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Session => summarize_by_key(
            entries,
            |entry| entry.session_id.to_string(),
            |session_id| (session_id.to_string(), None),
        )
        .map(|mut rows| {
            for row in &mut rows {
                row.session_id = row.date.take();
            }
            rows
        }),
        AgentReportKind::Weekly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Monday,
            ))
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
    use crate::{TokenUsageRaw, UsageEntry, UsageMessage};

    #[test]
    fn snapshots_daily_weekly_monthly_and_session_json_reports() {
        let entries = snapshot_entries();
        let daily = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let weekly = summarize_entries(&entries, AgentReportKind::Weekly).unwrap();
        let monthly = summarize_entries(&entries, AgentReportKind::Monthly).unwrap();
        let sessions = summarize_entries(&entries, AgentReportKind::Session).unwrap();
        assert_eq!(
            weekly
                .iter()
                .map(|row| row.week.as_deref())
                .collect::<Vec<_>>(),
            [Some("2026-10-26"), Some("2026-11-02")]
        );

        insta::assert_json_snapshot!(
            "cline_daily_json_report",
            report_from_rows(&daily, AgentReportKind::Daily)
        );
        insta::assert_json_snapshot!(
            "cline_weekly_json_report",
            report_from_rows(&weekly, AgentReportKind::Weekly)
        );
        insta::assert_json_snapshot!(
            "cline_monthly_json_report",
            report_from_rows(&monthly, AgentReportKind::Monthly)
        );
        insta::assert_json_snapshot!(
            "cline_session_json_report",
            report_from_rows(&sessions, AgentReportKind::Session)
        );
    }

    fn snapshot_entries() -> Vec<LoadedEntry> {
        [
            entry(
                "task:task-a",
                "2026-10-31T12:00:00Z",
                "claude-sonnet-4-20250514",
                TokenUsageRaw {
                    input_tokens: 100,
                    output_tokens: 25,
                    cache_creation_input_tokens: 15,
                    cache_read_input_tokens: 10,
                    ..TokenUsageRaw::default()
                },
                0.002,
                Some(0.002),
                None,
            ),
            entry(
                "cli:task-a",
                "2026-11-01T12:00:00Z",
                "claude-opus-4-20250514",
                TokenUsageRaw {
                    input_tokens: 40,
                    output_tokens: 12,
                    cache_creation_input_tokens: 5,
                    cache_read_input_tokens: 5,
                    ..TokenUsageRaw::default()
                },
                0.001,
                Some(0.001),
                None,
            ),
            entry(
                "task:task-b",
                "2026-11-02T12:00:00Z",
                "unknown",
                TokenUsageRaw {
                    input_tokens: 12,
                    output_tokens: 3,
                    ..TokenUsageRaw::default()
                },
                0.0,
                None,
                Some("unknown"),
            ),
        ]
        .into()
    }

    fn entry(
        session_id: &str,
        timestamp: &str,
        model: &str,
        usage: TokenUsageRaw,
        cost: f64,
        stored_cost: Option<f64>,
        missing_pricing_model: Option<&str>,
    ) -> LoadedEntry {
        let timestamp = crate::parse_ts_timestamp(timestamp).expect("valid snapshot timestamp");
        let timestamp_text = crate::format_rfc3339_millis(timestamp);
        let date = crate::format_date(timestamp, None);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: timestamp_text,
                version: None,
                message: UsageMessage {
                    usage,
                    model: Some(model.to_string()),
                    id: Some(format!("request-{date}")),
                },
                cost_usd: stored_cost,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date,
            project: Arc::from("cline"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("Cline"),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some(model.to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: missing_pricing_model.map(str::to_string),
            stored_cost,
        }
    }
}
