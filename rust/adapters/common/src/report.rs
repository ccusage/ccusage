use std::io::IsTerminal;

use serde_json::Value;

use ccusage_core::cli::{AgentReportKind, SharedArgs};
use ccusage_core::{
    Align, Color, Result, SimpleTable, USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary, color,
    first_column, format_currency, format_models_multiline, format_number, json_value_u64,
    print_box_title, should_use_compact_layout, terminal_style, terminal_width, totals_json,
};

pub fn print_table_for_agent(
    agent_name: &str,
    kind: AgentReportKind,
    rows: &[UsageSummary],
    shared: &SharedArgs,
) -> Result<()> {
    if rows.is_empty() {
        eprintln!("No {agent_name} usage data found.");
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = std::io::stdout().is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        USAGE_COMPACT_WIDTH_THRESHOLD,
    );
    print_box_title(
        &format!(
            "{agent_name} Token Usage Report - {}",
            agent_report_label(kind)
        ),
        shared,
    );
    let first_column = first_column(kind);
    let mut table = if compact {
        let mut headers = vec![
            first_column,
            "Models",
            "Input",
            "Output",
            "Credits",
            "Cost (USD)",
        ];
        let mut aligns = vec![
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ];
        if shared.no_cost {
            headers.pop();
            aligns.pop();
        }
        SimpleTable::new(headers, aligns, terminal_style(shared))
    } else {
        let mut headers = vec![
            first_column,
            "Models",
            "Input",
            "Output",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Credits",
            "Cost (USD)",
        ];
        let mut aligns = vec![
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ];
        if shared.no_cost {
            headers.pop();
            aligns.pop();
        }
        SimpleTable::new(headers, aligns, terminal_style(shared))
    }
    .with_terminal_width(terminal_width)
    .with_date_compaction(true);

    for row in rows {
        let label = row
            .date
            .as_deref()
            .or(row.month.as_deref())
            .or(row.week.as_deref())
            .or(row.session_id.as_deref())
            .unwrap_or("");
        let models = format_models_multiline(&row.models_used);
        if compact {
            let mut row = vec![
                label.to_string(),
                models,
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format!("{:.2}", row.credits.unwrap_or_default()),
                format_currency(row.total_cost),
            ];
            if shared.no_cost {
                row.pop();
            }
            table.push(row);
        } else {
            let mut row = vec![
                label.to_string(),
                models,
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format_number(row.cache_creation_tokens),
                format_number(row.cache_read_tokens),
                format_number(row.total_tokens()),
                format!("{:.2}", row.credits.unwrap_or_default()),
                format_currency(row.total_cost),
            ];
            if shared.no_cost {
                row.pop();
            }
            table.push(row);
        }
    }

    let totals = totals_json(rows);
    table.separator();
    let credits = totals
        .get("credits")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    if compact {
        let mut row = vec![
            color(shared, "Total", Color::Yellow),
            String::new(),
            color(
                shared,
                format_number(json_value_u64(totals.get("inputTokens"))),
                Color::Yellow,
            ),
            color(
                shared,
                format_number(json_value_u64(totals.get("outputTokens"))),
                Color::Yellow,
            ),
            color(shared, format!("{credits:.2}"), Color::Yellow),
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
            row.pop();
        }
        table.push(row);
    } else {
        let input = json_value_u64(totals.get("inputTokens"));
        let output = json_value_u64(totals.get("outputTokens"));
        let cache_create = json_value_u64(totals.get("cacheCreationTokens"));
        let cache_read = json_value_u64(totals.get("cacheReadTokens"));
        let mut row = vec![
            color(shared, "Total", Color::Yellow),
            String::new(),
            color(shared, format_number(input), Color::Yellow),
            color(shared, format_number(output), Color::Yellow),
            color(shared, format_number(cache_create), Color::Yellow),
            color(shared, format_number(cache_read), Color::Yellow),
            color(
                shared,
                format_number(json_value_u64(totals.get("totalTokens"))),
                Color::Yellow,
            ),
            color(shared, format!("{credits:.2}"), Color::Yellow),
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
            row.pop();
        }
        table.push(row);
    }
    table.print()?;
    Ok(())
}

fn agent_report_label(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Daily",
        AgentReportKind::Weekly => "Weekly",
        AgentReportKind::Monthly => "Monthly",
        AgentReportKind::Session => "Session",
    }
}
