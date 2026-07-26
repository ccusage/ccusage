use std::collections::BTreeSet;

use serde_json::Value;

use crate::{ModelBreakdown, cli::AgentReportKind, fast::FxHashMap};

#[derive(Debug, Clone)]
pub(super) struct AllRow {
    pub(super) period: String,
    pub(super) agent: &'static str,
    pub(super) models_used: Vec<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) total_cost: f64,
    pub(super) metadata: Option<Value>,
    pub(super) metadata_agents: Option<Vec<&'static str>>,
    pub(super) agent_breakdowns: Option<Vec<AllRow>>,
    pub(super) model_breakdowns: Vec<ModelBreakdown>,
}

pub(super) struct AllLoadResult {
    pub(super) rows: Vec<AllRow>,
    pub(super) detected_agents: Vec<&'static str>,
}

pub(super) struct AllSectionsLoadResult {
    pub(super) sections: Vec<(AgentReportKind, Vec<AllRow>)>,
    pub(super) daily_detected_agents: Vec<&'static str>,
    pub(super) session_detected_agents: Vec<&'static str>,
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

pub(super) struct AgentRows {
    pub(super) rows: Vec<AllRow>,
    pub(super) detected: bool,
}

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
    agent_breakdowns: Vec<AllRow>,
    agent_indexes: FxHashMap<&'static str, usize>,
}

impl AllAccumulator {
    pub(super) fn add(&mut self, row: AllRow) {
        self.input_tokens += row.input_tokens;
        self.output_tokens += row.output_tokens;
        self.cache_creation_tokens += row.cache_creation_tokens;
        self.cache_read_tokens += row.cache_read_tokens;
        self.total_tokens += row.total_tokens;
        self.total_cost += row.total_cost;
        self.models.extend(row.models_used.iter().cloned());
        if let Some(agents) = row.metadata_agents.as_ref() {
            self.agents.extend(agents.iter().copied());
        } else if row.agent != "all" {
            self.agents.insert(row.agent);
        }
        match self.agent_indexes.get(row.agent).copied() {
            Some(index) => merge_agent_breakdown(&mut self.agent_breakdowns[index], row),
            None => {
                self.agent_indexes
                    .insert(row.agent, self.agent_breakdowns.len());
                self.agent_breakdowns.push(AllRow {
                    metadata_agents: Some(vec![row.agent]),
                    agent_breakdowns: None,
                    ..row
                });
            }
        }
    }

    pub(super) fn into_row(self, period: String) -> AllRow {
        let mut agent_breakdowns = self.agent_breakdowns;
        for breakdown in &mut agent_breakdowns {
            breakdown.period = period.clone();
        }
        agent_breakdowns.sort_by(|a, b| a.agent.cmp(b.agent));
        let mut model_breakdowns = aggregate_model_breakdowns(&agent_breakdowns);
        model_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        AllRow {
            period,
            agent: "all",
            models_used: self.models.into_iter().collect(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            total_tokens: self.total_tokens,
            total_cost: self.total_cost,
            metadata: None,
            metadata_agents: Some(self.agents.into_iter().collect()),
            agent_breakdowns: Some(agent_breakdowns),
            model_breakdowns,
        }
    }
}

fn merge_agent_breakdown(target: &mut AllRow, source: AllRow) {
    target.input_tokens += source.input_tokens;
    target.output_tokens += source.output_tokens;
    target.cache_creation_tokens += source.cache_creation_tokens;
    target.cache_read_tokens += source.cache_read_tokens;
    target.total_tokens += source.total_tokens;
    target.total_cost += source.total_cost;
    let mut models: BTreeSet<String> = target.models_used.drain(..).collect();
    models.extend(source.models_used);
    target.models_used = models.into_iter().collect();
    target.model_breakdowns =
        merge_model_breakdowns(target.model_breakdowns.drain(..), source.model_breakdowns);
}

fn merge_model_breakdowns(
    existing: impl IntoIterator<Item = ModelBreakdown>,
    additional: impl IntoIterator<Item = ModelBreakdown>,
) -> Vec<ModelBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<ModelBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.model_name.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(ModelBreakdown {
                model_name: item.model_name.clone(),
                ..ModelBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens += item.input_tokens;
        b.output_tokens += item.output_tokens;
        b.cache_creation_tokens += item.cache_creation_tokens;
        b.cache_read_tokens += item.cache_read_tokens;
        b.extra_total_tokens += item.extra_total_tokens;
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

pub(super) fn aggregate_model_breakdowns(rows: &[AllRow]) -> Vec<ModelBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<ModelBreakdown> = Vec::new();
    for row in rows {
        for item in &row.model_breakdowns {
            let index = *indexes.entry(item.model_name.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(ModelBreakdown {
                    model_name: item.model_name.clone(),
                    ..ModelBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens += item.input_tokens;
            b.output_tokens += item.output_tokens;
            b.cache_creation_tokens += item.cache_creation_tokens;
            b.cache_read_tokens += item.cache_read_tokens;
            b.extra_total_tokens += item.extra_total_tokens;
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}
