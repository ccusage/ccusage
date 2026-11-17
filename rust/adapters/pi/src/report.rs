#[cfg(test)]
mod tests {
    use crate::*;
    use serde_json::json;

    #[test]
    fn empty_report_has_zero_totals_object() {
        let report = crate::report_from_rows(&[], AgentReportKind::Daily);

        assert_eq!(report["daily"], json!([]));
        assert!(report["totals"].is_object());
        assert_eq!(report["totals"]["inputTokens"], json!(0));
        assert_eq!(report["totals"]["outputTokens"], json!(0));
        assert_eq!(report["totals"]["cacheCreationTokens"], json!(0));
        assert_eq!(report["totals"]["cacheReadTokens"], json!(0));
        assert_eq!(report["totals"]["totalTokens"], json!(0));
        assert_eq!(report["totals"]["totalCost"].as_f64(), Some(0.0));
    }
}
