use std::{collections::BTreeMap, sync::Arc};

use serde::{Serialize, Serializer};

use ccusage_core::{LoadedEntry, ModelBreakdown, Result, SessionAccumulator, UsageSummary};

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
    pub metadata: Option<SessionMetadata>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_credits"
    )]
    pub credits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
}

fn serialize_credits<S: Serializer>(
    credits: &Option<f64>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    credits.map(ccusage_core::json_float).serialize(serializer)
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

pub fn summary_rows(
    agent: &'static str,
    summaries: Vec<UsageSummary>,
    include_project_path: bool,
) -> Vec<SourceRow> {
    summaries
        .into_iter()
        .filter(summary_has_usage)
        .filter_map(|summary| {
            let period = summary
                .date
                .as_ref()
                .or(summary.week.as_ref())
                .or(summary.month.as_ref())
                .or(summary.session_id.as_ref())?
                .clone();
            let total_tokens = summary.total_tokens();
            let metadata = summary_metadata(&summary, include_project_path);
            Some(SourceRow {
                agent,
                metadata,
                usage: RowUsage {
                    period,
                    models_used: summary.models_used,
                    input_tokens: summary.input_tokens,
                    output_tokens: summary.output_tokens,
                    cache_creation_tokens: summary.cache_creation_tokens,
                    cache_read_tokens: summary.cache_read_tokens,
                    total_tokens,
                    total_cost: summary.total_cost,
                    model_breakdowns: summary.model_breakdowns,
                },
            })
        })
        .collect()
}

pub fn summarize_entry_sessions(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    session_groups(entries, crate::source::SessionScope::Project)
        .into_values()
        .map(|(_, group)| group.into_summary())
        .collect::<Result<Vec<_>>>()
        .map(nonempty_summaries)
}

pub fn summarize_source_sessions_in_order(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    let mut groups = session_groups(entries, crate::source::SessionScope::Source)
        .into_values()
        .collect::<Vec<_>>();
    ccusage_core::sort::stable_by_key(&mut groups, |(index, _)| *index);
    groups
        .into_iter()
        .map(|(_, group)| group.into_summary())
        .collect::<Result<Vec<_>>>()
        .map(nonempty_summaries)
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

fn summary_metadata(summary: &UsageSummary, include_project_path: bool) -> Option<SessionMetadata> {
    summary.session_id.as_ref()?;
    let metadata = SessionMetadata {
        credits: summary.credits,
        last_activity: summary.last_activity.clone().map(Some),
        project_path: summary
            .project_path
            .clone()
            .filter(|_| include_project_path),
        reasoning_output_tokens: None,
    };
    (metadata.credits.is_some()
        || metadata.last_activity.is_some()
        || metadata.project_path.is_some())
    .then_some(metadata)
}

pub fn summarize_entries(
    entries: &[LoadedEntry],
    kind: crate::source::SummaryKind,
    session: crate::source::SessionShape,
    scope: crate::source::SessionScope,
) -> Result<Vec<UsageSummary>> {
    use crate::source::{SessionShape, SummaryKind};
    use ccusage_core::summary::{
        Bucket, KeyTarget, summarize_bucket, summarize_by_day, summarize_by_key_target,
    };

    let summaries = match kind {
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
            let daily = summarize_by_day(entries)?;
            Ok(match kind {
                SummaryKind::Monthly => summarize_bucket(&daily, Bucket::Monthly),
                SummaryKind::Weekly(start) => summarize_bucket(&daily, Bucket::Weekly(start)),
                SummaryKind::Daily | SummaryKind::Session => daily,
            })
        }
    };
    summaries.map(nonempty_summaries)
}

fn summary_has_usage(summary: &UsageSummary) -> bool {
    summary.total_tokens() != 0 || summary.total_cost != 0.0
}

fn nonempty_summaries(mut rows: Vec<UsageSummary>) -> Vec<UsageSummary> {
    rows.retain(summary_has_usage);
    rows
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
    fn period_rows_do_not_construct_session_metadata() {
        let mut summaries = summarize_entries(
            &[entry("s", "2026-01-01T00:00:00Z", 10)],
            SummaryKind::Daily,
            SessionShape::WithActivity,
            SessionScope::Source,
        )
        .unwrap();
        summaries[0].credits = Some(3.0);
        let rows = summary_rows("test", summaries, true);
        assert!(rows[0].metadata.is_none());
    }

    #[test]
    fn tokenless_zero_cost_rows_are_omitted_in_every_report_shape() {
        let entries = [entry("s", "2026-01-01T00:00:00Z", 0)];
        for kind in [
            SummaryKind::Daily,
            SummaryKind::Monthly,
            SummaryKind::Weekly(WeekDay::Monday),
            SummaryKind::Session,
        ] {
            assert!(
                summarize_entries(
                    &entries,
                    kind,
                    SessionShape::WithActivity,
                    SessionScope::Source
                )
                .unwrap()
                .is_empty()
            );
        }
        assert!(
            summarize_source_sessions_in_order(&entries)
                .unwrap()
                .is_empty()
        );
        let mut billed = entries[0].clone();
        billed.cost = 3.0;
        assert_eq!(
            summarize_source_sessions_in_order(&[billed]).unwrap()[0].total_cost,
            3.0
        );
    }

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
    fn daily_rows_keep_label_order_for_extended_years() {
        let entries = [(10000, 1, 1), (2026, 1, 1), (-2, 1, 1), (-1, 12, 31)].map(|(y, m, d)| {
            let mut entry = entry("s", "2026-01-01T00:00:00Z", 1);
            entry.date = ccusage_core::cli::IsoDate::from_ymd(y, m, d).unwrap();
            entry
        });
        let rows = summarize_entries(
            &entries,
            SummaryKind::Daily,
            SessionShape::Plain,
            SessionScope::Source,
        )
        .unwrap();
        let dates = rows
            .iter()
            .map(|row| row.date.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            dates,
            ["-001-12-31", "-002-01-01", "10000-01-01", "2026-01-01"]
        );
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
