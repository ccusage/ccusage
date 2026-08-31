use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket,
};

pub fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| ccusage_core::agent_summary_json(row, kind, kind == AgentReportKind::Session))
        .collect::<Vec<_>>();
    serde_json::json!({
        rows_key(kind): rows_json,
        "totals": crate::totals_json(rows),
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
    use std::sync::Arc;

    use super::*;
    use crate::{
        Align, SimpleTable, TerminalStyle, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
        first_column, format_currency, format_models_multiline, format_number,
        format_rfc3339_millis, totals_json,
    };

    #[test]
    fn snapshots_monthly_and_session_reports() {
        let entries = vec![
            loaded_entry(
                "session-a",
                "2026-01-02",
                1_767_316_800_000,
                "claude-opus-4.7",
                usage(70, 50, 20, 10),
                0.00173,
            ),
            loaded_entry(
                "session-a",
                "2026-01-03",
                1_767_403_200_000,
                "claude-opus-4.7",
                usage(10, 5, 0, 2),
                0.0001,
            ),
            loaded_entry(
                "session-b",
                "2026-02-01",
                1_769_908_800_000,
                "gpt-5.4",
                usage(5, 3, 1, 0),
                0.0002,
            ),
        ];
        let monthly = summarize_entries(&entries, AgentReportKind::Monthly).unwrap();
        let sessions = summarize_entries(&entries, AgentReportKind::Session).unwrap();

        insta::assert_json_snapshot!(serde_json::json!({
            "monthly": report_from_rows(&monthly, AgentReportKind::Monthly),
            "sessions": report_from_rows(&sessions, AgentReportKind::Session),
        }));
    }

    #[test]
    fn snapshots_monthly_and_session_tables() {
        let entries = vec![
            loaded_entry(
                "session-a",
                "2026-01-02",
                1_767_316_800_000,
                "claude-opus-4.7",
                usage(70, 50, 20, 10),
                0.00173,
            ),
            loaded_entry(
                "session-a",
                "2026-01-03",
                1_767_403_200_000,
                "claude-opus-4.7",
                usage(10, 5, 0, 2),
                0.0001,
            ),
            loaded_entry(
                "session-b",
                "2026-02-01",
                1_769_908_800_000,
                "gpt-5.4",
                usage(5, 3, 1, 0),
                0.0002,
            ),
        ];
        let monthly = summarize_entries(&entries, AgentReportKind::Monthly).unwrap();
        let sessions = summarize_entries(&entries, AgentReportKind::Session).unwrap();

        insta::assert_snapshot!(format!(
            "Monthly\n{}\n\nSession\n{}",
            render_table(&monthly, AgentReportKind::Monthly),
            render_table(&sessions, AgentReportKind::Session),
        ));
    }

    fn render_table(rows: &[crate::UsageSummary], kind: AgentReportKind) -> String {
        let include_last_activity = rows.iter().any(|row| row.last_activity.is_some());
        let mut headers = vec![
            first_column(kind),
            "Models",
            "Input",
            "Output",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ];
        let mut aligns = vec![
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ];
        if include_last_activity {
            headers.push("Last Activity");
            aligns.push(Align::Left);
        }
        let mut table = SimpleTable::new(
            headers,
            aligns,
            TerminalStyle {
                no_color: true,
                ..TerminalStyle::default()
            },
        )
        .with_terminal_width(120)
        .with_date_compaction(true);
        for row in rows {
            let mut values = vec![
                crate::summary_period(row).to_string(),
                format_models_multiline(&row.models_used),
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format_number(row.cache_creation_tokens),
                format_number(row.cache_read_tokens),
                format_number(row.total_tokens()),
                format_currency(row.total_cost),
            ];
            if include_last_activity {
                values.push(
                    row.last_activity
                        .as_deref()
                        .and_then(|value| value.get(..10))
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            table.push(values);
        }
        let totals = totals_json(rows);
        table.separator();
        let mut total = vec![
            "Total".to_string(),
            String::new(),
            format_number(totals["inputTokens"].as_u64().unwrap_or_default()),
            format_number(totals["outputTokens"].as_u64().unwrap_or_default()),
            format_number(totals["cacheCreationTokens"].as_u64().unwrap_or_default()),
            format_number(totals["cacheReadTokens"].as_u64().unwrap_or_default()),
            format_number(totals["totalTokens"].as_u64().unwrap_or_default()),
            format_currency(totals["totalCost"].as_f64().unwrap_or_default()),
        ];
        if include_last_activity {
            total.push(String::new());
        }
        table.push(total);
        table.render()
    }

    fn loaded_entry(
        session_id: &str,
        date: &str,
        timestamp_millis: i64,
        model: &str,
        usage: TokenUsageRaw,
        cost: f64,
    ) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(timestamp_millis);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format_rfc3339_millis(timestamp),
                version: None,
                message: UsageMessage {
                    usage,
                    model: Some(model.to_string()),
                    id: Some(format!("{session_id}-{timestamp_millis}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: date.to_string(),
            project: Arc::from("copilot"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("/workspace/copilot"),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some(model.to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        }
    }

    fn usage(
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_input_tokens: u64,
        cache_read_input_tokens: u64,
    ) -> TokenUsageRaw {
        TokenUsageRaw {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            speed: None,
            cache_creation: None,
        }
    }
}
