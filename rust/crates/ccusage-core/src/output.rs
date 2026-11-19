use std::{
    collections::BTreeSet,
    io::{self, BufWriter, IsTerminal, Write},
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    Align, Color, ModelBreakdown, Result, SimpleTable, Totals, USAGE_COMPACT_WIDTH_THRESHOLD,
    UsageSummary, cli::SharedArgs, cli_error, color, format_breakdown_model_label, format_date_tz,
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
        let stdout = std::io::stdout();
        let mut stdout = BufWriter::new(stdout.lock());
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
    let timezone = resolve_tz(shared.timezone.as_deref());
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
        _ => {}
    }
}

fn format_last_activity_date(value: &str, timezone: &JiffTimeZone) -> String {
    parse_ts_timestamp(value).map_or_else(
        || value.get(..10).unwrap_or(value).to_string(),
        |timestamp| format_date_tz(timestamp, timezone),
    )
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::ModelBreakdown;

    #[test]
    fn last_activity_column_shows_the_date_in_the_report_timezone() {
        let timezone = resolve_tz(Some("America/Los_Angeles"));

        assert_eq!(
            format_last_activity_date("2026-02-24T02:00:00.000Z", &timezone),
            "2026-02-23"
        );
        assert_eq!(
            format_last_activity_date("2026-02-24", &timezone),
            "2026-02-24"
        );
    }

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
        assert!(totals.get("unpricedModels").is_none());
    }

    fn summary_with_breakdowns(breakdowns: Vec<ModelBreakdown>) -> UsageSummary {
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
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.25,
            credits: None,
            message_count: None,
            models_used: breakdowns
                .iter()
                .map(|breakdown| breakdown.model_name.clone())
                .collect(),
            model_breakdowns: breakdowns,
            project: None,
            versions: None,
        }
    }

    fn breakdown(model: &str, missing_pricing: bool) -> ModelBreakdown {
        ModelBreakdown {
            model_name: model.to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_1h_tokens: None,
            extra_total_tokens: 0,
            cost: if missing_pricing { 0.0 } else { 0.25 },
            missing_pricing,
            stored_cost: None,
        }
    }

    #[test]
    fn totals_json_lists_unpriced_models_sorted_and_deduplicated() {
        let totals = totals_json(&[
            summary_with_breakdowns(vec![
                breakdown("zeta-new-model", true),
                breakdown("gpt-5", false),
            ]),
            summary_with_breakdowns(vec![
                breakdown("alpha-new-model", true),
                breakdown("zeta-new-model", true),
            ]),
        ]);

        assert_eq!(
            totals["unpricedModels"],
            json!(["alpha-new-model", "zeta-new-model"])
        );
    }

    #[test]
    fn model_breakdown_json_marks_missing_pricing_only_when_true() {
        let priced = serde_json::to_value(breakdown("gpt-5", false)).unwrap();
        let unpriced = serde_json::to_value(breakdown("new-model", true)).unwrap();

        assert!(priced.get("missingPricing").is_none());
        assert_eq!(unpriced["missingPricing"], true);
    }

    #[test]
    fn model_breakdown_json_includes_stored_cost_only_when_present() {
        let calculated = serde_json::to_value(breakdown("gpt-5", false)).unwrap();
        let stored = serde_json::to_value(ModelBreakdown {
            stored_cost: Some(0.5),
            ..breakdown("claude-sonnet-4-20250514", false)
        })
        .unwrap();

        assert!(calculated.get("storedCost").is_none());
        assert_eq!(stored["storedCost"], 0.5);
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
                            "storedCost": 0.1,
                            "inputTokens": 100,
                            "missingPricing": true
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
                "totalCost": 0.25,
                "unpricedModels": ["gpt-5"]
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
                                "inputTokens": 100,
                                "missingPricing": true
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
                    "totalTokens": 172,
                    "unpricedModels": ["gpt-5"]
                }
            })
        );
    }

    #[test]
    fn missing_pricing_warnings_deduplicate_models() {
        assert_eq!(
            missing_pricing_warnings_for_models(
                ["gpt-5.2-codex", "claude-sonnet-4-20250514", "gpt-5.2-codex"],
                false
            ),
            vec![
                "WARN  Missing pricing for claude-sonnet-4-20250514; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
                "WARN  Missing pricing for gpt-5.2-codex; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
            ]
        );
    }

    #[test]
    fn missing_pricing_warnings_mention_embedded_pricing_offline() {
        assert_eq!(
            missing_pricing_warnings_for_models(["gpt-5.2-codex"], true),
            vec![
                "WARN  Missing embedded pricing for gpt-5.2-codex; cost excludes this model. Run without --offline or update ccusage after pricing is added.",
            ]
        );
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
    fn snapshots_model_multiline_formatting_sorts_dedupes_and_shortens_names() {
        let models = vec![
            "claude-sonnet-4-20250514".to_string(),
            "gpt-5.2-codex".to_string(),
            "claude-sonnet-4-20250514".to_string(),
            "unknown".to_string(),
        ];

        insta::assert_snapshot!(format_models_multiline(&models));
    }

    #[test]
    fn sanitizes_terminal_control_characters_as_visible_escapes() {
        assert_eq!(
            sanitize_terminal_text("future\nclient\t\u{1b}[31m"),
            r#"future\nclient\t\u{1b}[31m"#
        );
    }

    #[test]
    fn model_breakdown_total_includes_extra_tokens() {
        let breakdown = ModelBreakdown {
            model_name: "model-a".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            cache_creation_1h_tokens: None,
            extra_total_tokens: 7,
            cost: 0.25,
            missing_pricing: false,
            stored_cost: None,
        };

        assert_eq!(breakdown.total_tokens(), 172);
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
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 0,
                    cost: 0.3,
                    missing_pricing: false,
                    stored_cost: None,
                },
                ModelBreakdown {
                    model_name: "claude-sonnet-4-20250514".to_string(),
                    input_tokens: 334,
                    output_tokens: 267,
                    cache_creation_tokens: 39,
                    cache_read_tokens: 0,
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 0,
                    cost: 0.12,
                    missing_pricing: false,
                    stored_cost: None,
                },
            ],
            project: project.map(str::to_string),
            versions: Some(vec!["1.0.0".to_string(), "1.1.0".to_string()]),
        }
    }
}
