#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentReportKind;
    use std::sync::Arc;

    use crate::*;
    use crate::{TimestampMs, TokenUsageRaw};

    fn entry(session_id: &str, date: &str, millis: i64, project_path: &str) -> LoadedEntry {
        LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            message_id: Some(format!("{session_id}-{millis}")),
            version: None,
            recorded_cost: None,
            timestamp: TimestampMs::from_millis(millis),
            date: ccusage_core::cli::IsoDate::parse(date).unwrap(),
            project: Arc::from("antigravity"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost: 0.25,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("gemini-3-pro".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            stored_cost: None,
            estimated_pricing: false,
        }
    }

    #[test]
    fn session_json_includes_activity_bounds_and_project_metadata() {
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[
                entry(
                    "session-a",
                    "2026-05-04",
                    1_778_000_000_000,
                    "/workspace/app",
                ),
                entry(
                    "session-a",
                    "2026-05-05",
                    1_778_086_400_000,
                    "/workspace/app",
                ),
            ],
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

        assert_eq!(report["sessions"][0]["sessionId"], "session-a");
        assert_eq!(report["sessions"][0]["projectPath"], "/workspace/app");
        assert_eq!(
            report["sessions"][0]["firstActivity"],
            "2026-05-05T16:53:20.000Z"
        );
        assert_eq!(
            report["sessions"][0]["lastActivity"],
            "2026-05-06T16:53:20.000Z"
        );
    }
}
