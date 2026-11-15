use serde_json::{Value, json};

use ccusage_core::{ModelBreakdown, UsageSummary, json_float};

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
