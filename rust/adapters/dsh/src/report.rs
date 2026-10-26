use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| crate::agent_summary_json(row, kind, false))
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
        AgentReportKind::Weekly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Monday,
            ))
        }
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
    use std::{env, process::Command, sync::Arc};

    use super::*;
    use crate::{
        TokenUsageRaw, UsageEntry, UsageMessage, format_rfc3339_millis, parse_ts_timestamp,
    };

    const TABLE_HELPER_ENV: &str = "CCUSAGE_DSH_TABLE_SNAPSHOT_KIND";
    const TABLE_START: &str = "--- DSH_TABLE_START ---\n";
    const TABLE_END: &str = "--- DSH_TABLE_END ---";

    #[test]
    fn snapshots_json_reports_for_all_report_kinds() {
        let entries = snapshot_entries();
        let report = |kind| {
            let rows = summarize_entries(&entries, kind).unwrap();
            report_from_rows(&rows, kind)
        };

        insta::assert_json_snapshot!(serde_json::json!({
            "daily": report(AgentReportKind::Daily),
            "weekly": report(AgentReportKind::Weekly),
            "monthly": report(AgentReportKind::Monthly),
            "sessions": report(AgentReportKind::Session),
        }));
    }

    #[test]
    fn weekly_buckets_start_on_monday_across_sunday_boundary() {
        let entries = vec![
            loaded_entry("sunday", "/workspace/api", "2026-10-25", 10, 5, 0, 0, 0.01),
            loaded_entry("monday", "/workspace/api", "2026-10-26", 20, 10, 0, 0, 0.02),
        ];

        let rows = summarize_entries(&entries, AgentReportKind::Weekly).unwrap();
        let buckets = rows
            .iter()
            .filter_map(|row| row.week.as_deref())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(rows.len(), 2);
        assert_eq!(
            buckets,
            std::collections::BTreeSet::from(["2026-10-19", "2026-10-26"])
        );
    }

    #[test]
    fn snapshots_terminal_tables_for_all_report_kinds() {
        let executable = env::current_exe().unwrap();
        for name in ["daily", "weekly", "monthly", "session"] {
            let output = Command::new(&executable)
                .args([
                    "--exact",
                    "report::tests::render_table_snapshot_helper",
                    "--nocapture",
                ])
                .env(TABLE_HELPER_ENV, name)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "table helper failed for {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = String::from_utf8(output.stdout).unwrap();
            let (_, rendered) = output.split_once(TABLE_START).unwrap_or_else(|| {
                panic!("table helper did not emit start marker for {name}: {output}")
            });
            let (rendered, _) = rendered.split_once(TABLE_END).unwrap_or_else(|| {
                panic!("table helper did not emit end marker for {name}: {output}")
            });
            insta::assert_snapshot!(format!("{name}-table"), rendered.trim());
        }
    }

    #[test]
    fn render_table_snapshot_helper() {
        let Ok(name) = env::var(TABLE_HELPER_ENV) else {
            return;
        };
        let kind = match name.as_str() {
            "daily" => AgentReportKind::Daily,
            "weekly" => AgentReportKind::Weekly,
            "monthly" => AgentReportKind::Monthly,
            "session" => AgentReportKind::Session,
            _ => panic!("unknown report kind {name}"),
        };
        let entries = snapshot_entries();
        let rows = summarize_entries(&entries, kind).unwrap();
        let shared = crate::cli::SharedArgs {
            no_color: true,
            ..crate::cli::SharedArgs::with_defaults()
        };
        println!("{TABLE_START}");
        crate::print_usage_table(
            "DeepSeek Harness Token Usage Report",
            crate::first_column(kind),
            &rows,
            &shared,
            false,
            None,
        )
        .unwrap();
        println!("{TABLE_END}");
    }

    fn snapshot_entries() -> Vec<LoadedEntry> {
        vec![
            loaded_entry(
                "session-a",
                "/workspace/api",
                "2026-10-26",
                100,
                50,
                25,
                5,
                0.42,
            ),
            loaded_entry(
                "session-a",
                "/workspace/api",
                "2026-10-26",
                30,
                20,
                0,
                0,
                0.12,
            ),
            loaded_entry(
                "session-b",
                "/workspace/cli",
                "2026-10-27",
                80,
                40,
                10,
                0,
                0.24,
            ),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn loaded_entry(
        session_id: &str,
        project_path: &str,
        date: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost: f64,
    ) -> LoadedEntry {
        let timestamp = parse_ts_timestamp(&format!("{date}T12:00:00.000Z")).unwrap();
        let usage = TokenUsageRaw {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_write,
            speed: None,
            cache_creation: None,
        };
        let model = "deepseek-v4.1-flash".to_string();
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format_rfc3339_millis(timestamp),
                version: Some("4".to_string()),
                message: UsageMessage {
                    usage,
                    model: Some(model.clone()),
                    id: Some(format!("{session_id}-{input}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: date.to_string(),
            project: Arc::from("dsh"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some(model),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            stored_cost: None,
        }
    }
}
