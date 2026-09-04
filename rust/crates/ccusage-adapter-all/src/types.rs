use std::collections::BTreeSet;

use serde_json::Value;

use crate::{
    ModelBreakdown, PluginBreakdown, SkillBreakdown, SourceTypeBreakdown, cli::AgentReportKind,
    fast::FxHashMap,
};

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
    pub(super) plugin_breakdowns: Vec<PluginBreakdown>,
    pub(super) skill_breakdowns: Vec<SkillBreakdown>,
    pub(super) source_type_breakdowns: Vec<SourceTypeBreakdown>,
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
        self.input_tokens = self.input_tokens.saturating_add(row.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(row.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(row.cache_creation_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(row.cache_read_tokens);
        self.total_tokens = self.total_tokens.saturating_add(row.total_tokens);
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
        let mut plugin_breakdowns = aggregate_plugin_breakdowns(&agent_breakdowns);
        plugin_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        let mut skill_breakdowns = aggregate_skill_breakdowns(&agent_breakdowns);
        skill_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        let mut source_type_breakdowns = aggregate_source_type_breakdowns(&agent_breakdowns);
        source_type_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
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
            plugin_breakdowns,
            skill_breakdowns,
            source_type_breakdowns,
        }
    }
}

fn merge_agent_breakdown(target: &mut AllRow, source: AllRow) {
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
    models.extend(source.models_used);
    target.models_used = models.into_iter().collect();
    target.model_breakdowns =
        merge_model_breakdowns(target.model_breakdowns.drain(..), source.model_breakdowns);
    target.plugin_breakdowns =
        merge_plugin_breakdowns(target.plugin_breakdowns.drain(..), source.plugin_breakdowns);
    target.skill_breakdowns =
        merge_skill_breakdowns(target.skill_breakdowns.drain(..), source.skill_breakdowns);
    target.source_type_breakdowns = merge_source_type_breakdowns(
        target.source_type_breakdowns.drain(..),
        source.source_type_breakdowns,
    );
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
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn merge_plugin_breakdowns(
    existing: impl IntoIterator<Item = PluginBreakdown>,
    additional: impl IntoIterator<Item = PluginBreakdown>,
) -> Vec<PluginBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<PluginBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.plugin_name.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(PluginBreakdown {
                plugin_name: item.plugin_name.clone(),
                ..PluginBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn merge_skill_breakdowns(
    existing: impl IntoIterator<Item = SkillBreakdown>,
    additional: impl IntoIterator<Item = SkillBreakdown>,
) -> Vec<SkillBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SkillBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.skill_name.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(SkillBreakdown {
                skill_name: item.skill_name.clone(),
                ..SkillBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn merge_source_type_breakdowns(
    existing: impl IntoIterator<Item = SourceTypeBreakdown>,
    additional: impl IntoIterator<Item = SourceTypeBreakdown>,
) -> Vec<SourceTypeBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SourceTypeBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.source_type.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(SourceTypeBreakdown {
                source_type: item.source_type.clone(),
                ..SourceTypeBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn aggregate_model_breakdowns(rows: &[AllRow]) -> Vec<ModelBreakdown> {
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
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

fn aggregate_plugin_breakdowns(rows: &[AllRow]) -> Vec<PluginBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<PluginBreakdown> = Vec::new();
    for row in rows {
        for item in &row.plugin_breakdowns {
            let index = *indexes.entry(item.plugin_name.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(PluginBreakdown {
                    plugin_name: item.plugin_name.clone(),
                    ..PluginBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

fn aggregate_skill_breakdowns(rows: &[AllRow]) -> Vec<SkillBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SkillBreakdown> = Vec::new();
    for row in rows {
        for item in &row.skill_breakdowns {
            let index = *indexes.entry(item.skill_name.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(SkillBreakdown {
                    skill_name: item.skill_name.clone(),
                    ..SkillBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

fn aggregate_source_type_breakdowns(rows: &[AllRow]) -> Vec<SourceTypeBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SourceTypeBreakdown> = Vec::new();
    for row in rows {
        for item in &row.source_type_breakdowns {
            let index = *indexes.entry(item.source_type.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(SourceTypeBreakdown {
                    source_type: item.source_type.clone(),
                    ..SourceTypeBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_aggregates_plugin_skill_and_source_type_breakdowns_across_agents() {
        let mut accumulator = AllAccumulator::default();
        accumulator.add(AllRow {
            period: "2026-01-02".to_string(),
            agent: "claude",
            models_used: vec!["claude-sonnet-4-20250514".to_string()],
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 150,
            total_cost: 0.1,
            metadata: None,
            metadata_agents: None,
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "aws".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost: 0.1,
                ..PluginBreakdown::default()
            }],
            skill_breakdowns: Vec::new(),
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost: 0.1,
                ..SourceTypeBreakdown::default()
            }],
        });
        accumulator.add(AllRow {
            period: "2026-01-02".to_string(),
            agent: "codex",
            models_used: vec!["gpt-5.2-codex".to_string()],
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 30,
            total_cost: 0.02,
            metadata: None,
            metadata_agents: None,
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "unattributed".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..PluginBreakdown::default()
            }],
            skill_breakdowns: Vec::new(),
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..SourceTypeBreakdown::default()
            }],
        });

        let row = accumulator.into_row("2026-01-02".to_string());

        assert_eq!(row.plugin_breakdowns.len(), 2);
        assert!(
            row.plugin_breakdowns
                .iter()
                .any(|b| b.plugin_name == "aws" && b.input_tokens == 100)
        );
        assert_eq!(row.source_type_breakdowns.len(), 1);
        assert_eq!(row.source_type_breakdowns[0].input_tokens, 120);
    }
}
