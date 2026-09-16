use std::io::IsTerminal;

use serde_json::Value;

use ccusage_core::cli::{AgentReportKind, SharedArgs};
use ccusage_core::{
    Align, Color, Result, SimpleTable, USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary, color,
    first_column, format_breakdown_model_label, format_currency, format_models_multiline,
    format_number, json_value_u64, print_box_title, should_use_compact_layout, terminal_style,
    terminal_width, totals_json,
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
            let mut row_values = vec![
                label.to_string(),
                models,
                format_number(row.input_tokens),
                format_number(row.output_tokens),
                format!("{:.2}", row.credits.unwrap_or_default()),
                format_currency(row.total_cost),
            ];
            if shared.no_cost {
                row_values.pop();
            }
            table.push(row_values);
        } else {
            let mut row_values = vec![
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
                row_values.pop();
            }
            table.push(row_values);
        }
        push_breakdown_rows(&mut table, row, compact, shared);
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

fn push_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    shared: &SharedArgs,
) {
    for row in enabled_breakdown_rows(row, compact, shared) {
        table.push(row);
    }
}

fn enabled_breakdown_rows(
    row: &UsageSummary,
    compact: bool,
    shared: &SharedArgs,
) -> Vec<Vec<String>> {
    if shared.breakdown {
        breakdown_rows(row, compact, shared)
    } else {
        Vec::new()
    }
}

fn breakdown_rows(row: &UsageSummary, compact: bool, shared: &SharedArgs) -> Vec<Vec<String>> {
    let mut rows = Vec::with_capacity(row.model_breakdowns.len());
    for breakdown in &row.model_breakdowns {
        if compact {
            let mut values = vec![
                color(
                    shared,
                    format_breakdown_model_label(&breakdown.model_name),
                    Color::Grey,
                ),
                String::new(),
                color(shared, format_number(breakdown.input_tokens), Color::Grey),
                color(shared, format_number(breakdown.output_tokens), Color::Grey),
                String::new(),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ];
            if shared.no_cost {
                values.pop();
            }
            rows.push(values);
        } else {
            let mut values = vec![
                color(
                    shared,
                    format_breakdown_model_label(&breakdown.model_name),
                    Color::Grey,
                ),
                String::new(),
                color(shared, format_number(breakdown.input_tokens), Color::Grey),
                color(shared, format_number(breakdown.output_tokens), Color::Grey),
                color(
                    shared,
                    format_number(breakdown.cache_creation_tokens),
                    Color::Grey,
                ),
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(breakdown.total_tokens()), Color::Grey),
                String::new(),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ];
            if shared.no_cost {
                values.pop();
            }
            rows.push(values);
        }
    }
    rows
}

fn agent_report_label(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Daily",
        AgentReportKind::Weekly => "Weekly",
        AgentReportKind::Monthly => "Monthly",
        AgentReportKind::Session => "Session",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_core::{ModelBreakdown, cli::SharedArgs};

    fn breakdown_summary() -> UsageSummary {
        UsageSummary {
            date: Some("2026-01-02".to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            extra_total_tokens: 7,
            total_cost: 0.25,
            credits: None,
            message_count: None,
            models_used: vec!["model-a".to_string()],
            model_breakdowns: vec![
                ModelBreakdown {
                    model_name: "model-a".to_string(),
                    input_tokens: 60,
                    output_tokens: 30,
                    cache_creation_tokens: 6,
                    cache_read_tokens: 3,
                    extra_total_tokens: 4,
                    cost: 0.15,
                    missing_pricing: false,
                },
                ModelBreakdown {
                    model_name: "model-b".to_string(),
                    input_tokens: 40,
                    output_tokens: 20,
                    cache_creation_tokens: 4,
                    cache_read_tokens: 2,
                    extra_total_tokens: 3,
                    cost: 0.10,
                    missing_pricing: false,
                },
            ],
            project: None,
            versions: None,
        }
    }

    #[test]
    fn breakdown_rows_include_extra_tokens_and_match_parent_total() {
        let row = breakdown_summary();
        let shared = SharedArgs {
            no_color: true,
            ..SharedArgs::default()
        };
        let shared = SharedArgs {
            breakdown: true,
            ..shared
        };
        let rows = enabled_breakdown_rows(&row, false, &shared);

        assert_eq!(rows.len(), 2);
        assert!(rows[0][0].contains("└─ model-a"));
        assert!(rows[1][0].contains("└─ model-b"));
        assert_eq!(rows[0][6], "103");
        assert_eq!(rows[1][6], "69");

        let subtotal: u64 = row
            .model_breakdowns
            .iter()
            .map(|breakdown| breakdown.total_tokens())
            .sum();
        assert_eq!(subtotal, row.total_tokens());
        assert_eq!(row.model_breakdowns[0].total_tokens(), 103);
    }

    #[test]
    fn no_breakdown_flag_keeps_existing_row_count() {
        let row = breakdown_summary();
        let shared = SharedArgs {
            no_color: true,
            ..SharedArgs::default()
        };

        assert!(!shared.breakdown);
        assert!(enabled_breakdown_rows(&row, false, &shared).is_empty());
        assert_eq!(row.model_breakdowns.len(), 2);
    }
}
