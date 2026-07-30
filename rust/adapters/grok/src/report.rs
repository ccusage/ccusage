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
        project_path: &str,
    ) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(millis);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T12:43:06.355Z"),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: input,
                        output_tokens: 20,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: cache_read,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("grok-4.5-build".to_string()),
                    id: Some(format!("evt-{millis}")),
                },
                cost_usd: None,
                request_id: Some(format!("evt-{millis}")),
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: date.to_string(),
            project: Arc::from("grok"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost: 0.0113,
            credits: None,
            model: Some("grok-4.5-build".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 10,
            message_count: None,
        }
    }

    #[test]
    fn reports_cache_reads_separately_from_input_tokens() {
        let rows = summarize_entries(
            &[entry(
                "sess-a",
                "2026-07-29",
                1_785_328_986_355,
                60,
                40,
                "/tmp/proj",
            )],
            AgentReportKind::Daily,
        )
        .unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 60);
        assert_eq!(report["daily"][0]["outputTokens"], 20);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 40);
        // totalTokens includes uncached input + output + cache read + reasoning.
        assert_eq!(report["daily"][0]["totalTokens"], 130);
    }

    #[test]
    fn groups_conversations_under_the_session_report() {
        let rows = summarize_entries(
            &[
                entry(
                    "sess-a",
                    "2026-07-28",
                    1_785_242_586_000,
                    60,
                    0,
                    "D:\\work\\proj",
                ),
                entry(
                    "sess-a",
                    "2026-07-29",
                    1_785_328_986_355,
                    40,
                    20,
                    "D:\\work\\proj",
                ),
            ],
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-a"));
        assert_eq!(rows[0].project_path.as_deref(), Some("D:\\work\\proj"));
        assert_eq!(rows[0].input_tokens, 100);
        assert_eq!(rows[0].cache_read_tokens, 20);
        assert_eq!(rows[0].extra_total_tokens, 20);
        // The activity range is what the session table's Last Activity column and
        // the session JSON report.
        assert_eq!(
            rows[0].first_activity.as_deref(),
            Some("2026-07-28T12:43:06.000Z")
        );
        assert_eq!(
            rows[0].last_activity.as_deref(),
            Some("2026-07-29T12:43:06.355Z")
        );
    }

    #[test]
    fn session_json_includes_activity_and_project_path() {
        let rows = summarize_entries(
            &[entry(
                "sess-a",
                "2026-07-29",
                1_785_328_986_355,
                60,
                40,
                "/workspace/api",
            )],
            AgentReportKind::Session,
        )
        .unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Session);

        assert_eq!(report["sessions"][0]["sessionId"], "sess-a");
        assert_eq!(report["sessions"][0]["projectPath"], "/workspace/api");
        assert_eq!(
            report["sessions"][0]["lastActivity"],
            "2026-07-29T12:43:06.355Z"
        );
        assert_eq!(
            report["sessions"][0]["firstActivity"],
            "2026-07-29T12:43:06.355Z"
        );
    }
}
