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
    use crate::{TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage};

    fn entry(session_id: &str, date: &str, millis: i64, project_path: &str) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T12:00:00.000Z"),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("gemini-3-pro".to_string()),
                    id: Some(format!("{session_id}-{millis}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
                attribution_plugin: None,
                attribution_skill: None,
            },
            timestamp: TimestampMs::from_millis(millis),
            date: date.to_string(),
            project: Arc::from("antigravity"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost: 0.25,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("gemini-3-pro".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        }
    }

    #[test]
    fn session_json_includes_activity_bounds_and_project_metadata() {
        let rows = summarize_entries(
            &[
                entry(
                    "session-a",
                    "2026-05-04",
                    1_778_000_000_000,
                    "/workspace/app",
                ),
                entry(
                    "session-a",
                    "2026-05-05",
                    1_778_086_400_000,
                    "/workspace/app",
                ),
            ],
            AgentReportKind::Session,
        )
        .unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Session);

        assert_eq!(report["sessions"][0]["sessionId"], "session-a");
        assert_eq!(report["sessions"][0]["projectPath"], "/workspace/app");
        assert_eq!(
            report["sessions"][0]["firstActivity"],
            "2026-05-05T16:53:20.000Z"
        );
        assert_eq!(
            report["sessions"][0]["lastActivity"],
            "2026-05-06T16:53:20.000Z"
        );
    }

    #[test]
    fn weekly_report_buckets_from_monday() {
        let rows = summarize_entries(
            &[
                entry("sunday", "2026-05-03", 1_778_000_000_000, "/workspace/app"),
                entry("monday", "2026-05-04", 1_778_086_400_000, "/workspace/app"),
            ],
            AgentReportKind::Weekly,
        )
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].week.as_deref(), Some("2026-04-27"));
        assert_eq!(rows[1].week.as_deref(), Some("2026-05-04"));
    }
}
