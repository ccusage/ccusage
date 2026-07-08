use serde_json::{Value, json};
use std::io::IsTerminal;

use crate::{
    Align, BucketKind, Color, LoadedEntry, Result, SimpleTable, adapter::opencode,
    cli::AgentReportKind, cli::SharedArgs, cli::WeekDay, format_currency, format_models_multiline,
    format_number, json_value_u64, print_box_title, print_missing_pricing_warnings,
    short_model_name, should_use_compact_layout, summarize_by_key, summarize_summaries_by_bucket,
    totals_json,
};

pub(crate) fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| opencode::agent_summary_json(row, kind, false))
        .collect::<Vec<_>>();
    json!({
        rows_key(kind): rows_json,
        "totals": totals_json(rows),
    })
}

pub(crate) fn summarize_entries(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
) -> Result<Vec<crate::UsageSummary>> {
    match kind {
        AgentReportKind::Daily => summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        ),
        AgentReportKind::Monthly => {
            let daily = summarize_by_key(
                entries,
                |entry| entry.date.clone(),
                |date| (date.to_string(), None),
            )?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Session => summarize_by_key(
            entries,
            |entry| entry.session_id.to_string(),
            |session_id| (session_id.to_string(), None),
        )
        .map(|mut rows| {
            for row in &mut rows {
                row.session_id = row.date.take();
            }
            rows
        }),
        AgentReportKind::Weekly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Sunday,
            ))
        }
    }
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "sessions",
    }
}

pub(crate) fn print_table(
    kind: AgentReportKind,
    rows: &[crate::UsageSummary],
    shared: &SharedArgs,
) -> Result<()> {
    print_table_for_agent("Amp", kind, rows, shared)
}

pub(crate) fn print_table_for_agent(
    agent_name: &str,
    kind: AgentReportKind,
    rows: &[crate::UsageSummary],
    shared: &SharedArgs,
) -> Result<()> {
    if rows.is_empty() {
        eprintln!("No {agent_name} usage data found.");
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
    print_box_title(
        &format!(
            "{agent_name} Token Usage Report - {}",
            agent_report_label(kind)
        ),
        shared,
    );
    let first_column = opencode::first_column(kind);
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
        SimpleTable::new(headers, aligns, crate::terminal_style(shared))
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
        SimpleTable::new(headers, aligns, crate::terminal_style(shared))
    }
    .with_terminal_width(terminal_width)
    .with_date_compaction(true);

    for row in rows {
        let label = row
            .date
            .as_deref()
            .or(row.month.as_deref())
            .or(row.session_id.as_deref())
            .unwrap_or("");
        let models = format_models_multiline(&row.models_used);
        if compact {
            let mut row_cells = vec![
                label.to_string(),
                models,
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format!("{:.2}", row.credits.unwrap_or_default()),
                format_currency(row.total_cost),
            ];
            if shared.no_cost {
                row_cells.pop();
            }
            table.push(row_cells);
        } else {
            let mut row_cells = vec![
                label.to_string(),
                models,
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format_number(row.cache_creation_tokens),
                format_number(row.cache_read_tokens),
                format_number(
                    row.input_tokens
                        + row.output_tokens
                        + row.cache_creation_tokens
                        + row.cache_read_tokens,
                ),
                format!("{:.2}", row.credits.unwrap_or_default()),
                format_currency(row.total_cost),
            ];
            if shared.no_cost {
                row_cells.pop();
            }
            table.push(row_cells);
        }

        // Emit per-model breakdown sub-rows when --breakdown is set.
        if shared.breakdown {
            push_agent_breakdown_rows(&mut table, row, compact, shared);
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
            crate::color(shared, "Total", Color::Yellow),
            String::new(),
            crate::color(
                shared,
                format_number(json_value_u64(totals.get("inputTokens"))),
                Color::Yellow,
            ),
            crate::color(
                shared,
                format_number(json_value_u64(totals.get("outputTokens"))),
                Color::Yellow,
            ),
            crate::color(shared, format!("{credits:.2}"), Color::Yellow),
            crate::color(
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
            crate::color(shared, "Total", Color::Yellow),
            String::new(),
            crate::color(shared, format_number(input), Color::Yellow),
            crate::color(shared, format_number(output), Color::Yellow),
            crate::color(shared, format_number(cache_create), Color::Yellow),
            crate::color(shared, format_number(cache_read), Color::Yellow),
            crate::color(
                shared,
                format_number(input + output + cache_create + cache_read),
                Color::Yellow,
            ),
            crate::color(shared, format!("{credits:.2}"), Color::Yellow),
            crate::color(
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
    print_missing_pricing_warnings(rows, shared.offline);
    if compact {
        eprintln!("\nRunning in Compact Mode");
        eprintln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

/// Emit `└─ <model>` sub-rows for each model in `row.model_breakdowns`.
///
/// Column layout must match `print_table_for_agent` exactly:
/// - compact (with cost):    label | models | input | output | credits | cost
/// - compact (no_cost):      label | models | input | output | credits
/// - full (with cost):       label | models | input | output | cache_create | cache_read | total | credits | cost
/// - full (no_cost):         label | models | input | output | cache_create | cache_read | total | credits
///
/// Breakdown rows use the label col for the model indicator, leave the models
/// col blank, and show per-model token counts + cost. Credits is left blank
/// (agents don't track credits per-model).
fn push_agent_breakdown_rows(
    table: &mut SimpleTable,
    row: &crate::UsageSummary,
    compact: bool,
    shared: &SharedArgs,
) {
    for breakdown in &row.model_breakdowns {
        let total = breakdown.input_tokens
            + breakdown.output_tokens
            + breakdown.cache_creation_tokens
            + breakdown.cache_read_tokens;
        let mut values = if compact {
            vec![
                crate::color(
                    shared,
                    format!("  └─ {}", short_model_name(&breakdown.model_name)),
                    Color::Grey,
                ),
                String::new(), // models col — blank for sub-rows
                crate::color(shared, format_number(breakdown.input_tokens), Color::Grey),
                crate::color(shared, format_number(breakdown.output_tokens), Color::Grey),
                String::new(), // credits — not tracked per-model
                crate::color(shared, format_currency(breakdown.cost), Color::Grey),
            ]
        } else {
            vec![
                crate::color(
                    shared,
                    format!("  └─ {}", short_model_name(&breakdown.model_name)),
                    Color::Grey,
                ),
                String::new(), // models col — blank for sub-rows
                crate::color(shared, format_number(breakdown.input_tokens), Color::Grey),
                crate::color(shared, format_number(breakdown.output_tokens), Color::Grey),
                crate::color(
                    shared,
                    format_number(breakdown.cache_creation_tokens),
                    Color::Grey,
                ),
                crate::color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                crate::color(shared, format_number(total), Color::Grey),
                String::new(), // credits — not tracked per-model
                crate::color(shared, format_currency(breakdown.cost), Color::Grey),
            ]
        };
        if shared.no_cost {
            values.pop();
        }
        table.push(values);
    }
}

fn agent_report_label(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Daily",
        AgentReportKind::Weekly => "Weekly",
        AgentReportKind::Monthly => "Monthly",
        AgentReportKind::Session => "Session",
    }
}
