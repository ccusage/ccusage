#[cfg(test)]
mod tests {
    use crate::*;
    use crate::{cli::SharedArgs, paths::DSH_HOME_ENV};
    use ccusage_core::PricingMap;
    use ccusage_core::cli::{AgentReportKind, WeekDay};
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn snapshots_focused_reports_from_fixture_sessions() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/--home-user-project--/dsh-parent/session.v4.jsonl":
                include_str!("../tests/fixtures/parent/session.v4.jsonl"),
            "sessions/--home-user-project--/dsh-fork/session.v4.jsonl":
                include_str!("../tests/fixtures/fork/session.v4.jsonl"),
        });
        Environment::with_var(DSH_HOME_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..SharedArgs::default()
            };
            let (entries, _) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            for kind in [
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ] {
                let rows = SOURCE
                    .summarize(
                        &entries,
                        ccusage_adapter_common::source::SummaryKind::for_report(
                            kind,
                            WeekDay::Monday,
                        ),
                    )
                    .unwrap();
                insta::assert_json_snapshot!(
                    format!("focused_dsh_{kind:?}_json").to_lowercase(),
                    ccusage_adapter_common::source::EntrySource::report_json(
                        &crate::SOURCE,
                        &rows,
                        kind
                    )
                );
            }
        })
    }

    #[test]
    fn weeks_start_on_monday_like_the_last_window() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/p/s/session.v4.jsonl": concat!(
                r#"{"type":"session","version":4,"id":"s","createdAt":1794744000000}"#, "\n",
                r#"{"type":"request/context","seq":1,"time":1794744000000,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#, "\n",
                r#"{"type":"assistant/message","seq":2,"time":1794744000000,"data":{"usage":{"inputTokens":1,"outputTokens":1}}}"#, "\n",
                r#"{"type":"assistant/message","seq":3,"time":1794830400000,"data":{"usage":{"inputTokens":1,"outputTokens":1}}}"#, "\n",
            ),
        });
        Environment::with_var(DSH_HOME_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..SharedArgs::default()
            };
            let (entries, _) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            let rows = SOURCE
                .summarize(
                    &entries,
                    ccusage_adapter_common::source::SummaryKind::Weekly(WeekDay::Monday),
                )
                .unwrap();
            let weeks = rows
                .iter()
                .map(|row| row.week.as_deref())
                .collect::<Vec<_>>();
            assert_eq!(weeks, [Some("2026-11-09"), Some("2026-11-16")]);
        })
    }
}
