use std::{fs, path::Path};

use serde::Deserialize;
use serde_json::Value;

use crate::{Context, ModelBreakdown, Result, cli_error};

use super::{BUILT_IN_AGENT_NAMES, loader::leak_agent_name, types::AllRow};

#[derive(Deserialize)]
struct DailyReport {
    daily: Vec<DailyRow>,
    totals: ReportTotals,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyRow {
    period: String,
    agent: String,
    models_used: Vec<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
    #[serde(default)]
    metadata: Option<Value>,
    #[serde(default)]
    model_breakdowns: Vec<ImportedModelBreakdown>,
    #[serde(default)]
    agents: Vec<ImportedAgentRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportedAgentRow {
    agent: String,
    models_used: Vec<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
    #[serde(default)]
    model_breakdowns: Vec<ImportedModelBreakdown>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportedModelBreakdown {
    model_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    cost: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportTotals {
    _input_tokens: u64,
    _output_tokens: u64,
    _cache_creation_tokens: u64,
    _cache_read_tokens: u64,
    _total_tokens: u64,
    _total_cost: f64,
}

fn parse_report(json: &str) -> Result<Vec<AllRow>> {
    let report: DailyReport = serde_json::from_str(json)?;
    let _ = report.totals;
    report.daily.into_iter().map(DailyRow::into_row).collect()
}

pub(super) fn load(path: &Path) -> Result<Vec<AllRow>> {
    let json = fs::read_to_string(path).context(format!(
        "failed to read daily report from {}",
        path.display()
    ))?;
    parse_report(&json).context(format!(
        "failed to parse daily report from {}",
        path.display()
    ))
}

impl DailyRow {
    fn into_row(self) -> Result<AllRow> {
        let metadata_agents = metadata_agents(self.metadata.as_ref())?;
        let agent_breakdowns = self
            .agents
            .into_iter()
            .map(|row| row.into_row(&self.period))
            .collect::<Result<Vec<_>>>()?;
        Ok(AllRow {
            period: self.period,
            agent: resolve_agent(&self.agent)?,
            models_used: self.models_used,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            total_tokens: self.total_tokens,
            total_cost: self.total_cost,
            metadata: self.metadata,
            metadata_agents,
            agent_breakdowns: (!agent_breakdowns.is_empty()).then_some(agent_breakdowns),
            model_breakdowns: convert_model_breakdowns(self.model_breakdowns),
        })
    }
}

impl ImportedAgentRow {
    fn into_row(self, period: &str) -> Result<AllRow> {
        let agent = resolve_agent(&self.agent)?;
        Ok(AllRow {
            period: period.to_string(),
            agent,
            models_used: self.models_used,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            total_tokens: self.total_tokens,
            total_cost: self.total_cost,
            metadata: None,
            metadata_agents: Some(vec![agent]),
            agent_breakdowns: None,
            model_breakdowns: convert_model_breakdowns(self.model_breakdowns),
        })
    }
}

fn convert_model_breakdowns(rows: Vec<ImportedModelBreakdown>) -> Vec<ModelBreakdown> {
    rows.into_iter()
        .map(|row| ModelBreakdown {
            model_name: row.model_name,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cost: row.cost,
            ..ModelBreakdown::default()
        })
        .collect()
}

fn metadata_agents(metadata: Option<&Value>) -> Result<Option<Vec<&'static str>>> {
    let Some(agents) = metadata
        .and_then(|value| value.get("agents"))
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    agents
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| cli_error("metadata.agents entries must be strings"))
                .and_then(resolve_agent)
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn resolve_agent(agent: &str) -> Result<&'static str> {
    if agent == "all" {
        return Ok("all");
    }
    Ok(BUILT_IN_AGENT_NAMES
        .iter()
        .copied()
        .find(|known| *known == agent)
        .unwrap_or_else(|| leak_agent_name(agent)))
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;

    use super::*;

    #[test]
    fn parses_ccusage_daily_report_rows() {
        let rows = parse_report(
            r#"{
                "daily": [{
                    "agent": "all",
                    "modelsUsed": ["claude-sonnet-4-20250514"],
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "cacheCreationTokens": 5,
                    "cacheReadTokens": 10,
                    "totalTokens": 135,
                    "totalCost": 0.25,
                    "metadata": {"agents": ["claude"]},
                    "agents": [{
                        "agent": "claude",
                        "modelsUsed": ["claude-sonnet-4-20250514"],
                        "inputTokens": 100,
                        "outputTokens": 20,
                        "cacheCreationTokens": 5,
                        "cacheReadTokens": 10,
                        "totalTokens": 135,
                        "totalCost": 0.25,
                        "modelBreakdowns": []
                    }],
                    "modelBreakdowns": [{
                        "modelName": "claude-sonnet-4-20250514",
                        "inputTokens": 100,
                        "outputTokens": 20,
                        "cacheCreationTokens": 5,
                        "cacheReadTokens": 10,
                        "cost": 0.25
                    }],
                    "period": "2026-07-26"
                }],
                "totals": {
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "cacheCreationTokens": 5,
                    "cacheReadTokens": 10,
                    "totalTokens": 135,
                    "totalCost": 0.25
                }
            }"#,
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].period, "2026-07-26");
        assert_eq!(rows[0].total_tokens, 135);
        assert_eq!(rows[0].model_breakdowns.len(), 1);
        assert_eq!(rows[0].metadata_agents, Some(vec!["claude"]));
        assert_eq!(rows[0].agent_breakdowns.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn loads_daily_report_from_file() {
        let fixture = fs_fixture!({
            "fleet-report.json": r#"{
                "daily": [],
                "totals": {
                    "inputTokens": 0,
                    "outputTokens": 0,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 0,
                    "totalTokens": 0,
                    "totalCost": 0
                }
            }"#,
        });

        let rows = load(&fixture.path("fleet-report.json")).unwrap();

        assert!(rows.is_empty());
    }

    #[test]
    fn parses_named_pi_store_agents_emitted_by_ccusage() {
        let rows = parse_report(
            r#"{
                "daily": [{
                    "agent": "omp",
                    "modelsUsed": ["omp/gpt-5"],
                    "inputTokens": 1,
                    "outputTokens": 2,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 0,
                    "totalTokens": 3,
                    "totalCost": 0,
                    "modelBreakdowns": [],
                    "period": "2026-07-26"
                }],
                "totals": {
                    "inputTokens": 1,
                    "outputTokens": 2,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 0,
                    "totalTokens": 3,
                    "totalCost": 0
                }
            }"#,
        )
        .unwrap();

        assert_eq!(rows[0].agent, "omp");
    }
}
