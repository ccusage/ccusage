use serde_json::Value;

use crate::{
    BucketKind, LoadedEntry, Result, UsageSummary,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub(crate) fn report_from_rows(rows: &[UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| crate::adapter::opencode::agent_summary_json(row, kind, false))
        .collect::<Vec<_>>();
    serde_json::json!({
        rows_key(kind): rows_json,
        "totals": totals_json(rows),
    })
}

pub(super) fn summary_period(row: &UsageSummary) -> &str {
    row.date
        .as_deref()
        .or(row.month.as_deref())
        .or(row.session_id.as_deref())
        .unwrap_or("")
}

pub(crate) fn summarize_entries(
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
                WeekDay::Sunday,
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

    fn make_entry(session_id: &str, date: &str, input: u64, output: u64, cost: f64) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T01:00:00.000Z"),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: input,
                        output_tokens: output,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("goose-claude-4-6-sonnet".to_string()),
                    id: Some(session_id.to_string()),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: crate::parse_ts_timestamp(&format!("{date}T01:00:00.000Z")).unwrap(),
            date: date.to_string(),
            project: Arc::from("buzz"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("Buzz"),
            cost,
            credits: None,
            model: Some("goose-claude-4-6-sonnet".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 0,
            message_count: None,
        }
    }

    #[test]
    fn aggregates_daily_totals_across_sessions() {
        let entries = vec![
            make_entry("ses_a", "2026-07-07", 1000, 100, 0.01),
            make_entry("ses_b", "2026-07-07", 2000, 200, 0.02),
            make_entry("ses_c", "2026-07-08", 500, 50, 0.005),
        ];

        let rows = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        assert_eq!(rows.len(), 2);
        // Day 1: ses_a + ses_b
        let day1 = rows
            .iter()
            .find(|r| r.date.as_deref() == Some("2026-07-07"))
            .unwrap();
        assert_eq!(day1.input_tokens, 3000);
        assert_eq!(day1.output_tokens, 300);
        assert!((day1.total_cost - 0.03).abs() < 1e-9);
    }

    #[test]
    fn aggregates_session_totals_per_session_id() {
        let entries = vec![
            // Two turns in the same session
            make_entry("ses_a", "2026-07-07", 1000, 100, 0.01),
            make_entry("ses_a", "2026-07-07", 2000, 200, 0.02),
            make_entry("ses_b", "2026-07-07", 500, 50, 0.005),
        ];

        let rows = summarize_entries(&entries, AgentReportKind::Session).unwrap();
        assert_eq!(rows.len(), 2);
        let ses_a = rows
            .iter()
            .find(|r| r.session_id.as_deref() == Some("ses_a"))
            .unwrap();
        assert_eq!(ses_a.input_tokens, 3000);
        assert_eq!(ses_a.output_tokens, 300);
    }

    #[test]
    fn builds_json_report_with_totals() {
        let entries = vec![make_entry("ses_x", "2026-07-07", 100, 10, 0.01)];
        let rows = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 10);
        assert_eq!(report["totals"]["inputTokens"], 100);
    }
}
