use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub(crate) fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
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

pub(crate) fn print_table(
    kind: AgentReportKind,
    rows: &[crate::UsageSummary],
    shared: &crate::cli::SharedArgs,
) -> Result<()> {
    crate::print_usage_table(
        "Muse Code Token Usage Report",
        ccusage_core::first_column(kind),
        rows,
        shared,
        false,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PricingMap, cli::CostMode, cli::SharedArgs};
    use ccusage_test_support::{EnvVarGuard, fs_fixture};
    use std::process::Command;

    #[test]
    fn fixture_reports_snapshot_daily_monthly_and_session_json_and_rendered_tables() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/observed-session/session.jsonl": include_str!(
                "../fixtures/issue-1694-model-completed.jsonl"
            ),
            "muse/sessions/2026/09/08/synthetic-session/session.jsonl": include_str!(
                "../fixtures/report-synthetic-completions.jsonl"
            ),
        });
        let data_root = fixture.path("muse");
        let _env = EnvVarGuard::set("MUSE_DATA_DIR", &data_root);
        let shared = report_test_args();
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();
        let kinds = [
            ("daily", AgentReportKind::Daily),
            ("monthly", AgentReportKind::Monthly),
            ("session", AgentReportKind::Session),
        ];
        let mut reports = serde_json::Map::new();

        for (name, kind) in kinds {
            let rows = summarize_entries(&entries, kind).unwrap();
            let report = report_from_rows(&rows, kind);
            if name == "daily" {
                assert_eq!(report["totals"]["inputTokens"], 720);
                assert_eq!(report["totals"]["outputTokens"], 38);
                assert_eq!(report["totals"]["cacheReadTokens"], 27_151);
            }
            if name == "monthly" {
                assert_eq!(report["monthly"].as_array().unwrap().len(), 2);
                assert_eq!(report["monthly"][0]["month"], "2026-09");
                assert_eq!(report["monthly"][1]["month"], "2026-10");
            }
            if name == "session" {
                assert_eq!(report["sessions"].as_array().unwrap().len(), 2);
            }
            reports.insert(name.to_owned(), report);
        }

        insta::assert_json_snapshot!(serde_json::Value::Object(reports));

        for (name, _) in kinds {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "report::tests::render_fixture_table_child",
                    "--nocapture",
                ])
                .env("MUSE_REPORT_TABLE_KIND", name)
                .env("MUSE_DATA_DIR", &data_root)
                .output()
                .expect("failed to run the table rendering child test");
            assert!(
                output.status.success(),
                "table child test failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).expect("table output was not UTF-8");
            let table = stdout
                .split_once("@@MUSE_TABLE_START@@\n")
                .expect("table start marker was not printed")
                .1
                .split_once("@@MUSE_TABLE_END@@")
                .expect("table end marker was not printed")
                .0
                .trim_end();
            insta::assert_snapshot!(format!("muse_fixture_{name}_table"), table);
        }
    }

    #[test]
    fn render_fixture_table_child() {
        let Some(name) = std::env::var_os("MUSE_REPORT_TABLE_KIND") else {
            return;
        };
        let kind = match name.to_str() {
            Some("daily") => AgentReportKind::Daily,
            Some("monthly") => AgentReportKind::Monthly,
            Some("session") => AgentReportKind::Session,
            other => panic!("unexpected table report kind: {other:?}"),
        };
        let shared = report_test_args();
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();
        let rows = summarize_entries(&entries, kind).unwrap();

        println!("@@MUSE_TABLE_START@@");
        print_table(kind, &rows, &shared).unwrap();
        println!("@@MUSE_TABLE_END@@");
    }

    fn report_test_args() -> SharedArgs {
        SharedArgs {
            json: true,
            mode: CostMode::Display,
            no_color: true,
            single_thread: true,
            timezone: Some("UTC".to_owned()),
            ..SharedArgs::default()
        }
    }
}
