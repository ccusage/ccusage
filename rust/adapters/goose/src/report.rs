use serde_json::Value;

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

    #[test]
    fn includes_goose_reasoning_remainder_in_report_total() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-05-01T01:02:03.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some("session-a".to_string()),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: crate::parse_ts_timestamp("2026-05-01T01:02:03.000Z").unwrap(),
            date: "2026-05-01".to_string(),
            project: Arc::from("goose"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("Goose"),
            cost: 0.02,
            credits: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 30,
            message_count: None,
        };
        let rows = summarize_entries(&[entry], AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
    }
}
