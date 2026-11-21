#[cfg(test)]
mod tests {
    use crate::*;
    use ccusage_core::cli::AgentReportKind;
    use ccusage_core::{TimestampMs, TokenUsageRaw, parse_ts_timestamp};
    use std::sync::Arc;

    fn entry(session_id: &str, date: &str, millis: i64) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(millis);
        LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 60,
                output_tokens: 10,
                cache_creation_input_tokens: 15,
                cache_read_input_tokens: 25,
                speed: None,
                cache_creation: None,
            },
            message_id: Some(format!("usage-{millis}")),
            version: Some("0.16.3".to_string()),
            recorded_cost: None,
            timestamp,
            date: ccusage_core::cli::IsoDate::parse(date).unwrap(),
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
            stored_cost: None,
            estimated_pricing: false,
        }
    }

    #[test]
    fn reports_daily_and_session_totals() {
        let entries = [entry("session-1", "2026-08-16", 1_786_909_042_666)];
        let daily = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Daily,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &daily,
            AgentReportKind::Daily,
        );
        assert_eq!(report["daily"][0]["totalTokens"], 110);
        assert_eq!(report["totals"]["totalTokens"], 110);

        let sessions = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        assert_eq!(sessions[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(sessions[0].project_path.as_deref(), Some("/project"));
    }

    #[test]
    fn snapshots_focused_zcode_json_reports_for_daily_monthly_and_session() {
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
        let monthly = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Monthly,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let session = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();

        insta::assert_json_snapshot!(
            "focused_zcode_daily_json",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &daily,
                AgentReportKind::Daily
            )
        );
        insta::assert_json_snapshot!(
            "focused_zcode_monthly_json",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &monthly,
                AgentReportKind::Monthly
            )
        );
        insta::assert_json_snapshot!(
            "focused_zcode_session_json",
            ccusage_adapter_common::source::EntrySource::report_json(
                &crate::SOURCE,
                &session,
                AgentReportKind::Session
            )
        );
    }

    #[test]
    fn saturates_focused_daily_and_monthly_reports_for_extreme_counters() {
        let entries = [
            extreme_entry("2099-01-02", 4_070_908_800_000, u64::MAX),
            extreme_entry("2099-01-03", 4_071_081_600_000, 1),
        ];
        let daily = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Daily,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let daily_report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &daily,
            AgentReportKind::Daily,
        );
        let monthly = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &entries,
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Monthly,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let monthly_report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &monthly,
            AgentReportKind::Monthly,
        );

        for report in [daily_report, monthly_report] {
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
        assert_eq!(monthly[0].model_breakdowns[0].input_tokens, u64::MAX);
        assert_eq!(monthly[0].model_breakdowns[0].output_tokens, u64::MAX);
    }

    fn extreme_entry(date: &str, millis: i64, tokens: u64) -> LoadedEntry {
        let mut entry = entry("session-extreme", date, millis);
        entry.usage = TokenUsageRaw {
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
                let date = ccusage_core::cli::IsoDate::parse(timestamp.get(..10).unwrap()).unwrap();
                let timestamp = parse_ts_timestamp(timestamp).unwrap();
                LoadedEntry {
                    usage: TokenUsageRaw {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens: cache_creation_tokens,
                        cache_read_input_tokens: cache_read_tokens,
                        speed: None,
                        cache_creation: None,
                    },
                    message_id: Some(id.to_string()),
                    version: Some("0.16.3".to_string()),
                    recorded_cost: None,
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
                    stored_cost: None,
                    estimated_pricing: false,
                }
            },
        )
        .collect()
    }
}
