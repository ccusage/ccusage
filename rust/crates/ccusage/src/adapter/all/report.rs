use std::{collections::BTreeSet, io::IsTerminal};

use serde_json::{json, Value};

use crate::{
    cli::{AgentReportKind, SharedArgs, SortOrder},
    color, format_currency, format_models_multiline, format_number, json_float, print_box_title,
    short_model_name, should_use_compact_layout, Align, Color, ModelBreakdown, Result, SimpleTable,
    UsageSummary,
};

use super::types::AllRow;

pub(super) fn report_json(rows: &[AllRow], kind: AgentReportKind) -> Value {
    json!({
        rows_key(kind): rows.iter().map(row_json).collect::<Vec<_>>(),
        "totals": totals_json(rows),
    })
}

fn row_json(row: &AllRow) -> Value {
    let mut value = json!({
        "period": row.period,
        "agent": row.agent,
        "modelsUsed": row.models_used,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens,
        "totalCost": json_float(row.total_cost),
        "modelBreakdowns": row.model_breakdowns,
    });
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
    value
}

fn totals_json(rows: &[AllRow]) -> Value {
    json!({
        "inputTokens": rows.iter().map(|row| row.input_tokens).sum::<u64>(),
        "outputTokens": rows.iter().map(|row| row.output_tokens).sum::<u64>(),
        "cacheCreationTokens": rows.iter().map(|row| row.cache_creation_tokens).sum::<u64>(),
        "cacheReadTokens": rows.iter().map(|row| row.cache_read_tokens).sum::<u64>(),
        "totalTokens": rows.iter().map(|row| row.total_tokens).sum::<u64>(),
        "totalCost": json_float(rows.iter().map(|row| row.total_cost).sum::<f64>()),
    })
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
    let (headers, aligns) = all_table_columns(kind, compact);
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);

    for row in rows {
        table.push(all_table_row(row, compact, false));
        if let Some(agent_breakdowns) = row.agent_breakdowns.as_ref() {
            for breakdown in agent_breakdowns {
                table.push(all_table_row(breakdown, compact, true));
                // Always expand multi-model agent rows so each model shows its
                // own tokens/cost instead of only the summed agent totals.
                if should_expand_model_breakdowns(shared, &breakdown.model_breakdowns) {
                    push_model_breakdown_rows(
                        &mut table,
                        &breakdown.model_breakdowns,
                        compact,
                        shared,
                    );
                }
            }
        } else if should_expand_model_breakdowns(shared, &row.model_breakdowns) {
            push_model_breakdown_rows(&mut table, &row.model_breakdowns, compact, shared);
        }
    }
    table.separator();
    let totals = totals_json(rows);
    let table_total_tokens = rows.iter().map(table_total_tokens).sum::<u64>();
    if compact {
        table.push(vec![
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
        ]);
    } else {
        table.push(vec![
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
        ]);
    }
    table.print()?;
    crate::print_missing_pricing_warnings(&all_rows_as_usage_summaries(rows), shared.offline);
    if compact {
        eprintln!("\nRunning in Compact Mode");
        eprintln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
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

pub(super) fn all_table_row(row: &AllRow, compact: bool, breakdown: bool) -> Vec<String> {
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
        return vec![
            period,
            agent,
            models,
            format_number(row.input_tokens),
            format_number(row.output_tokens),
            format_currency(row.total_cost),
        ];
    }

    vec![
        period,
        agent,
        models,
        format_number(row.input_tokens),
        format_number(row.output_tokens),
        format_number(row.cache_creation_tokens),
        format_number(row.cache_read_tokens),
        format_number(table_total_tokens(row)),
        format_currency(row.total_cost),
    ]
}

fn table_total_tokens(row: &AllRow) -> u64 {
    row.input_tokens
        .saturating_add(row.output_tokens)
        .saturating_add(row.cache_creation_tokens)
        .saturating_add(row.cache_read_tokens)
}

fn should_expand_model_breakdowns(shared: &SharedArgs, breakdowns: &[ModelBreakdown]) -> bool {
    !breakdowns.is_empty() && (shared.breakdown || breakdowns.len() > 1)
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
            table.push(vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(b.input_tokens), Color::Grey),
                color(shared, format_number(b.output_tokens), Color::Grey),
                color(shared, format_currency(b.cost), Color::Grey),
            ]);
        } else {
            table.push(vec![
                String::new(),
                String::new(),
                model,
                color(shared, format_number(b.input_tokens), Color::Grey),
                color(shared, format_number(b.output_tokens), Color::Grey),
                color(shared, format_number(b.cache_creation_tokens), Color::Grey),
                color(shared, format_number(b.cache_read_tokens), Color::Grey),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(b.cost), Color::Grey),
            ]);
        }
    }
}

pub(super) fn all_table_columns(
    kind: AgentReportKind,
    compact: bool,
) -> (Vec<&'static str>, Vec<Align>) {
    if compact {
        return (
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
        );
    }

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
        _ => agent,
    }
}
