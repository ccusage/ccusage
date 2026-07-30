use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| ccusage_core::agent_summary_json(row, kind, kind == AgentReportKind::Session))
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
        AgentReportKind::Monthly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        // Antigravity keeps one database per conversation, so a session is a
        // conversation. Accumulating rather than grouping by key is what carries
        // the activity range and project path onto the row, which the session
        // table and JSON both report.
        AgentReportKind::Session => {
            let mut groups = BTreeMap::<String, SessionAccumulator>::new();
            for entry in entries {
                groups
                    .entry(entry.session_id.to_string())
                    .or_default()
                    .add_entry(entry);
            }
            groups
                .into_values()
                .map(SessionAccumulator::into_summary)
                .collect()
        }
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
    use crate::{TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage};

    fn entry(date: &str, cache_read: u64) -> LoadedEntry {
        let usage = TokenUsageRaw {
            input_tokens: 4050,
            output_tokens: 375,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cache_read,
            speed: None,
            cache_creation: None,
        };
        LoadedEntry {
            data: UsageEntry {
                session_id: Some("conversation-a".to_string()),
                timestamp: format!("{date}T01:02:03.000Z"),
                version: None,
                message: UsageMessage {
                    usage,
                    model: Some("gemini-3.6-flash".to_string()),
                    id: Some("response-a".to_string()),
                },
                cost_usd: None,
                request_id: Some("response-a".to_string()),
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: TimestampMs::from_millis(1_785_328_986_355),
            date: date.to_string(),
            project: Arc::from("antigravity"),
            session_id: Arc::from("conversation-a"),
            project_path: Arc::from("Antigravity"),
            cost: 0.0113,
            credits: None,
            model: Some("gemini-3.6-flash".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 0,
            message_count: None,
        }
    }

    #[test]
    fn reports_cache_reads_separately_from_input_tokens() {
        let rows =
            summarize_entries(&[entry("2026-07-29", 16275)], AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 4050);
        assert_eq!(report["daily"][0]["outputTokens"], 375);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 16275);
        assert_eq!(report["daily"][0]["totalTokens"], 20700);
    }

    #[test]
    fn groups_conversations_under_the_session_report() {
        let rows = summarize_entries(&[entry("2026-07-29", 0)], AgentReportKind::Session).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("conversation-a"));
        assert_eq!(rows[0].project_path.as_deref(), Some("Antigravity"));
        // The activity range is what the session table's Last Activity column and
        // the session JSON report.
        assert_eq!(
            rows[0].last_activity.as_deref(),
            Some("2026-07-29T12:43:06.355Z")
        );
        assert_eq!(rows[0].first_activity, rows[0].last_activity);
    }
}
