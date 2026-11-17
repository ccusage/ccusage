use std::{collections::BTreeMap, sync::Arc};

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

#[derive(Debug, Clone)]
pub struct RowUsage {
    pub period: String,
    pub models_used: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub model_breakdowns: Vec<ModelBreakdown>,
}

#[derive(Debug, Clone)]
pub struct SourceRow {
    pub agent: &'static str,
    pub usage: RowUsage,
    pub metadata: Option<Value>,
}

impl std::ops::Deref for SourceRow {
    type Target = RowUsage;

    fn deref(&self) -> &Self::Target {
        &self.usage
    }
}

impl std::ops::DerefMut for SourceRow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.usage
    }
}

impl From<UnifiedRow> for SourceRow {
    fn from(row: UnifiedRow) -> Self {
        Self {
            agent: row.agent,
            usage: RowUsage {
                period: row.period,
                models_used: row.models_used,
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cache_creation_tokens: row.cache_creation_tokens,
                cache_read_tokens: row.cache_read_tokens,
                total_tokens: row.total_tokens,
                total_cost: row.total_cost,
                model_breakdowns: row.model_breakdowns,
            },
            metadata: row.metadata,
        }
    }
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
pub fn summarize_entry_sessions(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    session_groups(entries, crate::source::SessionScope::Project)
        .into_values()
        .map(|(_, group)| group.into_summary())
        .collect()
}

pub fn summarize_source_sessions_in_order(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    let mut groups = session_groups(entries, crate::source::SessionScope::Source)
        .into_values()
        .collect::<Vec<_>>();
    groups.sort_by_key(|(index, _)| *index);
    groups
        .into_iter()
        .map(|(_, group)| group.into_summary())
        .collect()
}

type SessionKey = (Option<Arc<str>>, Arc<str>);

fn session_key(entry: &LoadedEntry, scope: crate::source::SessionScope) -> SessionKey {
    let project = match scope {
        crate::source::SessionScope::Source => None,
        crate::source::SessionScope::Project => Some(Arc::clone(&entry.project_path)),
    };
    (project, Arc::clone(&entry.session_id))
}

fn session_groups(
    entries: &[LoadedEntry],
    scope: crate::source::SessionScope,
) -> BTreeMap<SessionKey, (usize, SessionAccumulator)> {
    let mut groups = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let (_, group) = groups
            .entry(session_key(entry, scope))
            .or_insert_with(|| (index, SessionAccumulator::default()));
        group.add_entry(entry);
    }
    groups
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
    kind: crate::source::SummaryKind,
    session: crate::source::SessionShape,
    scope: crate::source::SessionScope,
) -> Result<Vec<UsageSummary>> {
    use crate::source::{SessionShape, SummaryKind};
    use ccusage_core::summary::{Bucket, KeyTarget, summarize_bucket, summarize_by_key_target};

    match kind {
        SummaryKind::Session => match session {
            SessionShape::Plain => summarize_by_key_target(
                entries,
                |entry| session_key(entry, scope),
                |(_, session)| (session.to_string(), None),
                KeyTarget::Session,
            ),
            SessionShape::WithActivity => session_groups(entries, scope)
                .into_values()
                .map(|(_, group)| group.into_summary())
                .collect(),
        },
        SummaryKind::Daily | SummaryKind::Monthly | SummaryKind::Weekly(_) => {
            let daily = summarize_by_key_target(
                entries,
                |entry| entry.date.clone(),
                |date| (date.clone(), None),
                KeyTarget::Date,
            )?;
            Ok(match kind {
                SummaryKind::Monthly => summarize_bucket(&daily, Bucket::Monthly),
                SummaryKind::Weekly(start) => summarize_bucket(&daily, Bucket::Weekly(start)),
                _ => daily,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        source::{SessionScope, SessionShape, SummaryKind},
        test_source::entry,
    };
    use ccusage_core::cli::WeekDay;

    #[test]
    fn source_session_scope_treats_project_path_as_latest_metadata() {
        let mut entries = vec![
            entry("shared", "2026-01-01T00:00:00Z", 10),
            entry("shared", "2026-01-02T00:00:00Z", 20),
        ];
        entries[0].project_path = Arc::from("/old");
        entries[1].project_path = Arc::from("/new");
        let rows = summarize_entries(
            &entries,
            SummaryKind::Session,
            SessionShape::WithActivity,
            SessionScope::Source,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 30);
        assert_eq!(rows[0].project_path.as_deref(), Some("/new"));
        let rows = summarize_entries(
            &entries,
            SummaryKind::Session,
            SessionShape::WithActivity,
            SessionScope::Project,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].project_path.as_deref(), Some("/new"));
        assert_eq!(rows[1].project_path.as_deref(), Some("/old"));
    }

    #[test]
    fn source_sessions_can_preserve_first_seen_order() {
        let entries = [
            entry("z", "2026-01-01T00:00:00Z", 10),
            entry("a", "2026-01-02T00:00:00Z", 20),
            entry("z", "2026-01-03T00:00:00Z", 30),
        ];
        let rows = summarize_source_sessions_in_order(&entries).unwrap();
        assert_eq!(rows[0].session_id.as_deref(), Some("z"));
        assert_eq!(rows[0].input_tokens, 40);
        assert_eq!(rows[1].session_id.as_deref(), Some("a"));
    }

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
            SummaryKind::Monthly,
            SessionShape::Plain,
            SessionScope::Source,
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
                SummaryKind::Weekly(start),
                SessionShape::Plain,
                SessionScope::Source,
            )
            .unwrap();
            assert_eq!(rows[0].week.as_deref(), Some(expected));
        }
    }
}
