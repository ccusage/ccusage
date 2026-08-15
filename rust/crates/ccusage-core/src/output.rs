use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufWriter, IsTerminal, Write},
};

use serde_json::{Value, json};

use crate::{
    Align, Color, Result, SimpleTable, USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary,
    cli::SharedArgs, cli_error, color, format_project_name, parse_project_aliases, print_box_title,
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

pub fn summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
    });
    if let Some(obj) = value.as_object_mut() {
        if let Some(date) = &row.date {
            obj.insert("date".to_string(), json!(date));
        }
        if let Some(month) = &row.month {
            obj.insert("month".to_string(), json!(month));
        }
        if let Some(week) = &row.week {
            obj.insert("week".to_string(), json!(week));
        }
        if let Some(project) = &row.project {
            obj.insert("project".to_string(), json!(project));
        }
        if let Some(credits) = row.credits {
            obj.insert("credits".to_string(), json!(credits));
        }
    }
    value
}

pub fn session_summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "sessionId": row.session_id,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "lastActivity": row.last_activity,
        "firstActivity": row.first_activity,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
        "projectPath": row.project_path,
    });
    if let (Some(obj), Some(credits)) = (value.as_object_mut(), row.credits) {
        obj.insert("credits".to_string(), json!(credits));
    }
    value
}

pub fn totals_json(rows: &[UsageSummary]) -> Value {
    let input = rows.iter().map(|row| row.input_tokens).sum::<u64>();
    let output = rows.iter().map(|row| row.output_tokens).sum::<u64>();
    let cache_create = rows
        .iter()
        .map(|row| row.cache_creation_tokens)
        .sum::<u64>();
    let cache_read = rows.iter().map(|row| row.cache_read_tokens).sum::<u64>();
    let extra = rows.iter().map(|row| row.extra_total_tokens).sum::<u64>();
    let mut value = json!({
        "inputTokens": input,
        "outputTokens": output,
        "cacheCreationTokens": cache_create,
        "cacheReadTokens": cache_read,
        "totalTokens": input + output + cache_create + cache_read + extra,
        "totalCost": rows.iter().map(|row| row.total_cost).sum::<f64>(),
    });
    let credits = rows.iter().filter_map(|row| row.credits).sum::<f64>();
    if credits > 0.0 {
        value["credits"] = json!(credits);
    }
    value
}

pub fn group_project_output(rows: &[UsageSummary]) -> Value {
    let mut projects: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in rows {
        projects
            .entry(row.project.clone().unwrap_or_else(|| "unknown".to_string()))
            .or_default()
            .push(summary_json(row));
    }
    json!(projects)
}

