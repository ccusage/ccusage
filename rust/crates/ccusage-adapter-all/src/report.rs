use std::{
    collections::BTreeSet,
    io::{BufWriter, IsTerminal, Write},
};

use serde::{
    Serialize,
    ser::{SerializeMap, Serializer},
};
use serde_json::{Value, json};

use crate::{
    Align, Color, ModelBreakdown, Result, SimpleTable, UsageSummary,
    cli::{AgentReportKind, SharedArgs, SortOrder},
    cli_error, color, format_currency, format_models_multiline, format_number, json_float,
    output::strip_cost_json,
    print_box_title, sanitize_terminal_text, short_model_name, should_use_compact_layout,
};

use super::types::{AllRow, SOURCE_BREAKDOWNS_METADATA_KEY, merge_source_breakdowns};

#[cfg(test)]
pub(super) fn report_json(rows: &[AllRow], kind: AgentReportKind) -> Value {
    report_json_with_options(rows, kind, false, false)
}

pub(super) fn report_json_with_agents(
    rows: &[AllRow],
    kind: AgentReportKind,
    include_agents: bool,
) -> Value {
    report_json_with_options(rows, kind, include_agents, false)
}

pub(super) fn report_json_with_options(
    rows: &[AllRow],
    kind: AgentReportKind,
    include_agents: bool,
    include_sources: bool,
) -> Value {
    json!({
        rows_key(kind): rows.iter().map(|row| row_json(row, include_agents, include_sources)).collect::<Vec<_>>(),
        "totals": totals_json(rows, include_sources),
    })
}

