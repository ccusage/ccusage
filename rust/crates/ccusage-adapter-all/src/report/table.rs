use std::collections::BTreeSet;

use super::totals;
use crate::{
    Color, ColumnKind, ModelBreakdown, Result, SimpleTable,
    cli::{AgentReportKind, SharedArgs},
    color, first_column, format_breakdown_model_label, format_currency, format_models_multiline,
    format_number, print_box_title, print_estimated_pricing_notes_for_models,
    print_missing_pricing_warnings_for_models, should_use_compact_layout,
    types::ReportRow,
};
use ccusage_adapter_common::rows::{RowUsage, SourceRow};

pub(crate) fn print_table(
    rows: &[ReportRow],
    kind: AgentReportKind,
    shared: &SharedArgs,
    detected_agents: &[&'static str],
) -> Result<()> {
    print_box_title(&all_report_title(kind, detected_agents), shared)?;
    if rows.is_empty() {
        ccusage_core::errln!("No usage data found.");
        return Ok(());
    }
    let terminal_width = crate::terminal_width();
    let is_tty = ccusage_core::stdout_is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        crate::USAGE_COMPACT_WIDTH_THRESHOLD,
    );
    let (headers, columns) = all_table_columns(kind, compact, shared.no_cost);
    let mut table = SimpleTable::new(headers, columns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);

    for row in rows {
        table.push(all_table_row(row, compact, false, shared.no_cost));
        if let Some(agent_breakdowns) = row.agent_breakdowns() {
            for breakdown in agent_breakdowns {
                table.push(source_table_row(breakdown, compact, true, shared.no_cost));
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
    print_estimated_pricing_notes_for_models(totals.estimated_models.iter().map(String::as_str));
    if compact {
        ccusage_core::errln!("\nRunning in Compact Mode");
        ccusage_core::errln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

pub(crate) fn all_report_title(kind: AgentReportKind, detected_agents: &[&'static str]) -> String {
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

pub(crate) fn all_table_row(
    row: &ReportRow,
    compact: bool,
    breakdown: bool,
    no_cost: bool,
) -> Vec<String> {
    match row {
        ReportRow::Source(source) => source_table_row(source, compact, breakdown, no_cost),
        ReportRow::Period(period) => table_row(
            &period.usage,
            "All".to_string(),
            String::new(),
            compact,
            breakdown,
            no_cost,
        ),
    }
}

pub(crate) fn source_table_row(
    row: &SourceRow,
    compact: bool,
    breakdown: bool,
    no_cost: bool,
) -> Vec<String> {
    let agent = if breakdown {
        format!("- {}", agent_label(row.agent))
    } else {
        agent_label(row.agent).to_string()
    };
    table_row(
        &row.usage,
        agent,
        format_models_multiline(&row.models_used),
        compact,
        breakdown,
        no_cost,
    )
}

fn table_row(
    row: &RowUsage,
    agent: String,
    models: String,
    compact: bool,
    breakdown: bool,
    no_cost: bool,
) -> Vec<String> {
    let period = if breakdown {
        String::new()
    } else {
        row.period.clone()
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

pub(crate) fn all_table_columns(
    kind: AgentReportKind,
    compact: bool,
    no_cost: bool,
) -> (Vec<&'static str>, Vec<ColumnKind>) {
    let (mut headers, mut columns) = if compact {
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
                ColumnKind::Primary,
                ColumnKind::Detail,
                ColumnKind::Models,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
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
                ColumnKind::Primary,
                ColumnKind::Detail,
                ColumnKind::Models,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
                ColumnKind::Numeric,
            ],
        )
    };
    if no_cost {
        headers.pop();
        columns.pop();
    }
    (headers, columns)
}

fn agent_label(agent: &str) -> &str {
    if agent == "all" {
        return "All";
    }
    ccusage_cli::AgentId::from_name(agent).map_or(agent, |id| id.spec().report_label)
}
