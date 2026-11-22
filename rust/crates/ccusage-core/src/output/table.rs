use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    format_currency, format_models_multiline, format_number,
    print_estimated_pricing_notes_for_models, print_missing_pricing_warnings_for_models,
};
use crate::{
    Color, ColumnKind, Result, SimpleTable, Totals, USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary,
    cli::{AgentReportKind, SharedArgs},
    color, date_at, format_breakdown_model_label, format_project_name, parse_project_aliases,
    parse_ts_timestamp, print_box_title, resolve_tz, terminal_width,
};

pub fn should_use_compact_layout(
    shared: &SharedArgs,
    is_stdout_tty: bool,
    terminal_width: usize,
    compact_width_threshold: usize,
) -> bool {
    shared.compact || (is_stdout_tty && terminal_width < compact_width_threshold)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsageTableOptions {
    pub show_cache_creation: bool,
}

impl Default for UsageTableOptions {
    fn default() -> Self {
        Self {
            show_cache_creation: true,
        }
    }
}

pub fn print_usage_table(
    title: &str,
    first_column: &str,
    rows: &[UsageSummary],
    shared: &SharedArgs,
    group_projects: bool,
    project_aliases: Option<&str>,
) -> Result<()> {
    print_usage_table_with_options(
        title,
        first_column,
        rows,
        shared,
        group_projects,
        project_aliases,
        UsageTableOptions::default(),
    )
}

pub fn print_usage_table_with_options(
    title: &str,
    first_column: &str,
    rows: &[UsageSummary],
    shared: &SharedArgs,
    group_projects: bool,
    project_aliases: Option<&str>,
    options: UsageTableOptions,
) -> Result<()> {
    if rows.is_empty() {
        ccusage_terminal::errln!("{}", empty_usage_table_message());
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = ccusage_terminal::stdout_is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        USAGE_COMPACT_WIDTH_THRESHOLD,
    );
    let include_last_activity = rows.iter().any(|row| row.last_activity.is_some());
    let timezone = resolve_tz(shared.timezone.as_ref());
    print_box_title(title, shared)?;
    let (mut headers, mut columns) =
        usage_table_columns(first_column, compact, options.show_cache_creation);
    if shared.no_cost {
        headers.pop();
        columns.pop();
    }
    if include_last_activity {
        headers.push("Last Activity");
        columns.push(ColumnKind::Text);
    }
    let mut table = SimpleTable::new(headers, columns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);
    let aliases = parse_project_aliases(project_aliases);
    let mut current_project: Option<&str> = None;
    for row in rows {
        if group_projects
            && let Some(project) = row.project.as_deref()
            && current_project != Some(project)
        {
            if current_project.is_some() {
                table.separator();
            }
            table.push(project_header_row(
                table.column_count(),
                &format_project_name(project, &aliases),
                shared,
            ));
            current_project = Some(project);
        }
        let label = row
            .date
            .as_deref()
            .or(row.month.as_deref())
            .or(row.week.as_deref())
            .or(row.session_id.as_deref())
            .unwrap_or("");
        let models = format_models_multiline(&row.models_used);
        let total_tokens = row.total_tokens();
        let mut values = vec![
            label.to_string(),
            models,
            format_number(row.input_tokens),
            format_number(row.output_tokens),
        ];
        if !compact && options.show_cache_creation {
            values.push(format_number(row.cache_creation_tokens));
        }
        if compact {
            values.push(format_currency(row.total_cost));
        } else {
            values.extend([
                format_number(row.cache_read_tokens),
                format_number(total_tokens),
                format_currency(row.total_cost),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        if include_last_activity {
            values.push(format_last_activity_date(
                row.last_activity.as_deref().unwrap_or_default(),
                &timezone,
            ));
        }
        table.push(values);
        if shared.breakdown {
            push_breakdown_rows(
                &mut table,
                row,
                compact,
                options.show_cache_creation,
                include_last_activity,
                shared,
            );
        }
    }

    let totals = Totals::from_summaries(rows);
    let total = |value: String| color(shared, value, Color::Yellow);
    table.separator();
    let mut total_row = vec![
        total("Total".to_string()),
        String::new(),
        total(format_number(totals.input_tokens)),
        total(format_number(totals.output_tokens)),
    ];
    if !compact && options.show_cache_creation {
        total_row.push(total(format_number(totals.cache_creation_tokens)));
    }
    if compact {
        total_row.push(total(format_currency(totals.total_cost)));
    } else {
        total_row.push(total(format_number(totals.cache_read_tokens)));
        total_row.push(total(format_number(totals.total_tokens)));
        total_row.push(total(format_currency(totals.total_cost)));
    }
    if shared.no_cost {
        total_row.pop();
    }
    if include_last_activity {
        total_row.push(String::new());
    }
    table.push(total_row);
    table.print()?;
    print_estimated_pricing_notes_for_models(totals.estimated_models.iter().map(String::as_str));
    print_missing_pricing_warnings_for_models(
        totals.unpriced_models.iter().map(String::as_str),
        shared.offline,
    );
    if compact {
        ccusage_terminal::errln!("\nRunning in Compact Mode");
        ccusage_terminal::errln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

pub(super) fn usage_table_columns(
    first_column: &str,
    compact: bool,
    show_cache_creation: bool,
) -> (Vec<&str>, Vec<ColumnKind>) {
    let mut headers = vec![first_column, "Models", "Input", "Output"];
    let mut columns = vec![
        ColumnKind::Primary,
        ColumnKind::Models,
        ColumnKind::Numeric,
        ColumnKind::Numeric,
    ];
    if !compact && show_cache_creation {
        headers.push("Cache Create");
        columns.push(ColumnKind::Numeric);
    }
    if compact {
        headers.push("Cost (USD)");
        columns.push(ColumnKind::Numeric);
    } else {
        headers.extend(["Cache Read", "Total Tokens", "Cost (USD)"]);
        columns.extend([
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ]);
    }
    (headers, columns)
}

pub(super) fn empty_usage_table_message() -> &'static str {
    "No usage data found."
}

fn project_header_row(column_count: usize, project: &str, shared: &SharedArgs) -> Vec<String> {
    let mut row = vec![String::new(); column_count];
    if let Some(first) = row.first_mut() {
        *first = color(shared, format!("Project: {project}"), Color::Blue);
    }
    row
}

fn push_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    show_cache_creation: bool,
    include_last_activity: bool,
    shared: &SharedArgs,
) {
    for breakdown in &row.model_breakdowns {
        let total = breakdown.total_tokens();
        let mut values = vec![
            color(
                shared,
                format_breakdown_model_label(&breakdown.model_name),
                Color::Grey,
            ),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if !compact && show_cache_creation {
            values.push(color(
                shared,
                format_number(breakdown.cache_creation_tokens),
                Color::Grey,
            ));
        }
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        if include_last_activity {
            values.push(String::new());
        }
        table.push(values);
    }
}

pub(super) fn format_last_activity_date(value: &str, timezone: &JiffTimeZone) -> String {
    parse_ts_timestamp(value).map_or_else(
        || value.get(..10).unwrap_or(value).to_string(),
        |timestamp| date_at(timestamp, timezone).to_string(),
    )
}

pub fn first_column(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Date",
        AgentReportKind::Weekly => "Week",
        AgentReportKind::Monthly => "Month",
        AgentReportKind::Session => "Session",
    }
}
