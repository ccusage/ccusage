use std::{
    collections::BTreeSet,
    io::{BufWriter, Write},
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    Color, ColumnKind, ModelBreakdown, Result, SimpleTable, Totals, USAGE_COMPACT_WIDTH_THRESHOLD,
    UsageSummary, cli::SharedArgs, cli_error, color, date_at, format_breakdown_model_label,
    format_project_name, parse_project_aliases, parse_ts_timestamp, print_box_title, resolve_tz,
    short_model_name, terminal_width,
};

pub fn wants_json(shared: &SharedArgs) -> bool {
    shared.json || shared.jq.is_some()
}

pub fn should_use_compact_layout(
    shared: &SharedArgs,
    is_stdout_tty: bool,
    terminal_width: usize,
    compact_width_threshold: usize,
) -> bool {
    shared.compact || (is_stdout_tty && terminal_width < compact_width_threshold)
}

pub fn totals_json(rows: &[UsageSummary]) -> Value {
    Totals::from_summaries(rows).to_json()
}

impl Totals {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "inputTokens": self.input_tokens,
            "outputTokens": self.output_tokens,
            "cacheCreationTokens": self.cache_creation_tokens,
            "cacheReadTokens": self.cache_read_tokens,
            "totalTokens": self.total_tokens,
            "totalCost": self.total_cost,
        });
        if let Some(credits) = self.credits {
            value["credits"] = json!(credits);
        }
        attach_unpriced_models(&mut value, self.unpriced_models.clone());
        value
    }
}

pub(crate) fn unpriced_models<'a>(
    breakdowns: impl IntoIterator<Item = &'a ModelBreakdown>,
) -> Vec<String> {
    breakdowns
        .into_iter()
        .filter(|breakdown| breakdown.missing_pricing)
        .map(|breakdown| breakdown.model_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Adds `unpricedModels` to JSON totals when any model lacked pricing.
pub fn attach_unpriced_models(totals: &mut Value, models: Vec<String>) {
    if !models.is_empty() {
        totals["unpricedModels"] = json!(models);
    }
}

pub fn print_json_or_jq(mut value: Value, jq: Option<&str>, no_cost: bool) -> Result<()> {
    if no_cost {
        strip_cost_json(&mut value);
    }
    write_json_or_jq(&value, jq)
}

pub fn write_json_or_jq(value: &impl Serialize, jq: Option<&str>) -> Result<()> {
    if let Some(filter) = jq {
        let mut child = std::process::Command::new("jq")
            .arg(filter)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .map_err(|error| cli_error(format!("failed to run jq: {error}")))?;
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = BufWriter::new(stdin);
            serde_json::to_writer(&mut stdin, value)?;
            stdin.write_all(b"\n")?;
            stdin.flush()?;
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(cli_error("jq failed"));
        }
    } else {
        let mut stdout = BufWriter::new(ccusage_terminal::stdout());
        serde_json::to_writer_pretty(&mut stdout, value)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
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

fn usage_table_columns(
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

fn empty_usage_table_message() -> &'static str {
    "No usage data found."
}

pub fn print_missing_pricing_warnings_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
    offline: bool,
) {
    for warning in missing_pricing_warnings_for_models(models, offline) {
        ccusage_terminal::errln!("{warning}");
    }
}

fn missing_pricing_warnings_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
    offline: bool,
) -> Vec<String> {
    let models = models.into_iter().collect::<BTreeSet<_>>();

    models
        .into_iter()
        .map(|model| {
            if offline {
                format!(
                    "WARN  Missing embedded pricing for {model}; cost excludes this model. Run without --offline or update ccusage after pricing is added."
                )
            } else {
                format!(
                    "WARN  Missing pricing for {model}; cost excludes this model. Update pricing or run again after LiteLLM has the model."
                )
            }
        })
        .collect()
}

pub fn json_float(value: f64) -> Value {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        json!(value as i64)
    } else {
        json!(value)
    }
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

pub fn format_models_multiline(models: &[String]) -> String {
    let mut models = models
        .iter()
        .map(|model| short_model_name(model))
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
        .into_iter()
        .map(|model| format!("- {model}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_number(value: u64) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub fn format_currency(value: f64) -> String {
    format!("${value:.2}")
}

pub fn sanitize_terminal_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            sanitized.extend(character.escape_default());
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

pub fn strip_cost_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("totalCost");
            map.remove("costUSD");
            map.remove("cost");
            map.remove("storedCost");
            for (_, child) in map.iter_mut() {
                strip_cost_json(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_cost_json(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn format_last_activity_date(value: &str, timezone: &JiffTimeZone) -> String {
    parse_ts_timestamp(value).map_or_else(
        || value.get(..10).unwrap_or(value).to_string(),
        |timestamp| date_at(timestamp, timezone).to_string(),
    )
}

#[cfg(test)]
mod tests;
