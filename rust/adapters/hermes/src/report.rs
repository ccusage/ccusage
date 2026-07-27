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
    fn report_includes_message_count_and_reasoning_total() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-1".to_string()),
                timestamp: "2025-06-15T15:06:40.250Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 1200,
                        output_tokens: 300,
                        cache_creation_input_tokens: 20,
                        cache_read_input_tokens: 50,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some("hermes:session-1".to_string()),
                },
                cost_usd: Some(0.34),
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: crate::parse_ts_timestamp("2025-06-15T15:06:40.250Z").unwrap(),
            date: "2025-06-15".to_string(),
            project: Arc::from("hermes"),
            session_id: Arc::from("session-1"),
            project_path: Arc::from("Hermes"),
            cost: 0.34,
            credits: None,
            extra_total_tokens: 10,
            message_count: Some(42),
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        };
        let rows = summarize_entries(&[entry], AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["totalTokens"], json!(1580));
        assert_eq!(report["daily"][0]["messageCount"], json!(42));
        assert_eq!(report["totals"]["totalTokens"], json!(1580));
    }
}