pub fn print_json_or_jq(mut value: Value, jq: Option<&str>, no_cost: bool) -> Result<()> {
    if no_cost {
        strip_cost_json(&mut value);
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
            serde_json::to_writer(&mut stdin, &value)?;
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
        serde_json::to_writer_pretty(&mut stdout, &value)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

/// Controls optional metrics in a focused usage table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsageTableOptions {
    /// Whether a full-width table includes the cache-creation column.
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

/// Print a focused usage table with source-specific display options.
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
        eprintln!("{}", empty_usage_table_message());
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = io::stdout().is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        USAGE_COMPACT_WIDTH_THRESHOLD,
    );
    let include_last_activity = rows.iter().any(|row| row.last_activity.is_some());
    print_box_title(title, shared);
    let (mut headers, mut aligns) =
        usage_table_columns(first_column, compact, options.show_cache_creation);
    if shared.no_cost {
        headers.pop();
        aligns.pop();
    }
    if include_last_activity {
        headers.push("Last Activity");
        aligns.push(Align::Left);
    }
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
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
            values.push(truncate_rfc3339_to_date(
                row.last_activity.as_deref().unwrap_or_default(),
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

    let totals = totals_json(rows);
    let input = totals
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output = totals
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_create = totals
        .get("cacheCreationTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_read = totals
        .get("cacheReadTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total_cost = totals
        .get("totalCost")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let total_tokens = totals
        .get("totalTokens")
        .and_then(Value::as_u64)
        .unwrap_or(input + output + cache_create + cache_read);
    table.separator();
    let mut total_row = vec![
        color(shared, "Total", Color::Yellow),
        String::new(),
        color(shared, format_number(input), Color::Yellow),
        color(shared, format_number(output), Color::Yellow),
    ];
    if !compact && options.show_cache_creation {
        total_row.push(color(shared, format_number(cache_create), Color::Yellow));
    }
    if compact {
        total_row.push(color(shared, format_currency(total_cost), Color::Yellow));
    } else {
        total_row.push(color(shared, format_number(cache_read), Color::Yellow));
        total_row.push(color(shared, format_number(total_tokens), Color::Yellow));
        total_row.push(color(shared, format_currency(total_cost), Color::Yellow));
    }
    if shared.no_cost {
        total_row.pop();
    }
    if include_last_activity {
        total_row.push(String::new());
    }
    table.push(total_row);
    table.print()?;
    print_missing_pricing_warnings(rows, shared.offline);
    if compact {
        eprintln!("\nRunning in Compact Mode");
        eprintln!("Expand terminal width to see cache metrics and total tokens");
    }
    Ok(())
}

fn usage_table_columns(
    first_column: &str,
    compact: bool,
    show_cache_creation: bool,
) -> (Vec<&str>, Vec<Align>) {
    let mut headers = vec![first_column, "Models", "Input", "Output"];
    let mut aligns = vec![Align::Left, Align::Left, Align::Right, Align::Right];
    if !compact && show_cache_creation {
        headers.push("Cache Create");
        aligns.push(Align::Right);
    }
    if compact {
        headers.push("Cost (USD)");
        aligns.push(Align::Right);
    } else {
        headers.extend(["Cache Read", "Total Tokens", "Cost (USD)"]);
        aligns.extend([Align::Right, Align::Right, Align::Right]);
    }
    (headers, aligns)
}

fn empty_usage_table_message() -> &'static str {
    "No usage data found."
}

pub fn print_missing_pricing_warnings(rows: &[UsageSummary], offline: bool) {
    for warning in missing_pricing_warnings(rows, offline) {
        eprintln!("{warning}");
    }
}

fn missing_pricing_warnings(rows: &[UsageSummary], offline: bool) -> Vec<String> {
    let models = rows
        .iter()
        .flat_map(|row| &row.model_breakdowns)
        .filter(|breakdown| breakdown.missing_pricing)
        .map(|breakdown| breakdown.model_name.as_str());

    missing_pricing_warnings_for_models(models, offline)
}

pub fn print_missing_pricing_warnings_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
    offline: bool,
) {
    for warning in missing_pricing_warnings_for_models(models, offline) {
        eprintln!("{warning}");
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
        let total = breakdown.input_tokens
            + breakdown.output_tokens
            + breakdown.cache_creation_tokens
            + breakdown.cache_read_tokens;
        let mut values = vec![
            color(
                shared,
                format!("  └─ {}", short_model_name(&breakdown.model_name)),
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

pub fn strip_cost_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("totalCost");
            map.remove("costUSD");
            map.remove("cost");
            for (_, child) in map.iter_mut() {
                strip_cost_json(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_cost_json(child);
            }
        }
        _ => {}
    }
}

fn truncate_rfc3339_to_date(s: &str) -> String {
    s.get(..10).unwrap_or(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelBreakdown;

    #[test]
    fn narrow_non_tty_output_does_not_auto_compact() {
        let shared = SharedArgs::default();

        assert!(!should_use_compact_layout(&shared, false, 80, 100));
    }

    #[test]
    fn narrow_tty_output_auto_compacts() {
        let shared = SharedArgs::default();

        assert!(should_use_compact_layout(&shared, true, 80, 100));
    }

    #[test]
    fn compact_flag_forces_compact_for_non_tty_output() {
        let shared = SharedArgs {
            compact: true,
            ..SharedArgs::default()
        };

        assert!(should_use_compact_layout(&shared, false, 120, 100));
    }

    #[test]
    fn empty_usage_table_message_is_provider_agnostic() {
        assert_eq!(empty_usage_table_message(), "No usage data found.");
    }

    #[test]
    fn focused_table_can_omit_cache_creation_without_dropping_cache_reads() {
        let (headers, aligns) = usage_table_columns("Date", false, false);

        assert_eq!(
            headers,
            vec![
                "Date",
                "Models",
                "Input",
                "Output",
                "Cache Read",
                "Total Tokens",
                "Cost (USD)",
            ]
        );
        assert_eq!(headers.len(), aligns.len());
    }

    #[test]
    fn focused_table_includes_cache_creation_by_default() {
        let options = UsageTableOptions::default();
        let (headers, aligns) = usage_table_columns("Date", false, options.show_cache_creation);

        assert!(headers.contains(&"Cache Create"));
        assert_eq!(headers.len(), aligns.len());
    }

    #[test]
    fn totals_json_includes_extra_tokens_in_total() {
        let totals = totals_json(&[UsageSummary {
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
            models_used: vec!["gpt-5".to_string()],
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }]);

        assert_eq!(totals["totalTokens"], 172);
    }

    #[test]
    fn strip_cost_json_removes_cost_fields_recursively() {
        let mut value = json!({
            "daily": [
                {
                    "date": "2026-01-02",
                    "totalTokens": 172,
                    "totalCost": 0.25,
                    "modelBreakdowns": [
                        {
                            "modelName": "gpt-5",
                            "costUSD": 0.1,
                            "inputTokens": 100
                        }
                    ],
                }
            ],
            "blocks": [
                {
                    "id": "2026-01-02T00:00:00.000Z",
                    "costUSD": 1.5,
                    "burnRate": {
                        "tokensPerMinute": 10,
                        "cost": 0.25
                    },
                    "projection": {
                        "totalTokens": 300,
                        "totalCost": 2.0
                    }
                }
            ],
            "totals": {
                "totalTokens": 172,
                "totalCost": 0.25
            }
        });

        strip_cost_json(&mut value);

        assert_eq!(
            value,
            json!({
                "daily": [
                    {
                        "date": "2026-01-02",
                        "totalTokens": 172,
                        "modelBreakdowns": [
                            {
                                "modelName": "gpt-5",
                                "inputTokens": 100
                            }
                        ],
                    }
                ],
                "blocks": [
                    {
                        "id": "2026-01-02T00:00:00.000Z",
                        "burnRate": {
                            "tokensPerMinute": 10
                        },
                        "projection": {
                            "totalTokens": 300
                        }
                    }
                ],
                "totals": {
                    "totalTokens": 172
                }
            })
        );
    }

    #[test]
    fn snapshots_summary_json_with_optional_fields_and_model_breakdowns() {
        let row = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));

        insta::assert_json_snapshot!(summary_json(&row));
    }

    #[test]
    fn missing_pricing_warnings_deduplicate_models() {
        let mut row = snapshot_summary("2026-01-02", None, None);
        row.model_breakdowns[0].missing_pricing = true;
        row.model_breakdowns[1].missing_pricing = true;

        assert_eq!(
            missing_pricing_warnings(&[row], false),
            vec![
                "WARN  Missing pricing for claude-sonnet-4-20250514; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
                "WARN  Missing pricing for gpt-5.2-codex; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
            ]
        );
    }

    #[test]
    fn missing_pricing_warnings_mention_embedded_pricing_offline() {
        let mut row = snapshot_summary("2026-01-02", None, None);
        row.model_breakdowns[0].missing_pricing = true;

        assert_eq!(
            missing_pricing_warnings(&[row], true),
            vec![
                "WARN  Missing embedded pricing for gpt-5.2-codex; cost excludes this model. Run without --offline or update ccusage after pricing is added.",
            ]
        );
    }

    #[test]
    fn snapshots_session_summary_json_with_present_and_missing_options() {
        let mut row = snapshot_summary("session-a", None, None);
        row.date = None;
        row.session_id = Some("session-a".to_string());
        row.project_path = Some("/Users/example/workspace/api".to_string());
        row.last_activity = Some("2026-01-02T12:34:56.000Z".to_string());
        row.first_activity = Some("2026-01-01T10:30:00.000Z".to_string());

        insta::assert_json_snapshot!(session_summary_json(&row));
    }

    #[test]
    fn snapshots_totals_json_with_extra_tokens_credits_and_zero_credit_omission() {
        let mut first = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));
        first.extra_total_tokens = 17;
        let mut second = snapshot_summary("2026-01-03", Some("workspace/web"), None);
        second.input_tokens = 0;
        second.output_tokens = 0;
        second.cache_creation_tokens = 0;
        second.cache_read_tokens = 0;
        second.extra_total_tokens = 3;
        second.total_cost = 0.0;

        insta::assert_json_snapshot!(totals_json(&[first, second]));
    }

    #[test]
    fn snapshots_group_project_output_orders_named_projects_and_unknown_bucket() {
        let named = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));
        let unknown = snapshot_summary("2026-01-03", None, None);
        let other = snapshot_summary("2026-01-04", Some("workspace/web"), None);

        insta::assert_json_snapshot!(group_project_output(&[named, unknown, other]));
    }

    #[test]
    fn snapshots_model_multiline_formatting_sorts_dedupes_and_shortens_names() {
        let models = vec![
            "claude-sonnet-4-20250514".to_string(),
            "gpt-5.2-codex".to_string(),
            "claude-sonnet-4-20250514".to_string(),
            "unknown".to_string(),
        ];

        insta::assert_snapshot!(format_models_multiline(&models));
    }

    fn snapshot_summary(period: &str, project: Option<&str>, credits: Option<f64>) -> UsageSummary {
        UsageSummary {
            date: Some(period.to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 1_234,
            output_tokens: 567,
            cache_creation_tokens: 89,
            cache_read_tokens: 10,
            extra_total_tokens: 0,
            total_cost: 0.42,
            credits,
            message_count: Some(7),
            models_used: vec![
                "gpt-5.2-codex".to_string(),
                "claude-sonnet-4-20250514".to_string(),
            ],
            model_breakdowns: vec![
                ModelBreakdown {
                    model_name: "gpt-5.2-codex".to_string(),
                    input_tokens: 900,
                    output_tokens: 300,
                    cache_creation_tokens: 50,
                    cache_read_tokens: 10,
                    extra_total_tokens: 0,
                    cost: 0.3,
                    missing_pricing: false,
                },
                ModelBreakdown {
                    model_name: "claude-sonnet-4-20250514".to_string(),
                    input_tokens: 334,
                    output_tokens: 267,
                    cache_creation_tokens: 39,
                    cache_read_tokens: 0,
                    extra_total_tokens: 0,
                    cost: 0.12,
                    missing_pricing: false,
                },
            ],
            project: project.map(str::to_string),
            versions: Some(vec!["1.0.0".to_string(), "1.1.0".to_string()]),
        }
    }
}
