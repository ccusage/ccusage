use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{ModelBreakdown, cli::AgentReportKind, fast::FxHashMap, json_float};

pub(super) const SOURCE_BREAKDOWNS_METADATA_KEY: &str = "sourceBreakdowns";

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
    merge_source_breakdowns(&mut target.metadata, source.metadata);
}

pub(super) fn merge_source_breakdowns(target: &mut Option<Value>, source: Option<Value>) {
    let Some(Value::Object(source_metadata)) = source else {
        return;
    };
    let Some(Value::Array(source_breakdowns)) = source_metadata.get(SOURCE_BREAKDOWNS_METADATA_KEY)
    else {
        return;
    };
    let source_breakdowns = source_breakdowns.clone();
    if !matches!(target, Some(Value::Object(_))) {
        *target = Some(Value::Object(Map::new()));
    }
    let Some(target_metadata) = target.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let target_breakdowns = target_metadata
        .entry(SOURCE_BREAKDOWNS_METADATA_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(target_breakdowns) = target_breakdowns.as_array_mut() else {
        return;
    };
    for source_breakdown in source_breakdowns {
        let Some(source_name) = source_breakdown.get("source").and_then(Value::as_str) else {
            continue;
        };
        let Some(index) = target_breakdowns.iter().position(|target_breakdown| {
            target_breakdown.get("source").and_then(Value::as_str) == Some(source_name)
        }) else {
            target_breakdowns.push(source_breakdown);
            continue;
        };
        merge_source_breakdown(&mut target_breakdowns[index], source_breakdown);
    }
}

fn merge_source_breakdown(target: &mut Value, source: Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    for key in [
        "inputTokens",
        "outputTokens",
        "cacheCreationTokens",
        "cacheReadTokens",
        "reasoningOutputTokens",
        "totalTokens",
        "totalCost",
    ] {
        add_json_number(target, source, key);
    }
    merge_string_array(target, source, "modelsUsed");
    merge_model_breakdown_values(target, source);
}

fn add_json_number(target: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    let Some(source_value) = source.get(key) else {
        return;
    };
    let Some(target_value) = target.get(key) else {
        target.insert(key.to_string(), source_value.clone());
        return;
    };
    let value = match (target_value.as_u64(), source_value.as_u64()) {
        (Some(target), Some(source)) => json!(target + source),
        _ => {
            json_float(target_value.as_f64().unwrap_or(0.0) + source_value.as_f64().unwrap_or(0.0))
        }
    };
    target.insert(key.to_string(), value);
}

fn merge_string_array(target: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    let Some(Value::Array(source_values)) = source.get(key) else {
        return;
    };
    let target_values = target
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(target_values) = target_values.as_array_mut() else {
        return;
    };
    for source_value in source_values {
        if !target_values.contains(source_value) {
            target_values.push(source_value.clone());
        }
    }
    target_values.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
}

fn merge_model_breakdown_values(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    let Some(Value::Array(source_models)) = source.get("modelBreakdowns") else {
        return;
    };
    let target_models = target
        .entry("modelBreakdowns".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(target_models) = target_models.as_array_mut() else {
        return;
    };
    for source_model in source_models {
        let Some(model_name) = source_model.get("modelName").and_then(Value::as_str) else {
            continue;
        };
        let Some(index) = target_models.iter().position(|target_model| {
            target_model.get("modelName").and_then(Value::as_str) == Some(model_name)
        }) else {
            target_models.push(source_model.clone());
            continue;
        };
        let (Some(target_model), Some(source_model)) = (
            target_models[index].as_object_mut(),
            source_model.as_object(),
        ) else {
            continue;
        };
        for key in [
            "inputTokens",
            "outputTokens",
            "cacheCreationTokens",
            "cacheReadTokens",
            "cost",
        ] {
            add_json_number(target_model, source_model, key);
        }
    }
    target_models.sort_by(|a, b| {
        let cost_order = b
            .get("cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .total_cmp(&a.get("cost").and_then(Value::as_f64).unwrap_or(0.0));
        cost_order.then_with(|| {
            a.get("modelName")
                .and_then(Value::as_str)
                .cmp(&b.get("modelName").and_then(Value::as_str))
        })
    });
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
