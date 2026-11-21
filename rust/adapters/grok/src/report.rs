#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentReportKind;
    use std::sync::Arc;

    use crate::*;
    use crate::{TimestampMs, TokenUsageRaw};

    fn entry(
        session_id: &str,
        date: &str,
        millis: i64,
        input: u64,
        cache_read: u64,
        project_path: &str,
    ) -> LoadedEntry {
        let timestamp = TimestampMs::from_millis(millis);
        LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: input,
                output_tokens: 20,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: cache_read,
                speed: None,
                cache_creation: None,
            },
            message_id: Some(format!("evt-{millis}")),
            version: None,
            recorded_cost: None,
            timestamp,
            date: ccusage_core::cli::IsoDate::parse(date).unwrap(),
            project: Arc::from("grok"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost: 0.0113,
            credits: None,
            model: Some("grok-4.5-build".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 0,
            message_count: None,
            stored_cost: None,
            estimated_pricing: false,
        }
    }

    #[test]
    fn reports_cache_reads_separately_from_input_tokens() {
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[entry(
                "sess-a",
                "2026-07-29",
                1_785_328_986_355,
                60,
                40,
                "/tmp/proj",
            )],
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Daily,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &rows,
            AgentReportKind::Daily,
        );

        assert_eq!(report["daily"][0]["inputTokens"], 60);
        assert_eq!(report["daily"][0]["outputTokens"], 20);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 0);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 40);
        assert_eq!(report["daily"][0]["totalTokens"], 120);
    }

    #[test]
    fn groups_conversations_under_the_session_report() {
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[
                entry(
                    "sess-a",
                    "2026-07-28",
                    1_785_242_586_000,
                    60,
                    0,
                    "D:\\work\\proj",
                ),
                entry(
                    "sess-a",
                    "2026-07-29",
                    1_785_328_986_355,
                    40,
                    20,
                    "D:\\work\\proj",
                ),
            ],
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-a"));
        assert_eq!(rows[0].project_path.as_deref(), Some("D:\\work\\proj"));
        assert_eq!(rows[0].input_tokens, 100);
        assert_eq!(rows[0].cache_read_tokens, 20);
        assert_eq!(rows[0].extra_total_tokens, 0);
        assert_eq!(
            rows[0].first_activity.as_deref(),
            Some("2026-07-28T12:43:06.000Z")
        );
        assert_eq!(
            rows[0].last_activity.as_deref(),
            Some("2026-07-29T12:43:06.355Z")
        );
    }

    #[test]
    fn session_json_includes_activity_and_project_path() {
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[entry(
                "sess-a",
                "2026-07-29",
                1_785_328_986_355,
                60,
                40,
                "/workspace/api",
            )],
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Session,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &rows,
            AgentReportKind::Session,
        );

        assert_eq!(report["sessions"][0]["sessionId"], "sess-a");
        assert_eq!(report["sessions"][0]["projectPath"], "/workspace/api");
        assert_eq!(
            report["sessions"][0]["lastActivity"],
            "2026-07-29T12:43:06.355Z"
        );
        assert_eq!(
            report["sessions"][0]["firstActivity"],
            "2026-07-29T12:43:06.355Z"
        );
    }
}
