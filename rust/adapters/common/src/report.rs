use ccusage_core::cli::{AgentReportKind, SharedArgs};
use ccusage_core::{
    Color, ColumnKind, Result, SimpleTable, Totals, USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary,
    color, first_column, format_breakdown_model_label, format_currency, format_models_multiline,
    format_number, print_box_title, should_use_compact_layout, terminal_style, terminal_width,
};

fn print_table_for_agent(
    agent_name: &str,
    kind: AgentReportKind,
    rows: &[UsageSummary],
    shared: &SharedArgs,
) -> Result<()> {
    if rows.is_empty() {
        ccusage_core::errln!("No {agent_name} usage data found.");
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = ccusage_core::stdout_is_terminal();
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
    )?;
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
        let mut columns = vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ];
        if shared.no_cost {
            headers.pop();
            columns.pop();
        }
        SimpleTable::new(headers, columns, terminal_style(shared))
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
        let mut columns = vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ];
        if shared.no_cost {
            headers.pop();
            columns.pop();
        }
        SimpleTable::new(headers, columns, terminal_style(shared))
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

    let totals = Totals::from_summaries(rows);
    let total = |value: String| color(shared, value, Color::Yellow);
    let credits = format!("{:.2}", totals.credits.unwrap_or_default());
    table.separator();
    let mut row = if compact {
        vec![
            total("Total".to_string()),
            String::new(),
            total(format_number(totals.input_tokens)),
            total(format_number(totals.output_tokens)),
            total(credits),
            total(format_currency(totals.total_cost)),
        ]
    } else {
        vec![
            total("Total".to_string()),
            String::new(),
            total(format_number(totals.input_tokens)),
            total(format_number(totals.output_tokens)),
            total(format_number(totals.cache_creation_tokens)),
            total(format_number(totals.cache_read_tokens)),
            total(format_number(totals.total_tokens)),
            total(credits),
            total(format_currency(totals.total_cost)),
        ]
    };
    if shared.no_cost {
        row.pop();
    }
    table.push(row);
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

pub fn report_json(
    rows: &[UsageSummary],
    kind: AgentReportKind,
    session_metadata: bool,
) -> serde_json::Value {
    let rows_json = rows
        .iter()
        .map(|row| {
            ccusage_core::agent_summary_json(
                row,
                kind,
                session_metadata && kind == AgentReportKind::Session,
            )
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        rows_key(kind): rows_json,
        "totals": ccusage_core::totals_json(rows),
    })
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Session => "sessions",
    }
}

pub fn print_table(
    descriptor: crate::source::EntryDescriptor,
    rows: &[UsageSummary],
    kind: AgentReportKind,
    shared: &SharedArgs,
) -> Result<()> {
    use crate::source::TableStyle;
    if descriptor.table == TableStyle::Credits {
        return print_table_for_agent(descriptor.label, kind, rows, shared);
    }
    let options = table_options(descriptor, rows);
    ccusage_core::print_usage_table_with_options(
        &format!("{} Token Usage Report", descriptor.label),
        first_column(kind),
        rows,
        shared,
        false,
        None,
        options,
    )
}

pub fn table_options(
    descriptor: crate::source::EntryDescriptor,
    rows: &[UsageSummary],
) -> ccusage_core::UsageTableOptions {
    ccusage_core::UsageTableOptions {
        show_cache_creation: descriptor.table != crate::source::TableStyle::AutoCacheCreation
            || rows.iter().any(|row| row.cache_creation_tokens > 0),
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
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 4,
                    cost: 0.15,
                    missing_pricing: false,
                    stored_cost: None,
                    estimated_pricing: false,
                },
                ModelBreakdown {
                    model_name: "model-b".to_string(),
                    input_tokens: 40,
                    output_tokens: 20,
                    cache_creation_tokens: 4,
                    cache_read_tokens: 2,
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 3,
                    cost: 0.10,
                    missing_pricing: false,
                    stored_cost: None,
                    estimated_pricing: false,
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
            color: ccusage_core::cli::ColorChoice::Never,
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
            color: ccusage_core::cli::ColorChoice::Never,
            ..SharedArgs::default()
        };

        assert!(!shared.breakdown);
        assert!(enabled_breakdown_rows(&row, false, &shared).is_empty());
        assert_eq!(row.model_breakdowns.len(), 2);
    }
}
