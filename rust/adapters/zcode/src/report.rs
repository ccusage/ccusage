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
        // Accumulating rather than grouping by key is what carries the activity
        // range and project path onto the row, which the session table and JSON
        // both report.
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

    fn entry(
        session_id: &str,
        date: &str,
        millis: i64,
        input: u64,
        cache_read: u64,
    ) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(millis);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T12:43:06.355Z"),
                version: Some("0.16.3".to_string()),
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: input,
                        output_tokens: 20,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: cache_read,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("glm-5.3".to_string()),
                    id: Some(format!("usage-{millis}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: date.to_string(),
            project: Arc::from("zcode"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("/work/proj"),
            cost: 0.01,
            credits: None,
            model: Some("glm-5.3".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 0,
            message_count: None,
        }
    }

    #[test]
    fn reports_cache_reads_separately_from_input_tokens() {
        let rows = summarize_entries(
            &[entry("sess-a", "2026-08-16", 1_786_909_042_666, 60, 40)],
            AgentReportKind::Daily,
        )
        .unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 60);
        assert_eq!(report["daily"][0]["outputTokens"], 20);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 40);
        assert_eq!(report["daily"][0]["totalTokens"], 120);
    }

    #[test]
    fn session_rows_carry_activity_range_and_project_path() {
        let rows = summarize_entries(
            &[
                entry("sess-a", "2026-08-16", 1_786_909_042_000, 60, 0),
                entry("sess-a", "2026-08-17", 1_786_995_442_000, 40, 20),
            ],
            AgentReportKind::Session,
        )
        .unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Session);

        assert_eq!(rows.len(), 1);
        assert_eq!(report["sessions"][0]["sessionId"], "sess-a");
        assert_eq!(report["sessions"][0]["projectPath"], "/work/proj");
        assert_eq!(
            report["sessions"][0]["firstActivity"],
            "2026-08-16T19:37:22.000Z"
        );
        assert_eq!(
            report["sessions"][0]["lastActivity"],
            "2026-08-17T19:37:22.000Z"
        );
    }
}
