#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentReportKind;
    use std::sync::Arc;

    use crate::TokenUsageRaw;
    use crate::*;
    use serde_json::json;

    #[test]
    fn report_includes_message_count_and_reasoning_total() {
        let entry = LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 1200,
                output_tokens: 300,
                cache_creation_input_tokens: 20,
                cache_read_input_tokens: 50,
                speed: None,
                cache_creation: None,
            },
            message_id: Some("hermes:session-1".to_string()),
            version: None,
            recorded_cost: Some(0.34),
            timestamp: ccusage_core::parse_ts_timestamp("2025-06-15T15:06:40.250Z").unwrap(),
            date: ccusage_core::cli::IsoDate::parse("2025-06-15").unwrap(),
            project: Arc::from("hermes"),
            session_id: Arc::from("session-1"),
            project_path: Arc::from("Hermes"),
            cost: 0.34,
            credits: None,
            extra_total_tokens: 10,
            message_count: Some(42),
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
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

        assert_eq!(report["daily"][0]["totalTokens"], json!(1580));
        assert_eq!(report["daily"][0]["messageCount"], json!(42));
        assert_eq!(report["totals"]["totalTokens"], json!(1580));
    }
}
