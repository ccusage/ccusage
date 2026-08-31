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
        TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage, first_column, format_currency,
        format_models_multiline, format_number, format_rfc3339_millis, parse_ts_timestamp,
        summary_period,
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
    fn snapshots_focused_zcode_cli_reports_for_daily_monthly_and_session() {
        let entries = snapshot_entries();
        let daily = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let monthly = summarize_entries(&entries, AgentReportKind::Monthly).unwrap();
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
            "focused_zcode_session_json",
            report_from_rows(&session, AgentReportKind::Session)
        );
        insta::assert_snapshot!(
            "focused_zcode_daily_table",
            serde_json::to_string_pretty(&table_snapshot(&daily, AgentReportKind::Daily)).unwrap()
        );
        insta::assert_snapshot!(
            "focused_zcode_monthly_table",
            serde_json::to_string_pretty(&table_snapshot(&monthly, AgentReportKind::Monthly))
                .unwrap()
        );
        insta::assert_snapshot!(
            "focused_zcode_session_table",
            serde_json::to_string_pretty(&table_snapshot(&session, AgentReportKind::Session))
                .unwrap()
        );
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

    fn table_snapshot(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
        let show_cache_creation = rows.iter().any(|row| row.cache_creation_tokens > 0);
        let include_last_activity = rows.iter().any(|row| row.last_activity.is_some());
        let mut headers = vec![first_column(kind), "Models", "Input", "Output"];
        if show_cache_creation {
            headers.push("Cache Create");
        }
        headers.extend(["Cache Read", "Total Tokens", "Cost (USD)"]);
        if include_last_activity {
            headers.push("Last Activity");
        }

        let mut rendered_rows = rows
            .iter()
            .map(|row| {
                json!({
                    "kind": "row",
                    "cells": table_row(row, show_cache_creation, include_last_activity),
                })
            })
            .collect::<Vec<_>>();
        let totals = report_from_rows(rows, kind)["totals"].clone();
        let mut total_cells = vec![
            "Total".to_string(),
            String::new(),
            format_number(totals["inputTokens"].as_u64().unwrap()),
            format_number(totals["outputTokens"].as_u64().unwrap()),
        ];
        if show_cache_creation {
            total_cells.push(format_number(
                totals["cacheCreationTokens"].as_u64().unwrap(),
            ));
        }
        total_cells.extend([
            format_number(totals["cacheReadTokens"].as_u64().unwrap()),
            format_number(totals["totalTokens"].as_u64().unwrap()),
            format_currency(totals["totalCost"].as_f64().unwrap()),
        ]);
        if include_last_activity {
            total_cells.push(String::new());
        }
        rendered_rows.push(json!({
            "kind": "total",
            "cells": total_cells,
        }));

        json!({
            "title": "ZCode Token Usage Report",
            "headers": headers,
            "rows": rendered_rows,
        })
    }

    fn table_row(
        row: &crate::UsageSummary,
        show_cache_creation: bool,
        include_last_activity: bool,
    ) -> Vec<String> {
        let mut cells = vec![
            summary_period(row).to_string(),
            format_models_multiline(&row.models_used),
            format_number(row.input_tokens),
            format_number(row.output_tokens),
        ];
        if show_cache_creation {
            cells.push(format_number(row.cache_creation_tokens));
        }
        cells.extend([
            format_number(row.cache_read_tokens),
            format_number(row.total_tokens()),
            format_currency(row.total_cost),
        ]);
        if include_last_activity {
            cells.push(
                row.last_activity
                    .as_deref()
                    .and_then(|value| value.get(..10))
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        cells
    }
}
