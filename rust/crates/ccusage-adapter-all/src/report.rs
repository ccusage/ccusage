use std::{collections::BTreeSet, io::IsTerminal};

use serde::{
    Serialize,
    ser::{SerializeMap, Serializer},
};
use serde_json::{Value, json};

use crate::{
    Align, Color, ModelBreakdown, Result, SimpleTable, Totals, TotalsRow, attach_unpriced_models,
    cli::{AgentReportKind, SharedArgs, SortOrder},
    color, format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    json_float,
    output::{strip_cost_json, write_json_or_jq},
    print_box_title, print_missing_pricing_warnings_for_models, should_use_compact_layout,
};

use ccusage_adapter_common::rows::UnifiedRow;

pub(super) fn report_json_with_agents(
    rows: &[UnifiedRow],
    kind: AgentReportKind,
    include_agents: bool,
) -> Value {
    json!({
        rows_key(kind): rows.iter().map(|row| row_json(row, include_agents)).collect::<Vec<_>>(),
        "totals": totals_json(rows),
    })
}

pub(super) fn sections_report_json(
    sections: &[(AgentReportKind, Vec<UnifiedRow>)],
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

pub(super) fn print_sections_report_json(
    sections: &[(AgentReportKind, Vec<UnifiedRow>)],
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

pub(super) struct OrderedJsonMap {
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

fn row_json(row: &UnifiedRow, include_agents: bool) -> Value {
    let mut value = agent_json(row);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("period".to_string(), json!(row.period));
    }
    if let (Some(obj), Some(agents)) = (value.as_object_mut(), row.metadata_agents.as_ref()) {
        obj.insert(
            "metadata".to_string(),
            row.metadata
                .clone()
                .unwrap_or_else(|| json!({ "agents": agents })),
        );
    } else if let (Some(obj), Some(metadata)) = (value.as_object_mut(), row.metadata.as_ref()) {
        obj.insert("metadata".to_string(), metadata.clone());
    }
    if include_agents
        && let (Some(obj), Some(agent_breakdowns)) =
            (value.as_object_mut(), row.agent_breakdowns.as_ref())
    {
        obj.insert(
            "agents".to_string(),
            Value::Array(agent_breakdowns.iter().map(agent_json).collect()),
        );
    }
    value
}

fn agent_json(row: &UnifiedRow) -> Value {
    json!({
        "agent": row.agent,
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

fn totals(rows: &[UnifiedRow]) -> Totals {
    Totals::from_rows(rows.iter().map(|row| TotalsRow {
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_creation_tokens: row.cache_creation_tokens,
        cache_read_tokens: row.cache_read_tokens,
        total_tokens: row.total_tokens,
        cost: row.total_cost,
        credits: None,
        model_breakdowns: &row.model_breakdowns,
    }))
}

fn totals_json(rows: &[UnifiedRow]) -> Value {
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

pub(super) fn print_table(
    rows: &[UnifiedRow],
    kind: AgentReportKind,
    shared: &SharedArgs,
    detected_agents: &[&'static str],
) -> Result<()> {
    print_box_title(&all_report_title(kind, detected_agents), shared);
    if rows.is_empty() {
        eprintln!("No usage data found.");
        return Ok(());
    }
    let terminal_width = crate::terminal_width();
    let is_tty = std::io::stdout().is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        crate::USAGE_COMPACT_WIDTH_THRESHOLD,
    );
    let (headers, aligns) = all_table_columns(kind, compact, shared.no_cost);
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);

    for row in rows {
        table.push(all_table_row(row, compact, false, shared.no_cost));
        if let Some(agent_breakdowns) = row.agent_breakdowns.as_ref() {
            for breakdown in agent_breakdowns {
                table.push(all_table_row(breakdown, compact, true, shared.no_cost));
                if shared.breakdown && !breakdown.model_breakdowns.is_empty() {
                    push_model_breakdown_rows(
                        &mut table,
                        &breakdown.model_breakdowns,
                        compact,
                        shared,
                    );
                }
            }
        } else if shared.breakdown && !row.model_breakdowns.is_empty() {
            push_model_breakdown_rows(&mut table, &row.model_breakdowns, compact, shared);
        }
    }
    table.separator();
    let totals = totals(rows);
    let total = |value: String| color(shared, value, Color::Yellow);
    let mut total_row = if compact {
        vec![
            total("Total".to_string()),
            String::new(),
            String::new(),
            total(format_number(totals.input_tokens)),
            total(format_number(totals.output_tokens)),
            total(format_currency(totals.total_cost)),
        ]
    } else {
        vec![
            total("Total".to_string()),
            String::new(),
            String::new(),
            total(format_number(totals.input_tokens)),
            total(format_number(totals.output_tokens)),
            total(format_number(totals.cache_creation_tokens)),
            total(format_number(totals.cache_read_tokens)),
            total(format_number(totals.total_tokens)),
            total(format_currency(totals.total_cost)),
        ]
    };
    if shared.no_cost {
        total_row.pop();
    }
    table.push(total_row);
    table.print()?;
    print_missing_pricing_warnings_for_models(
        totals.unpriced_models.iter().map(String::as_str),
        shared.offline,
    );
    if compact {
        eprintln!("\nRunning in Compact Mode");
        eprintln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

pub(super) fn all_report_title(kind: AgentReportKind, detected_agents: &[&'static str]) -> String {
    format!(
        "Coding (Agent) CLI Usage Report - {}\nDetected: {}",
        match kind {
            AgentReportKind::Daily => "Daily",
            AgentReportKind::Weekly => "Weekly",
            AgentReportKind::Monthly => "Monthly",
            AgentReportKind::Session => "Session",
        },
        detected_agent_labels(detected_agents)
    )
}

fn detected_agent_labels(detected_agents: &[&'static str]) -> String {
    let agents = detected_agents.iter().copied().collect::<BTreeSet<_>>();
    if agents.is_empty() {
        return "None".to_string();
    }
    agents
        .into_iter()
        .map(agent_label)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn all_table_row(
    row: &UnifiedRow,
    compact: bool,
    breakdown: bool,
    no_cost: bool,
) -> Vec<String> {
    let period = if breakdown {
        String::new()
    } else {
        row.period.clone()
    };
    let agent = if breakdown {
        format!("- {}", agent_label(row.agent))
    } else if row.agent_breakdowns.is_some() {
        "All".to_string()
    } else {
        agent_label(row.agent).to_string()
    };
    let models = if row.agent_breakdowns.is_some() {
        String::new()
    } else {
        format_models_multiline(&row.models_used)
    };

    if compact {
        let mut values = vec![
            period,
            agent,
            models,
            format_number(row.input_tokens),
            format_number(row.output_tokens),
            format_currency(row.total_cost),
        ];
        if no_cost {
            values.pop();
        }
        return values;
    }

    let mut values = vec![
        period,
        agent,
        models,
        format_number(row.input_tokens),
        format_number(row.output_tokens),
        format_number(row.cache_creation_tokens),
        format_number(row.cache_read_tokens),
        format_number(row.total_tokens),
        format_currency(row.total_cost),
    ];
    if no_cost {
        values.pop();
    }
    values
}

fn push_model_breakdown_rows(
    table: &mut SimpleTable,
    breakdowns: &[ModelBreakdown],
    compact: bool,
    shared: &SharedArgs,
) {
    for b in breakdowns {
        let total = b.total_tokens();
        let model = color(
            shared,
            format_breakdown_model_label(&b.model_name),
            Color::Grey,
        );
        if compact {
            let mut row = vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(b.input_tokens), Color::Grey),
                color(shared, format_number(b.output_tokens), Color::Grey),
                color(shared, format_currency(b.cost), Color::Grey),
            ];
            if shared.no_cost {
                row.pop();
            }
            table.push(row);
        } else {
            let mut row = vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(b.input_tokens), Color::Grey),
                color(shared, format_number(b.output_tokens), Color::Grey),
                color(shared, format_number(b.cache_creation_tokens), Color::Grey),
                color(shared, format_number(b.cache_read_tokens), Color::Grey),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(b.cost), Color::Grey),
            ];
            if shared.no_cost {
                row.pop();
            }
            table.push(row);
        }
    }
}

pub(super) fn all_table_columns(
    kind: AgentReportKind,
    compact: bool,
    no_cost: bool,
) -> (Vec<&'static str>, Vec<Align>) {
    let (mut headers, mut aligns) = if compact {
        (
            vec![
                first_column(kind),
                "Agent",
                "Models",
                "Input",
                "Output",
                "Cost (USD)",
            ],
            vec![
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right,
            ],
        )
    } else {
        (
            vec![
                first_column(kind),
                "Agent",
                "Models",
                "Input",
                "Output",
                "Cache Create",
                "Cache Read",
                "Total Tokens",
                "Cost (USD)",
            ],
            vec![
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
            ],
        )
    };
    if no_cost {
        headers.pop();
        aligns.pop();
    }
    (headers, aligns)
}

pub(super) fn sort_rows(rows: &mut [UnifiedRow], order: &SortOrder) {
    rows.sort_by(|a, b| match a.period.cmp(&b.period) {
        std::cmp::Ordering::Equal => a.agent.cmp(b.agent),
        order => order,
    });
    if *order == SortOrder::Desc {
        rows.reverse();
    }
}

fn first_column(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Date",
        AgentReportKind::Weekly => "Week",
        AgentReportKind::Monthly => "Month",
        AgentReportKind::Session => "Session",
    }
}

fn agent_label(agent: &str) -> &str {
    match agent {
        "all" => "All",
        "claude" => "Claude",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "amp" => "Amp",
        "droid" => "Droid",
        "codebuff" => "Codebuff",
        "hermes" => "Hermes",
        "pi" => "pi-agent",
        "goose" => "Goose",
        "openclaw" => "OpenClaw",
        "kilo" => "Kilo",
        "copilot" => "GitHub Copilot CLI",
        "gemini" => "Gemini CLI",
        "antigravity" => "Antigravity",
        "kimi" => "Kimi",
        "qwen" => "Qwen",
        "grok" => "Grok",
        "zcode" => "ZCode",
        "cline" => "Cline",
        "dsh" => "DeepSeek Harness",
        _ => agent,
    }
}
