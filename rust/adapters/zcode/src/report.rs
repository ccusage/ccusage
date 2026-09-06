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
    use crate::{
        TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage, format_rfc3339_millis,
        parse_ts_timestamp,
    };
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
                attribution_plugin: None,
                attribution_skill: None,
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

    #[test]
    fn snapshots_focused_zcode_json_reports_for_daily_monthly_weekly_and_session() {
        let entries = snapshot_entries();
        let daily = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let monthly = summarize_entries(&entries, AgentReportKind::Monthly).unwrap();
        let weekly = summarize_entries(&entries, AgentReportKind::Weekly).unwrap();
        let session = summarize_entries(&entries, AgentReportKind::Session).unwrap();

        insta::assert_json_snapshot!(
            "focused_zcode_daily_json",
            report_from_rows(&daily, AgentReportKind::Daily)
        );
        insta::assert_json_snapshot!(
            "focused_zcode_monthly_json",
            report_from_rows(&monthly, AgentReportKind::Monthly)
        );
        insta::assert_json_snapshot!(
            "focused_zcode_weekly_json",
            report_from_rows(&weekly, AgentReportKind::Weekly)
        );
        insta::assert_json_snapshot!(
            "focused_zcode_session_json",
            report_from_rows(&session, AgentReportKind::Session)
        );
    }

    #[test]
    fn saturates_focused_daily_and_weekly_reports_for_extreme_counters() {
        let entries = [
            extreme_entry("2099-01-02", 4_070_908_800_000, u64::MAX),
            extreme_entry("2099-01-03", 4_071_081_600_000, 1),
        ];
        let daily = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let daily_report = report_from_rows(&daily, AgentReportKind::Daily);
        let weekly = summarize_entries(&entries, AgentReportKind::Weekly).unwrap();
        let weekly_report = report_from_rows(&weekly, AgentReportKind::Weekly);

        for report in [daily_report, weekly_report] {
            for key in [
                "inputTokens",
                "outputTokens",
                "cacheCreationTokens",
                "cacheReadTokens",
                "totalTokens",
            ] {
                assert_eq!(report["totals"][key], u64::MAX, "{key}");
            }
        }
        assert_eq!(weekly[0].model_breakdowns[0].input_tokens, u64::MAX);
        assert_eq!(weekly[0].model_breakdowns[0].output_tokens, u64::MAX);
    }

    fn extreme_entry(date: &str, millis: i64, tokens: u64) -> LoadedEntry {
        let mut entry = entry("session-extreme", date, millis);
        entry.data.message.usage = TokenUsageRaw {
            input_tokens: tokens,
            output_tokens: tokens,
            cache_creation_input_tokens: tokens,
            cache_read_input_tokens: tokens,
            ..TokenUsageRaw::default()
        };
        entry.extra_total_tokens = tokens;
        entry
    }

    fn snapshot_entries() -> Vec<LoadedEntry> {
        [
            (
                "usage-52",
                "session-a",
                "2099-01-02T00:00:00.000Z",
                "GLM-5.2",
                60,
                10,
                15,
                25,
                0.00015549999999999999,
                "/workspace/project-a",
            ),
            (
                "usage-53",
                "session-a",
                "2099-01-15T12:00:00.000Z",
                "GLM-5.3",
                130,
                20,
                30,
                40,
                0.0003224,
                "/workspace/project-a",
            ),
            (
                "usage-53-b",
                "session-b",
                "2099-02-01T00:00:00.000Z",
                "GLM-5.3",
                40,
                5,
                0,
                10,
                0.0000806,
                "/workspace/project-b",
            ),
        ]
        .into_iter()
        .map(
            |(
                id,
                session_id,
                timestamp,
                model,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                cost,
                project_path,
            )| {
                let date = timestamp.get(..10).unwrap().to_string();
                let timestamp = parse_ts_timestamp(timestamp).unwrap();
                LoadedEntry {
                    data: UsageEntry {
                        session_id: Some(session_id.to_string()),
                        timestamp: format_rfc3339_millis(timestamp),
                        version: Some("0.16.3".to_string()),
                        message: UsageMessage {
                            usage: TokenUsageRaw {
                                input_tokens,
                                output_tokens,
                                cache_creation_input_tokens: cache_creation_tokens,
                                cache_read_input_tokens: cache_read_tokens,
                                speed: None,
                                cache_creation: None,
                            },
                            model: Some(model.to_string()),
                            id: Some(id.to_string()),
                        },
                        cost_usd: None,
                        request_id: None,
                        is_api_error_message: None,
                        is_sidechain: None,
                        attribution_plugin: None,
                        attribution_skill: None,
                    },
                    timestamp,
                    date,
                    project: Arc::from("zcode"),
                    session_id: Arc::from(session_id),
                    project_path: Arc::from(project_path),
                    cost,
                    extra_total_tokens: 0,
                    credits: None,
                    message_count: None,
                    model: Some(model.to_string()),
                    usage_limit_reset_time: None,
                    missing_pricing_model: None,
                }
            },
        )
        .collect()
    }
}
