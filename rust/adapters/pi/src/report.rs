use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator, cli::AgentReportKind, cli::WeekDay,
    summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| ccusage_core::agent_summary_json(row, kind, kind == AgentReportKind::Session))
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
            let daily = summarize_by_key(
                entries,
                |entry| entry.date.clone(),
                |date| (date.to_string(), None),
            )?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Session => {
            let mut groups = BTreeMap::<String, SessionAccumulator>::new();
            for entry in entries {
                groups
                    .entry(entry.session_id.to_string())
                    .or_default()
                    .add_entry(entry);
            }
            groups
                .into_values()
                .map(|group| group.into_summary())
                .collect()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_has_zero_totals_object() {
        let report = report_from_rows(&[], AgentReportKind::Daily);

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
