#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentReportKind;
    use std::sync::Arc;

    use crate::TokenUsageRaw;
    use crate::*;

    #[test]
    fn snapshots_daily_weekly_monthly_and_session_json_reports() {
        let entries = snapshot_entries();
        let daily = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Daily,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let weekly = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Weekly,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let monthly = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Monthly,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let sessions = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        assert_eq!(
            weekly
                .iter()
                .map(|row| row.week.as_deref())
                .collect::<Vec<_>>(),
            [Some("2026-10-26"), Some("2026-11-02")]
        );

        insta::assert_json_snapshot!(
            "cline_daily_json_report",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &daily,
                AgentReportKind::Daily
            )
        );
        insta::assert_json_snapshot!(
            "cline_weekly_json_report",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &weekly,
                AgentReportKind::Weekly
            )
        );
        insta::assert_json_snapshot!(
            "cline_monthly_json_report",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &monthly,
                AgentReportKind::Monthly
            )
        );
        insta::assert_json_snapshot!(
            "cline_session_json_report",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &sessions,
                AgentReportKind::Session
            )
        );
    }

    #[test]
    fn session_rows_carry_activity_bounds_and_project_path() {
        let usage = TokenUsageRaw {
            input_tokens: 10,
            output_tokens: 2,
            ..TokenUsageRaw::default()
        };
        let entries = [
            entry(
                "task:task-a",
                "2026-11-01T12:00:00Z",
                "claude-sonnet-4-20250514",
                usage,
                0.001,
                Some(0.001),
                None,
            ),
            entry(
                "task:task-a",
                "2026-10-31T09:30:00Z",
                "claude-sonnet-4-20250514",
                usage,
                0.001,
                Some(0.001),
                None,
            ),
        ];

        let sessions = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &sessions,
            AgentReportKind::Session,
        );
        let session = &report["sessions"][0];

        assert_eq!(session["sessionId"], "task:task-a");
        assert_eq!(session["firstActivity"], "2026-10-31T09:30:00.000Z");
        assert_eq!(session["lastActivity"], "2026-11-01T12:00:00.000Z");
        assert_eq!(session["projectPath"], "Cline");
    }

    fn snapshot_entries() -> Vec<LoadedEntry> {
        [
            entry(
                "task:task-a",
                "2026-10-31T12:00:00Z",
                "claude-sonnet-4-20250514",
                TokenUsageRaw {
                    input_tokens: 100,
                    output_tokens: 25,
                    cache_creation_input_tokens: 15,
                    cache_read_input_tokens: 10,
                    ..TokenUsageRaw::default()
                },
                0.002,
                Some(0.002),
                None,
            ),
            entry(
                "cli:task-a",
                "2026-11-01T12:00:00Z",
                "claude-opus-4-20250514",
                TokenUsageRaw {
                    input_tokens: 40,
                    output_tokens: 12,
                    cache_creation_input_tokens: 5,
                    cache_read_input_tokens: 5,
                    ..TokenUsageRaw::default()
                },
                0.001,
                Some(0.001),
                None,
            ),
            entry(
                "task:task-b",
                "2026-11-02T12:00:00Z",
                "vendor/custom-model",
                TokenUsageRaw {
                    input_tokens: 12,
                    output_tokens: 3,
                    ..TokenUsageRaw::default()
                },
                0.0,
                None,
                Some("vendor/custom-model"),
            ),
        ]
        .into()
    }

    fn entry(
        session_id: &str,
        timestamp: &str,
        model: &str,
        usage: TokenUsageRaw,
        cost: f64,
        stored_cost: Option<f64>,
        missing_pricing_model: Option<&str>,
    ) -> LoadedEntry {
        let timestamp = crate::parse_ts_timestamp(timestamp).expect("valid snapshot timestamp");

        let date = crate::date_at(timestamp, &jiff::tz::TimeZone::UTC);
        LoadedEntry {
            usage,
            message_id: Some(format!("request-{date}")),
            version: None,
            recorded_cost: stored_cost,
            timestamp,
            date,
            project: Arc::from("cline"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("Cline"),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some(model.to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: missing_pricing_model.map(str::to_string),
            stored_cost,
            estimated_pricing: false,
        }
    }
}
