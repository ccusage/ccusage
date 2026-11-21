use std::{
    collections::BTreeSet,
    io::{BufWriter, Write},
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ModelBreakdown, Result, Totals, UsageSummary,
    cli::{AgentReportKind, SharedArgs},
    cli_error,
};

pub fn wants_json(shared: &SharedArgs) -> bool {
    shared.json || shared.jq.is_some()
}

pub fn totals_json(rows: &[UsageSummary]) -> Value {
    Totals::from_summaries(rows).to_json()
}

impl Totals {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "inputTokens": self.input_tokens,
            "outputTokens": self.output_tokens,
            "cacheCreationTokens": self.cache_creation_tokens,
            "cacheReadTokens": self.cache_read_tokens,
            "totalTokens": self.total_tokens,
            "totalCost": self.total_cost,
        });
        if let Some(credits) = self.credits {
            value["credits"] = json!(credits);
        }
        attach_unpriced_models(&mut value, self.unpriced_models.clone());
        attach_estimated_models(&mut value, self.estimated_models.clone());
        value
    }
}

pub(crate) fn unpriced_models<'a>(
    breakdowns: impl IntoIterator<Item = &'a ModelBreakdown>,
) -> Vec<String> {
    breakdowns
        .into_iter()
        .filter(|breakdown| breakdown.missing_pricing)
        .map(|breakdown| breakdown.model_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn estimated_models<'a>(
    breakdowns: impl IntoIterator<Item = &'a ModelBreakdown>,
) -> Vec<String> {
    breakdowns
        .into_iter()
        .filter(|breakdown| breakdown.estimated_pricing)
        .map(|breakdown| breakdown.model_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn attach_estimated_models(totals: &mut Value, models: Vec<String>) {
    if !models.is_empty() {
        totals["estimatedModels"] = json!(models);
    }
}

/// Adds `unpricedModels` to JSON totals when any model lacked pricing.
pub fn attach_unpriced_models(totals: &mut Value, models: Vec<String>) {
    if !models.is_empty() {
        totals["unpricedModels"] = json!(models);
    }
}

pub fn print_json_or_jq(mut value: Value, jq: Option<&str>, no_cost: bool) -> Result<()> {
    if no_cost {
        strip_cost_json(&mut value);
    }
    write_json_or_jq(&value, jq)
}

pub fn write_json_or_jq(value: &impl Serialize, jq: Option<&str>) -> Result<()> {
    if let Some(filter) = jq {
        let mut child = std::process::Command::new("jq")
            .arg(filter)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .map_err(|error| cli_error(format!("failed to run jq: {error}")))?;
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = BufWriter::new(stdin);
            serde_json::to_writer(&mut stdin, value)?;
            stdin.write_all(b"\n")?;
            stdin.flush()?;
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(cli_error("jq failed"));
        }
    } else {
        let mut stdout = BufWriter::new(ccusage_terminal::stdout());
        serde_json::to_writer_pretty(&mut stdout, value)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

pub fn json_float(value: f64) -> Value {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        json!(value as i64)
    } else {
        json!(value)
    }
}

pub fn strip_cost_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("totalCost");
            map.remove("costUSD");
            map.remove("cost");
            map.remove("storedCost");
            for (_, child) in map.iter_mut() {
                strip_cost_json(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_cost_json(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub fn agent_summary_json(
    row: &UsageSummary,
    kind: AgentReportKind,
    include_session_metadata: bool,
) -> Value {
    let mut value = json!({
        period_key(kind): summary_period(row),
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
    });
    if let (Some(obj), Some(credits)) = (value.as_object_mut(), row.credits) {
        obj.insert("credits".to_string(), json!(credits));
    }
    if let (Some(obj), Some(message_count)) = (value.as_object_mut(), row.message_count) {
        obj.insert("messageCount".to_string(), json!(message_count));
    }
    if include_session_metadata && let Some(obj) = value.as_object_mut() {
        obj.insert(
            "lastActivity".to_string(),
            row.last_activity
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
        obj.insert(
            "firstActivity".to_string(),
            row.first_activity
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
        obj.insert(
            "projectPath".to_string(),
            row.project_path
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
    }
    value
}

pub fn summary_period(row: &UsageSummary) -> &str {
    row.date
        .as_deref()
        .or(row.week.as_deref())
        .or(row.month.as_deref())
        .or(row.session_id.as_deref())
        .unwrap_or_default()
}

fn period_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "date",
        AgentReportKind::Weekly => "week",
        AgentReportKind::Monthly => "month",
        AgentReportKind::Session => "sessionId",
    }
}

pub fn json_value_u64(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or_default()
}

pub fn non_empty_json_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}
