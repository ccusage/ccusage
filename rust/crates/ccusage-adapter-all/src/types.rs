use std::collections::BTreeSet;

use ccusage_adapter_common::rows::{RowUsage, SessionMetadata, SourceRow};

use crate::{
    Diagnostics, ModelBreakdown,
    cli::{AgentReportKind, WeekDay},
    fast::FxHashMap,
};

#[derive(Debug, Clone)]
pub(super) struct PeriodRow {
    pub(super) usage: RowUsage,
    pub(super) agents: Vec<&'static str>,
    pub(super) agent_breakdowns: Vec<SourceRow>,
}

#[derive(Debug, Clone)]
pub(super) enum ReportRow {
    Source(SourceRow),
    Period(PeriodRow),
}

impl std::ops::Deref for ReportRow {
    type Target = RowUsage;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Source(row) => &row.usage,
            Self::Period(row) => &row.usage,
        }
    }
}

impl std::ops::DerefMut for ReportRow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Source(row) => &mut row.usage,
            Self::Period(row) => &mut row.usage,
        }
    }
}

impl ReportRow {
    pub(super) fn agent(&self) -> &'static str {
        match self {
            Self::Source(row) => row.agent,
            Self::Period(_) => "all",
        }
    }

    pub(super) fn agents(&self) -> Option<&[&'static str]> {
        match self {
            Self::Source(_) => None,
            Self::Period(row) => Some(&row.agents),
        }
    }

    pub(super) fn metadata(&self) -> Option<&SessionMetadata> {
        match self {
            Self::Source(row) => row.metadata.as_ref(),
            Self::Period(_) => None,
        }
    }

    pub(super) fn agent_breakdowns(&self) -> Option<&[SourceRow]> {
        match self {
            Self::Source(_) => None,
            Self::Period(row) => Some(&row.agent_breakdowns),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Bucket {
    Day,
    Month,
    Week(WeekDay),
}

pub(super) use ccusage_core::cli::IsoDate as ReportDay;

#[derive(Clone)]
pub(super) struct DayRow {
    pub(super) day: ReportDay,
    pub(super) row: SourceRow,
}

impl Bucket {
    pub(super) fn for_report(kind: AgentReportKind, start: WeekDay) -> Option<Self> {
        match kind {
            AgentReportKind::Daily => Some(Self::Day),
            AgentReportKind::Monthly => Some(Self::Month),
            AgentReportKind::Weekly => Some(Self::Week(start)),
            AgentReportKind::Session => None,
        }
    }

    pub(super) fn key(self, date: ReportDay) -> String {
        match self {
            Self::Day => date.to_string(),
            Self::Month => date.format_month(),
            Self::Week(start) => date.week_start(start).to_string(),
        }
    }
}

pub(super) struct AllLoadResult {
    pub(super) rows: Vec<ReportRow>,
    pub(super) detected_agents: Vec<&'static str>,
    pub(super) diagnostics: Vec<Diagnostics>,
}

pub(super) struct AllSectionsLoadResult {
    pub(super) sections: Vec<(AgentReportKind, Vec<ReportRow>)>,
    pub(super) daily_detected_agents: Vec<&'static str>,
    pub(super) session_detected_agents: Vec<&'static str>,
    pub(super) diagnostics: Vec<Diagnostics>,
}

impl AllSectionsLoadResult {
    pub(super) fn detected_agents_for(&self, kind: AgentReportKind) -> &[&'static str] {
        match kind {
            AgentReportKind::Session => &self.session_detected_agents,
            AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
                &self.daily_detected_agents
            }
        }
    }
}

pub(super) type AgentRows = ccusage_adapter_common::source::LoadedRows;

pub(super) struct AgentLoadSpec<'scope> {
    pub(super) index: usize,
    pub(super) agent: &'static str,
    pub(super) progress_agent: crate::progress::UsageLoadAgent,
    pub(super) load: Box<dyn FnOnce() -> crate::Result<AgentRows> + Send + 'scope>,
}

pub(super) struct LoadedAgentRows {
    pub(super) index: usize,
    pub(super) agent: &'static str,
    pub(super) agent_rows: AgentRows,
}

#[derive(Default)]
pub(super) struct AllAccumulator {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
    models: BTreeSet<String>,
    agents: BTreeSet<&'static str>,
    agent_breakdowns: Vec<SourceRow>,
    agent_indexes: FxHashMap<&'static str, usize>,
}

impl AllAccumulator {
    pub(super) fn add(&mut self, row: SourceRow) {
        self.input_tokens = self.input_tokens.saturating_add(row.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(row.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(row.cache_creation_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(row.cache_read_tokens);
        self.total_tokens = self.total_tokens.saturating_add(row.total_tokens);
        self.total_cost += row.total_cost;
        self.models.extend(row.models_used.iter().cloned());
        self.agents.insert(row.agent);
        match self.agent_indexes.get(row.agent).copied() {
            Some(index) => merge_agent_breakdown(&mut self.agent_breakdowns[index], row),
            None => {
                self.agent_indexes
                    .insert(row.agent, self.agent_breakdowns.len());
                self.agent_breakdowns.push(row);
            }
        }
    }

    pub(super) fn into_row(self, period: String) -> ReportRow {
        let mut agent_breakdowns = self.agent_breakdowns;
        for breakdown in &mut agent_breakdowns {
            breakdown.period = period.clone();
        }
        ccusage_core::sort::stable_by(&mut agent_breakdowns, |a, b| a.agent.cmp(b.agent));
        let model_breakdowns = merge_by_model(
            agent_breakdowns
                .iter()
                .flat_map(|breakdown| &breakdown.model_breakdowns),
        );
        ReportRow::Period(PeriodRow {
            usage: RowUsage {
                period,
                models_used: self.models.into_iter().collect(),
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cache_creation_tokens: self.cache_creation_tokens,
                cache_read_tokens: self.cache_read_tokens,
                total_tokens: self.total_tokens,
                total_cost: self.total_cost,
                model_breakdowns,
            },
            agents: self.agents.into_iter().collect(),
            agent_breakdowns,
        })
    }
}

fn merge_agent_breakdown(target: &mut SourceRow, source: SourceRow) {
    target.input_tokens = target.input_tokens.saturating_add(source.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(source.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(source.cache_creation_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(source.cache_read_tokens);
    target.total_tokens = target.total_tokens.saturating_add(source.total_tokens);
    target.total_cost += source.total_cost;
    let mut models: BTreeSet<String> = target.models_used.drain(..).collect();
    models.extend(source.usage.models_used);
    target.models_used = models.into_iter().collect();
    target.model_breakdowns = merge_by_model(
        target
            .model_breakdowns
            .iter()
            .chain(&source.usage.model_breakdowns),
    );
}

fn merge_by_model<'a>(items: impl IntoIterator<Item = &'a ModelBreakdown>) -> Vec<ModelBreakdown> {
    let mut indexes = FxHashMap::<&str, usize>::default();
    let mut breakdowns: Vec<ModelBreakdown> = Vec::new();
    for item in items {
        let index = *indexes.entry(&item.model_name).or_insert_with(|| {
            breakdowns.push(ModelBreakdown {
                model_name: item.model_name.clone(),
                ..ModelBreakdown::default()
            });
            breakdowns.len() - 1
        });
        breakdowns[index].merge(item);
    }
    ModelBreakdown::sort_by_cost_desc(&mut breakdowns);
    breakdowns
}
