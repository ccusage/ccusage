use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub(crate) fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
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
    use super::*;
    use crate::{TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage};
    use std::sync::Arc;

    fn entry(session_id: &str, date: &str, millis: i64) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(millis);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T12:00:00.000Z"),
                version: Some("0.16.3".to_string()),
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 60,
                        output_tokens: 10,
                        cache_creation_input_tokens: 15,
                        cache_read_input_tokens: 25,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("GLM-5.3".to_string()),
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
            project_path: Arc::from("/project"),
            cost: 0.01,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("GLM-5.3".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        }
    }

    #[test]
    fn reports_daily_and_session_totals() {
        let entries = [entry("session-1", "2026-08-16", 1_786_909_042_666)];
        let daily = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&daily, AgentReportKind::Daily);
        assert_eq!(report["daily"][0]["totalTokens"], 110);
        assert_eq!(report["totals"]["totalTokens"], 110);

        let sessions = summarize_entries(&entries, AgentReportKind::Session).unwrap();
        assert_eq!(sessions[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(sessions[0].project_path.as_deref(), Some("/project"));
    }
}
