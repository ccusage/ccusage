use serde::{
    Serialize,
    ser::{SerializeMap, Serializer},
};
use serde_json::{Value, json};

use super::totals;
use crate::{
    Result, attach_estimated_models, attach_unpriced_models,
    cli::AgentReportKind,
    json_float,
    output::{strip_cost_json, write_json_or_jq},
    types::ReportRow,
};
use ccusage_adapter_common::rows::RowUsage;

pub(crate) fn report_json_with_agents(
    rows: &[ReportRow],
    kind: AgentReportKind,
    include_agents: bool,
) -> Value {
    json!({
        rows_key(kind): rows.iter().map(|row| row_json(row, include_agents)).collect::<Vec<_>>(),
        "totals": totals_json(rows),
    })
}

pub(crate) fn sections_report_json(
    sections: &[(AgentReportKind, Vec<ReportRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
) -> OrderedJsonMap {
    let mut fields = Vec::with_capacity(sections.len() + 1);
    for (kind, rows) in sections {
        fields.push((
            rows_key(*kind),
            Value::Array(
                rows.iter()
                    .map(|row| row_json(row, include_agents))
                    .collect(),
            ),
        ));
    }
    let command_rows = sections
        .iter()
        .find_map(|(kind, rows)| (*kind == command_kind).then_some(rows.as_slice()))
        .unwrap_or(&[]);
    fields.push(("totals", totals_json(command_rows)));
    OrderedJsonMap { fields }
}

pub(crate) fn print_sections_report_json(
    sections: &[(AgentReportKind, Vec<ReportRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
    jq: Option<&str>,
    no_cost: bool,
) -> Result<()> {
    let mut report = sections_report_json(sections, command_kind, include_agents);
    if no_cost {
        report.strip_costs();
    }
    write_json_or_jq(&report, jq)
}

pub(crate) struct OrderedJsonMap {
    fields: Vec<(&'static str, Value)>,
}

impl OrderedJsonMap {
    fn strip_costs(&mut self) {
        for (_, value) in &mut self.fields {
            strip_cost_json(value);
        }
    }
}

impl Serialize for OrderedJsonMap {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.fields.len()))?;
        for (key, value) in &self.fields {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

fn row_json(row: &ReportRow, include_agents: bool) -> Value {
    let mut value = agent_json(row.agent(), row);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("period".to_string(), json!(row.period));
        match row {
            ReportRow::Source(_) => {
                if let Some(metadata) = row.metadata() {
                    obj.insert("metadata".to_string(), json!(metadata));
                }
            }
            ReportRow::Period(period) => {
                obj.insert("metadata".to_string(), json!({ "agents": row.agents() }));
                if include_agents {
                    obj.insert(
                        "agents".to_string(),
                        Value::Array(
                            period
                                .agent_breakdowns
                                .iter()
                                .map(|source| agent_json(source.agent, &source.usage))
                                .collect(),
                        ),
                    );
                }
            }
        }
    }
    value
}

fn agent_json(agent: &str, row: &RowUsage) -> Value {
    json!({
        "agent": agent,
        "modelsUsed": row.models_used,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens,
        "totalCost": json_float(row.total_cost),
        "modelBreakdowns": row.model_breakdowns,
    })
}

fn totals_json(rows: &[ReportRow]) -> Value {
    let totals = totals(rows);
    let mut value = json!({
        "inputTokens": totals.input_tokens,
        "outputTokens": totals.output_tokens,
        "cacheCreationTokens": totals.cache_creation_tokens,
        "cacheReadTokens": totals.cache_read_tokens,
        "totalTokens": totals.total_tokens,
        "totalCost": json_float(totals.total_cost),
    });
    attach_unpriced_models(&mut value, totals.unpriced_models);
    attach_estimated_models(&mut value, totals.estimated_models);
    value
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "session",
    }
}
