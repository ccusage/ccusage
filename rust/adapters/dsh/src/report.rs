use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result,
    cli::{AgentReportKind, WeekDay},
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
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
    use super::*;
    use crate::{PricingMap, cli::SharedArgs, paths::DSH_HOME_ENV};
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn snapshots_focused_reports_from_fixture_sessions() {
        let fixture = fs_fixture!({
            "sessions/--home-user-project--/dsh-parent/session.v4.jsonl":
                include_str!("../tests/fixtures/parent/session.v4.jsonl"),
            "sessions/--home-user-project--/dsh-fork/session.v4.jsonl":
                include_str!("../tests/fixtures/fork/session.v4.jsonl"),
        });
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            offline: true,
            ..SharedArgs::default()
        };
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let rows = summarize_entries(&entries, kind).unwrap();
            insta::assert_json_snapshot!(
                format!("focused_dsh_{kind:?}_json").to_lowercase(),
                report_from_rows(&rows, kind)
            );
        }
    }

    #[test]
    fn weeks_start_on_monday_like_the_last_window() {
        let fixture = fs_fixture!({
            "sessions/p/s/session.v4.jsonl": concat!(
                r#"{"type":"session","version":4,"id":"s","createdAt":1794744000000}"#, "\n",
                r#"{"type":"request/context","seq":1,"time":1794744000000,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#, "\n",
                r#"{"type":"assistant/message","seq":2,"time":1794744000000,"data":{"usage":{"inputTokens":1,"outputTokens":1}}}"#, "\n",
                r#"{"type":"assistant/message","seq":3,"time":1794830400000,"data":{"usage":{"inputTokens":1,"outputTokens":1}}}"#, "\n",
            ),
        });
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            offline: true,
            ..SharedArgs::default()
        };
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        let rows = summarize_entries(&entries, AgentReportKind::Weekly).unwrap();
        let weeks = rows
            .iter()
            .map(|row| row.week.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(weeks, [Some("2026-11-09"), Some("2026-11-16")]);
    }
}
