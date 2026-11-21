#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentReportKind;
    use std::sync::Arc;

    use crate::TokenUsageRaw;
    use crate::*;

    #[test]
    fn includes_goose_reasoning_remainder_in_report_total() {
        let entry = LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            message_id: Some("session-a".to_string()),
            version: None,
            recorded_cost: None,
            timestamp: crate::parse_ts_timestamp("2026-05-01T01:02:03.000Z").unwrap(),
            date: ccusage_core::cli::IsoDate::parse("2026-05-01").unwrap(),
            project: Arc::from("goose"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("Goose"),
            cost: 0.02,
            credits: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            extra_total_tokens: 30,
            message_count: None,
            stored_cost: None,
            estimated_pricing: false,
        };
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[entry],
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

        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
    }
}