#[cfg(test)]
pub(super) fn sections_report_json(
    sections: &[(AgentReportKind, Vec<AllRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
) -> OrderedJsonMap {
    sections_report_json_with_options(sections, command_kind, include_agents, false)
}

fn sections_report_json_with_options(
    sections: &[(AgentReportKind, Vec<AllRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
    include_sources: bool,
) -> OrderedJsonMap {
    let mut fields = Vec::with_capacity(sections.len() + 1);
    for (kind, rows) in sections {
        fields.push((
            rows_key(*kind),
            Value::Array(
                rows.iter()
                    .map(|row| row_json(row, include_agents, include_sources))
                    .collect(),
            ),
        ));
    }
    let command_rows = sections
        .iter()
        .find_map(|(kind, rows)| (*kind == command_kind).then_some(rows.as_slice()))
        .unwrap_or(&[]);
    fields.push(("totals", totals_json(command_rows, include_sources)));
    OrderedJsonMap { fields }
}

pub(super) fn print_sections_report_json(
    sections: &[(AgentReportKind, Vec<AllRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
    jq: Option<&str>,
    no_cost: bool,
) -> Result<()> {
    print_sections_report_json_with_options(
        sections,
        command_kind,
        include_agents,
        false,
        jq,
        no_cost,
    )
}

pub(super) fn print_sections_report_json_with_options(
    sections: &[(AgentReportKind, Vec<AllRow>)],
    command_kind: AgentReportKind,
    include_agents: bool,
    include_sources: bool,
    jq: Option<&str>,
    no_cost: bool,
) -> Result<()> {
    let mut report =
        sections_report_json_with_options(sections, command_kind, include_agents, include_sources);
    if no_cost {
        report.strip_costs();
    }
    if let Some(filter) = jq {
        let mut child = std::process::Command::new("jq")
            .arg(filter)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .map_err(|error| cli_error(format!("failed to run jq: {error}")))?;
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = BufWriter::new(stdin);
            serde_json::to_writer(&mut stdin, &report)?;
            stdin.write_all(b"\n")?;
            stdin.flush()?;
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(cli_error("jq failed"));
        }
    } else {
        let stdout = std::io::stdout();
        let mut stdout = BufWriter::new(stdout.lock());
        serde_json::to_writer_pretty(&mut stdout, &report)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

pub(super) struct OrderedJsonMap {
    fields: Vec<(&'static str, Value)>,
}

impl OrderedJsonMap {
    #[cfg(test)]
    pub(super) fn get(&self, key: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find_map(|(field, value)| (*field == key).then_some(value))
    }

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

fn row_json(row: &AllRow, include_agents: bool, include_sources: bool) -> Value {
    let mut value = agent_json_with_sources(row, include_sources);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("period".to_string(), json!(row.period));
    }
    let mut metadata = row.metadata.clone();
    if let Some(metadata_object) = metadata.as_mut().and_then(Value::as_object_mut) {
        metadata_object.remove(SOURCE_BREAKDOWNS_METADATA_KEY);
        if metadata_object.is_empty() {
            metadata = None;
        }
    }
    if let (Some(obj), Some(agents)) = (value.as_object_mut(), row.metadata_agents.as_ref()) {
        obj.insert(
            "metadata".to_string(),
            metadata.unwrap_or_else(|| json!({ "agents": agents })),
        );
    } else if let (Some(obj), Some(metadata)) = (value.as_object_mut(), metadata) {
        obj.insert("metadata".to_string(), metadata);
    }
    if include_agents
        && let (Some(obj), Some(agent_breakdowns)) =
            (value.as_object_mut(), row.agent_breakdowns.as_ref())
    {
        obj.insert(
            "agents".to_string(),
            Value::Array(
                agent_breakdowns
                    .iter()
                    .map(|row| agent_json_with_sources(row, include_sources))
                    .collect(),
            ),
        );
    }
    value
}

fn agent_json_with_sources(row: &AllRow, include_sources: bool) -> Value {
    let mut value = agent_json(row);
    if include_sources
        && let Some(source_breakdowns) = source_breakdowns_for_row(row)
        && let Some(obj) = value.as_object_mut()
    {
        obj.insert(
            SOURCE_BREAKDOWNS_METADATA_KEY.to_string(),
            source_breakdowns,
        );
    }
    value
}

fn agent_json(row: &AllRow) -> Value {
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

fn totals_json(rows: &[AllRow], include_sources: bool) -> Value {
    let mut totals = json!({
        "inputTokens": rows.iter().map(|row| row.input_tokens).sum::<u64>(),
        "outputTokens": rows.iter().map(|row| row.output_tokens).sum::<u64>(),
        "cacheCreationTokens": rows.iter().map(|row| row.cache_creation_tokens).sum::<u64>(),
        "cacheReadTokens": rows.iter().map(|row| row.cache_read_tokens).sum::<u64>(),
        "totalTokens": rows.iter().map(|row| row.total_tokens).sum::<u64>(),
        "totalCost": json_float(rows.iter().map(|row| row.total_cost).sum::<f64>()),
    });
    if include_sources {
        totals[SOURCE_BREAKDOWNS_METADATA_KEY] = source_breakdowns_for_rows(rows);
    }
    totals
}

fn source_breakdowns_for_row(row: &AllRow) -> Option<Value> {
    if let Some(metadata) = row.metadata.as_ref()
        && let Some(source_breakdowns) = metadata.get(SOURCE_BREAKDOWNS_METADATA_KEY)
    {
        return Some(source_breakdowns.clone());
    }
    if row.agent != "all" {
        return None;
    }
    let mut metadata = None;
    for breakdown in row
        .agent_breakdowns
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|breakdown| breakdown.agent == "codex")
    {
        merge_source_breakdowns(&mut metadata, breakdown.metadata.clone());
    }
    metadata.and_then(|metadata| metadata.get(SOURCE_BREAKDOWNS_METADATA_KEY).cloned())
}

fn source_breakdowns_for_rows(rows: &[AllRow]) -> Value {
    let mut metadata = None;
    for row in rows {
        let Some(source_breakdowns) = source_breakdowns_for_row(row) else {
            continue;
        };
        merge_source_breakdowns(
            &mut metadata,
            Some(json!({ SOURCE_BREAKDOWNS_METADATA_KEY: source_breakdowns })),
        );
    }
    metadata
        .and_then(|metadata| metadata.get(SOURCE_BREAKDOWNS_METADATA_KEY).cloned())
        .unwrap_or_else(|| Value::Array(Vec::new()))
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
    rows: &[AllRow],
    kind: AgentReportKind,
    shared: &SharedArgs,
    detected_agents: &[&'static str],
) -> Result<()> {
    print_table_with_options(rows, kind, shared, detected_agents, false)
}

pub(super) fn print_table_with_options(
    rows: &[AllRow],
    kind: AgentReportKind,
    shared: &SharedArgs,
    detected_agents: &[&'static str],
    include_sources: bool,
) -> Result<()> {
    print_box_title(&all_report_title(kind, rows, detected_agents), shared);
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
                if include_sources {
                    push_source_breakdown_rows(&mut table, breakdown, compact, shared);
                }
                if shared.breakdown && !breakdown.model_breakdowns.is_empty() {
                    push_model_breakdown_rows(
                        &mut table,
                        &breakdown.model_breakdowns,
                        compact,
                        shared,
                    );
                }
            }
        } else {
            if include_sources {
                push_source_breakdown_rows(&mut table, row, compact, shared);
            }
            if shared.breakdown && !row.model_breakdowns.is_empty() {
                push_model_breakdown_rows(&mut table, &row.model_breakdowns, compact, shared);
            }
        }
    }
    table.separator();
    let totals = totals_json(rows, include_sources);
    let table_total_tokens = rows.iter().map(table_total_tokens).sum::<u64>();
    if compact {
        let mut total_row = vec![
            color(shared, "Total", Color::Yellow),
            String::new(),
            String::new(),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("inputTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("outputTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_currency(
                    totals
                        .get("totalCost")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                ),
                Color::Yellow,
            ),
        ];
        if shared.no_cost {
            total_row.pop();
        }
        table.push(total_row);
    } else {
        let mut total_row = vec![
            color(shared, "Total", Color::Yellow),
            String::new(),
            String::new(),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("inputTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("outputTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("cacheCreationTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_number(crate::json_value_u64(totals.get("cacheReadTokens"))),
                Color::Yellow,
            ),
            color(shared, format_number(table_total_tokens), Color::Yellow),
            color(
                shared,
                format_currency(
                    totals
                        .get("totalCost")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                ),
                Color::Yellow,
            ),
        ];
        if shared.no_cost {
            total_row.pop();
        }
        table.push(total_row);
    }
    table.print()?;
    crate::print_missing_pricing_warnings(&all_rows_as_usage_summaries(rows), shared.offline);
    if compact {
        eprintln!("\nRunning in Compact Mode");
        eprintln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

fn push_source_breakdown_rows(
    table: &mut SimpleTable,
    row: &AllRow,
    compact: bool,
    shared: &SharedArgs,
) {
    let Some(Value::Array(source_breakdowns)) = source_breakdowns_for_row(row) else {
        return;
    };
    for source_breakdown in source_breakdowns {
        table.push(source_table_row(&source_breakdown, compact, shared.no_cost));
        if shared.breakdown {
            push_source_model_breakdown_rows(table, &source_breakdown, compact, shared);
        }
    }
}

pub(super) fn source_table_row(source: &Value, compact: bool, no_cost: bool) -> Vec<String> {
    let models = source
        .get("modelsUsed")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let source_name = source
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("Uncategorized");
    let source_name = sanitize_terminal_text(source_name);
    let input = crate::json_value_u64(source.get("inputTokens"));
    let output = crate::json_value_u64(source.get("outputTokens"));
    let cache_creation = crate::json_value_u64(source.get("cacheCreationTokens"));
    let cache_read = crate::json_value_u64(source.get("cacheReadTokens"));
    let cost = source
        .get("totalCost")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut values = if compact {
        vec![
            String::new(),
            format!("- {source_name}"),
            format_models_multiline(&models),
            format_number(input),
            format_number(output),
            format_currency(cost),
        ]
    } else {
        vec![
            String::new(),
            format!("- {source_name}"),
            format_models_multiline(&models),
            format_number(input),
            format_number(output),
            format_number(cache_creation),
            format_number(cache_read),
            format_number(
                input
                    .saturating_add(output)
                    .saturating_add(cache_creation)
                    .saturating_add(cache_read),
            ),
            format_currency(cost),
        ]
    };
    if no_cost {
        values.pop();
    }
    values
}

fn push_source_model_breakdown_rows(
    table: &mut SimpleTable,
    source: &Value,
    compact: bool,
    shared: &SharedArgs,
) {
    let Some(Value::Array(model_breakdowns)) = source.get("modelBreakdowns") else {
        return;
    };
    for model_breakdown in model_breakdowns {
        let model = color(
            shared,
            format!(
                "- {}",
                short_model_name(
                    model_breakdown
                        .get("modelName")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            ),
            Color::Grey,
        );
        let input = crate::json_value_u64(model_breakdown.get("inputTokens"));
        let output = crate::json_value_u64(model_breakdown.get("outputTokens"));
        let cache_creation = crate::json_value_u64(model_breakdown.get("cacheCreationTokens"));
        let cache_read = crate::json_value_u64(model_breakdown.get("cacheReadTokens"));
        let cost = model_breakdown
            .get("cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let mut row = if compact {
            vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(input), Color::Grey),
                color(shared, format_number(output), Color::Grey),
                color(shared, format_currency(cost), Color::Grey),
            ]
        } else {
            vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(input), Color::Grey),
                color(shared, format_number(output), Color::Grey),
                color(shared, format_number(cache_creation), Color::Grey),
                color(shared, format_number(cache_read), Color::Grey),
                color(
                    shared,
                    format_number(
                        input
                            .saturating_add(output)
                            .saturating_add(cache_creation)
                            .saturating_add(cache_read),
                    ),
                    Color::Grey,
                ),
                color(shared, format_currency(cost), Color::Grey),
            ]
        };
        if shared.no_cost {
            row.pop();
        }
        table.push(row);
    }
}

fn all_rows_as_usage_summaries(rows: &[AllRow]) -> Vec<UsageSummary> {
    rows.iter()
        .map(|row| UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            extra_total_tokens: row.total_tokens.saturating_sub(table_total_tokens(row)),
            total_cost: row.total_cost,
            credits: None,
            message_count: None,
            models_used: row.models_used.clone(),
            model_breakdowns: row.model_breakdowns.clone(),
            project: None,
            versions: None,
        })
        .collect()
}

pub(super) fn all_report_title(
    kind: AgentReportKind,
    rows: &[AllRow],
    detected_agents: &[&'static str],
) -> String {
    format!(
        "Coding (Agent) CLI Usage Report - {}\nDetected: {}",
        match kind {
            AgentReportKind::Daily => "Daily",
            AgentReportKind::Weekly => "Weekly",
            AgentReportKind::Monthly => "Monthly",
            AgentReportKind::Session => "Session",
        },
        detected_agent_labels(rows, detected_agents)
    )
}

fn detected_agent_labels(rows: &[AllRow], detected_agents: &[&'static str]) -> String {
    let mut agents = BTreeSet::new();
    if detected_agents.is_empty() {
        for row in rows {
            if let Some(metadata_agents) = row.metadata_agents.as_ref() {
                agents.extend(metadata_agents.iter().copied());
            } else if row.agent != "all" {
                agents.insert(row.agent);
            }
            if let Some(breakdowns) = row.agent_breakdowns.as_ref() {
                agents.extend(breakdowns.iter().map(|breakdown| breakdown.agent));
            }
        }
    } else {
        agents.extend(detected_agents.iter().copied());
    }
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
    row: &AllRow,
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
        format_number(table_total_tokens(row)),
        format_currency(row.total_cost),
    ];
    if no_cost {
        values.pop();
    }
    values
}

fn table_total_tokens(row: &AllRow) -> u64 {
    row.input_tokens
        .saturating_add(row.output_tokens)
        .saturating_add(row.cache_creation_tokens)
        .saturating_add(row.cache_read_tokens)
}

fn push_model_breakdown_rows(
    table: &mut SimpleTable,
    breakdowns: &[ModelBreakdown],
    compact: bool,
    shared: &SharedArgs,
) {
    for b in breakdowns {
        let total =
            b.input_tokens + b.output_tokens + b.cache_creation_tokens + b.cache_read_tokens;
        let model = color(
            shared,
            format!("- {}", short_model_name(&b.model_name)),
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

pub(super) fn sort_rows(rows: &mut [AllRow], order: &SortOrder) {
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
        "kimi" => "Kimi",
        "qwen" => "Qwen",
        "grok" => "Grok",
        "zcode" => "ZCode",
        _ => agent,
    }
}
