use std::collections::BTreeMap;

use serde_json::{Value, json};

use ccusage_core::{
    LoadedEntry, ModelBreakdown, Result, SessionAccumulator, UsageSummary, json_float,
};

/// One row a source contributes to the unified report: a period, the agent it
/// belongs to, and that agent's usage in the period.
#[derive(Debug, Clone)]
pub struct UnifiedRow {
    pub period: String,
    pub agent: &'static str,
    pub models_used: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub metadata: Option<Value>,
    pub metadata_agents: Option<Vec<&'static str>>,
    pub agent_breakdowns: Option<Vec<UnifiedRow>>,
    pub model_breakdowns: Vec<ModelBreakdown>,
}

/// Converts a source's summaries into its unified rows, dropping the ones with
/// neither tokens nor cost.
pub fn summary_rows(
    agent: &'static str,
    summaries: Vec<UsageSummary>,
    include_project_path: bool,
) -> Vec<UnifiedRow> {
    summaries
        .into_iter()
        .filter_map(|summary| {
            let period = summary
                .date
                .as_ref()
                .or(summary.week.as_ref())
                .or(summary.month.as_ref())
                .or(summary.session_id.as_ref())?
                .clone();
            let total_tokens = summary.total_tokens();
            // Cost-only rows, such as Copilot credits missing from modelMetrics, still bill.
            if total_tokens == 0 && summary.total_cost == 0.0 {
                return None;
            }
            let metadata = summary_metadata(&summary, include_project_path);
            Some(UnifiedRow {
                period,
                agent,
                models_used: summary.models_used,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_creation_tokens: summary.cache_creation_tokens,
                cache_read_tokens: summary.cache_read_tokens,
                total_tokens,
                total_cost: summary.total_cost,
                metadata,
                metadata_agents: Some(vec![agent]),
                agent_breakdowns: None,
                model_breakdowns: summary.model_breakdowns,
            })
        })
        .collect()
}

/// Summarizes entries into one row per session of a project, ordered by
/// project path and then session id.
///
/// The unified session report groups this way for the sources whose focused
/// session report keys by session id alone, so two projects that reuse a
/// session id stay two rows.
pub fn summarize_entry_sessions(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    let mut groups = BTreeMap::<(String, String), SessionAccumulator>::new();
    for entry in entries {
        groups
            .entry((entry.project_path.to_string(), entry.session_id.to_string()))
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_values()
        .map(|group| group.into_summary())
        .collect()
}

fn summary_metadata(summary: &UsageSummary, include_project_path: bool) -> Option<Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(credits) = summary.credits {
        metadata.insert("credits".to_string(), json_float(credits));
    }
    if summary.session_id.is_some() {
        if let Some(last_activity) = summary.last_activity.as_ref() {
            metadata.insert("lastActivity".to_string(), json!(last_activity));
        }
        if include_project_path && let Some(project_path) = summary.project_path.as_ref() {
            metadata.insert("projectPath".to_string(), json!(project_path));
        }
    }
    if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    }
}

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: ccusage_core::cli::AgentReportKind,
    start: ccusage_core::cli::WeekDay,
    session: crate::source::SessionShape,
) -> Result<Vec<UsageSummary>> {
    use crate::source::SessionShape;
    use ccusage_core::{
        cli::AgentReportKind,
        summary::{Bucket, KeyTarget, summarize_bucket, summarize_by_key_target},
    };

    match kind {
        AgentReportKind::Session => match session {
            SessionShape::Plain => summarize_by_key_target(
                entries,
                |entry| (entry.project_path.clone(), entry.session_id.clone()),
                |(_, session)| (session.to_string(), None),
                KeyTarget::Session,
            ),
            SessionShape::WithActivity => summarize_entry_sessions(entries),
        },
        AgentReportKind::Daily | AgentReportKind::Monthly | AgentReportKind::Weekly => {
            let daily = summarize_by_key_target(
                entries,
                |entry| entry.date.clone(),
                |date| (date.clone(), None),
                KeyTarget::Date,
            )?;
            Ok(match kind {
                AgentReportKind::Monthly => summarize_bucket(&daily, Bucket::Monthly),
                AgentReportKind::Weekly => summarize_bucket(&daily, Bucket::Weekly(start)),
                _ => daily,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{source::SessionShape, test_source::entry};
    use ccusage_core::cli::{AgentReportKind, WeekDay};

    #[test]
    fn buckets_daily_rows_before_summing_costs() {
        let mut entries = vec![
            entry("a", "2026-01-02T00:00:00Z", 1),
            entry("a", "2026-01-01T00:00:00Z", 2),
            entry("a", "2026-01-02T01:00:00Z", 3),
        ];
        entries[0].cost = 1e16;
        entries[1].cost = 1.0;
        entries[2].cost = -1e16;
        let rows = summarize_entries(
            &entries,
            AgentReportKind::Monthly,
            WeekDay::Monday,
            SessionShape::Plain,
        )
        .unwrap();
        assert_eq!(rows[0].total_cost, 1.0);
        assert_eq!(rows[0].month.as_deref(), Some("2026-01"));
    }

    #[test]
    fn weekly_bucket_uses_the_requested_start() {
        let entries = [entry("a", "2026-01-04T12:00:00Z", 1)];
        for (start, expected) in [
            (WeekDay::Monday, "2025-12-29"),
            (WeekDay::Sunday, "2026-01-04"),
        ] {
            let rows = summarize_entries(
                &entries,
                AgentReportKind::Weekly,
                start,
                SessionShape::Plain,
            )
            .unwrap();
            assert_eq!(rows[0].week.as_deref(), Some(expected));
        }
    }
}
