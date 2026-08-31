use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    Align, CodexGroup, CodexModelUsage, CodexServiceTier, CodexTimestampedUsage, CodexUsageBucket,
    Color, PricingMap, Result, SimpleTable,
    cli::{AgentReportKind, SharedArgs},
    color, format_currency, format_models_multiline, format_number, json_float,
    missing_pricing_model_for_token_total, print_box_title,
    print_missing_pricing_warnings_for_models, sanitize_terminal_text,
};

use super::speed::CodexSpeedPolicy;

pub(super) fn report_from_groups(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
) -> Value {
    let rows = groups
        .iter()
        .map(|(period, group)| group_json(period, group, kind, pricing, speed))
        .collect::<Vec<_>>();
    let totals = totals_json(groups.values(), pricing, speed);
    json!({
        rows_key(kind): rows,
        "totals": totals,
    })
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "sessions",
    }
}

fn period_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "date",
        AgentReportKind::Weekly => "week",
        AgentReportKind::Monthly => "month",
        AgentReportKind::Session => "sessionId",
    }
}

fn group_json(
    period: &str,
    group: &CodexGroup,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
) -> Value {
    let cost = calculate_group_cost(group, pricing, speed);
    let input_tokens = non_cached_input_tokens(
        group.input_tokens,
        group.cached_input_tokens,
        group.cache_creation_tokens,
    );
    let models = group
        .models
        .iter()
        .map(|(model, usage)| (model.clone(), model_usage_json(usage)))
        .collect::<BTreeMap<_, _>>();
    let mut row = json!({
        period_key(kind): period,
        "inputTokens": input_tokens,
        "cacheCreationTokens": group.cache_creation_tokens,
        "cacheReadTokens": group.cached_input_tokens,
        "outputTokens": group.output_tokens,
        "reasoningOutputTokens": group.reasoning_output_tokens,
        "totalTokens": group.total_tokens,
        "costUSD": json_float(cost),
        "models": models,
    });
    if kind == AgentReportKind::Session {
        row["lastActivity"] = json!(group.last_activity);
        let separator = period.rfind('/');
        row["sessionFile"] = json!(separator.map_or(period, |index| &period[index + 1..]));
        row["directory"] = json!(separator.map_or("", |index| &period[..index]));
    }
    row
}

pub fn non_cached_input_tokens(
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
) -> u64 {
    input_tokens.saturating_sub(cached_input_tokens.saturating_add(cache_creation_tokens))
}

fn model_usage_json(usage: &CodexModelUsage) -> Value {
    json!({
        "inputTokens": non_cached_input_tokens(
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_creation_tokens,
        ),
        "cacheCreationTokens": usage.cache_creation_tokens,
        "cacheReadTokens": usage.cached_input_tokens,
        "outputTokens": usage.output_tokens,
        "reasoningOutputTokens": usage.reasoning_output_tokens,
        "totalTokens": usage.total_tokens,
        "isFallback": usage.is_fallback,
    })
}

fn totals_json<'a>(
    groups: impl Iterator<Item = &'a CodexGroup>,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
) -> Value {
    let mut input = 0;
    let mut cached = 0;
    let mut creation = 0;
    let mut output = 0;
    let mut reasoning = 0;
    let mut total = 0;
    let mut cost = 0.0;
    for group in groups {
        input += non_cached_input_tokens(
            group.input_tokens,
            group.cached_input_tokens,
            group.cache_creation_tokens,
        );
        cached += group.cached_input_tokens;
        creation += group.cache_creation_tokens;
        output += group.output_tokens;
        reasoning += group.reasoning_output_tokens;
        total += group.total_tokens;
        cost += calculate_group_cost(group, pricing, speed);
    }
    json!({
        "inputTokens": input,
        "cacheCreationTokens": creation,
        "cacheReadTokens": cached,
        "outputTokens": output,
        "reasoningOutputTokens": reasoning,
        "totalTokens": total,
        "costUSD": json_float(cost),
    })
}

pub fn calculate_codex_model_cost(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
    speed: impl Into<CodexSpeedPolicy>,
) -> f64 {
    let speed = speed.into();
    if !usage.timestamped_usage.is_empty() {
        return usage
            .timestamped_usage
            .iter()
            .filter_map(|(timestamp, timestamped_usage)| {
                let pricing =
                    pricing.find_at(model, crate::TimestampMs::from_millis(*timestamp))?;
                Some(calculate_codex_timestamped_cost(
                    timestamped_usage,
                    &pricing,
                    speed,
                ))
            })
            .sum();
    }

    let Some(pricing) = pricing.find(model) else {
        return 0.0;
    };
    calculate_codex_usage_cost(
        model_usage_bucket(usage),
        usage.recorded_standard_usage,
        usage.recorded_fast_usage,
        &pricing,
        speed,
    )
}

fn calculate_codex_timestamped_cost(
    usage: &CodexTimestampedUsage,
    pricing: &crate::Pricing,
    speed: CodexSpeedPolicy,
) -> f64 {
    calculate_codex_usage_cost(
        usage.usage,
        usage.recorded_standard_usage,
        usage.recorded_fast_usage,
        pricing,
        speed,
    )
}

fn calculate_codex_usage_cost(
    total_usage: CodexUsageBucket,
    recorded_standard_usage: CodexUsageBucket,
    recorded_fast_usage: CodexUsageBucket,
    pricing: &crate::Pricing,
    speed: CodexSpeedPolicy,
) -> f64 {
    let standard_cost = calculate_codex_bucket_cost(&total_usage, pricing);
    let fast_usage = match speed {
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard) => return standard_cost,
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast) => total_usage,
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard) => recorded_fast_usage,
        CodexSpeedPolicy::Auto(CodexServiceTier::Fast) => {
            subtract_codex_usage_bucket(total_usage, recorded_standard_usage)
        }
    };
    standard_cost
        + calculate_codex_bucket_cost(&fast_usage, pricing) * (pricing.fast_multiplier - 1.0)
}

fn model_usage_bucket(usage: &CodexModelUsage) -> CodexUsageBucket {
    CodexUsageBucket {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        output_tokens: usage.output_tokens,
        long_context_input_tokens: usage.long_context_input_tokens,
        long_context_cached_input_tokens: usage.long_context_cached_input_tokens,
        long_context_cache_creation_tokens: usage.long_context_cache_creation_tokens,
        long_context_output_tokens: usage.long_context_output_tokens,
    }
}

fn subtract_codex_usage_bucket(
    total: CodexUsageBucket,
    excluded: CodexUsageBucket,
) -> CodexUsageBucket {
    CodexUsageBucket {
        input_tokens: total.input_tokens.saturating_sub(excluded.input_tokens),
        cached_input_tokens: total
            .cached_input_tokens
            .saturating_sub(excluded.cached_input_tokens),
        cache_creation_tokens: total
            .cache_creation_tokens
            .saturating_sub(excluded.cache_creation_tokens),
        output_tokens: total.output_tokens.saturating_sub(excluded.output_tokens),
        long_context_input_tokens: total
            .long_context_input_tokens
            .saturating_sub(excluded.long_context_input_tokens),
        long_context_cached_input_tokens: total
            .long_context_cached_input_tokens
            .saturating_sub(excluded.long_context_cached_input_tokens),
        long_context_cache_creation_tokens: total
            .long_context_cache_creation_tokens
            .saturating_sub(excluded.long_context_cache_creation_tokens),
        long_context_output_tokens: total
            .long_context_output_tokens
            .saturating_sub(excluded.long_context_output_tokens),
    }
}

fn calculate_codex_bucket_cost(usage: &CodexUsageBucket, pricing: &crate::Pricing) -> f64 {
    let cache_read = if pricing.cache_read_explicit {
        pricing.cache_read
    } else {
        pricing.input
    };
    let cache_creation = pricing.cache_creation_input_token_cost();
    // OpenAI bills every token of a long-context request (input above 272K
    // tokens) at the long-context rates, so the aggregated usage is priced as
    // two independent buckets. Models without long-context rates fall back to
    // the flat rates, which keeps both buckets at the same price.
    let long_input_rate = pricing.input_above_200k.unwrap_or(pricing.input);
    let long_output_rate = pricing.output_above_200k.unwrap_or(pricing.output);
    let long_cache_read = if pricing.cache_read_explicit {
        pricing.cache_read_above_200k.unwrap_or(cache_read)
    } else {
        long_input_rate
    };
    let long_cache_creation_rate = pricing
        .cache_creation_input_token_cost_above_200k_tokens()
        .unwrap_or(cache_creation);
    let long_input = usage.long_context_input_tokens.min(usage.input_tokens);
    let long_cached = usage
        .long_context_cached_input_tokens
        .min(usage.cached_input_tokens)
        .min(long_input);
    let long_cache_creation = usage
        .long_context_cache_creation_tokens
        .min(usage.cache_creation_tokens)
        .min(long_input.saturating_sub(long_cached));
    let long_output = usage.long_context_output_tokens.min(usage.output_tokens);
    let short_cached = usage.cached_input_tokens.saturating_sub(long_cached);
    let short_cache_creation = usage
        .cache_creation_tokens
        .saturating_sub(long_cache_creation);
    let short_non_cached = usage
        .input_tokens
        .saturating_sub(long_input)
        .saturating_sub(short_cached.saturating_add(short_cache_creation));
    let long_non_cached =
        long_input.saturating_sub(long_cached.saturating_add(long_cache_creation));
    short_non_cached as f64 * pricing.input
        + short_cached as f64 * cache_read
        + short_cache_creation as f64 * cache_creation
        + (usage.output_tokens - long_output) as f64 * pricing.output
        + long_non_cached as f64 * long_input_rate
        + long_cached as f64 * long_cache_read
        + long_cache_creation as f64 * long_cache_creation_rate
        + long_output as f64 * long_output_rate
}

pub fn calculate_group_cost<S>(group: &CodexGroup, pricing: &PricingMap, speed: S) -> f64
where
    S: Into<CodexSpeedPolicy> + Copy,
{
    let speed = speed.into();
    group
        .models
        .iter()
        .map(|(model, usage)| calculate_codex_model_cost(model, usage, pricing, speed))
        .sum()
}

pub fn codex_model_missing_pricing(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
) -> bool {
    missing_pricing_model_for_token_total(
        Some(model),
        usage
            .total_tokens
            .max(usage.input_tokens.saturating_add(usage.output_tokens)),
        Some(pricing),
    )
    .is_some()
}

pub fn codex_missing_pricing_models(
    groups: &BTreeMap<String, CodexGroup>,
    pricing: &PricingMap,
) -> Vec<String> {
    let mut models = BTreeSet::new();
    for group in groups.values() {
        for (model, usage) in &group.models {
            if codex_model_missing_pricing(model, usage, pricing) {
                models.insert(model.clone());
            }
        }
    }
    models.into_iter().collect()
}

fn codex_table_columns(
    first_column: &'static str,
    no_cost: bool,
) -> (Vec<&'static str>, Vec<Align>) {
    let mut headers = vec![
        first_column,
        "Models",
        "Input",
        "Output",
        "Reasoning",
        "Cache Create",
        "Cache Read",
        "Total Tokens",
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
    if no_cost {
        headers.pop();
        aligns.pop();
    }
    (headers, aligns)
}

fn codex_table_row(
    label: &str,
    kind: AgentReportKind,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
    no_cost: bool,
    terminal_width: usize,
) -> (Vec<String>, u64, f64) {
    let input_tokens = non_cached_input_tokens(
        group.input_tokens,
        group.cached_input_tokens,
        group.cache_creation_tokens,
    );
    let cost = calculate_group_cost(group, pricing, speed);
    let models = format_models_multiline(&group.models.keys().cloned().collect::<Vec<_>>());
    let mut row = vec![
        codex_table_label(label, kind, terminal_width),
        models,
        format_number(input_tokens),
        format_number(group.output_tokens),
        format_number(group.reasoning_output_tokens),
        format_number(group.cache_creation_tokens),
        format_number(group.cached_input_tokens),
        format_number(group.total_tokens),
        format_currency(cost),
    ];
    if no_cost {
        row.pop();
    }
    (row, input_tokens, cost)
}

fn codex_table_label(label: &str, kind: AgentReportKind, terminal_width: usize) -> String {
    let label = sanitize_terminal_text(label);
    if matches!(kind, AgentReportKind::Daily) && terminal_width <= 120 {
        let bytes = label.as_bytes();
        if bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[8..10].iter().all(u8::is_ascii_digit)
        {
            return format!("{}\n{}", &label[..4], &label[5..]);
        }
    }
    label
}

#[derive(Default)]
struct CodexTableTotals {
    input_tokens: u64,
    cache_creation_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    cost: f64,
}

impl CodexTableTotals {
    fn add(&mut self, group: &CodexGroup, input_tokens: u64, cost: f64) {
        self.input_tokens += input_tokens;
        self.cache_creation_tokens += group.cache_creation_tokens;
        self.cached_input_tokens += group.cached_input_tokens;
        self.output_tokens += group.output_tokens;
        self.reasoning_output_tokens += group.reasoning_output_tokens;
        self.total_tokens += group.total_tokens;
        self.cost += cost;
    }
}

fn codex_table_total_row(
    totals: &CodexTableTotals,
    shared: &SharedArgs,
    no_cost: bool,
) -> Vec<String> {
    let mut row = vec![
        color(shared, "Total", Color::Yellow),
        String::new(),
        color(shared, format_number(totals.input_tokens), Color::Yellow),
        color(shared, format_number(totals.output_tokens), Color::Yellow),
        color(
            shared,
            format_number(totals.reasoning_output_tokens),
            Color::Yellow,
        ),
        color(
            shared,
            format_number(totals.cache_creation_tokens),
            Color::Yellow,
        ),
        color(
            shared,
            format_number(totals.cached_input_tokens),
            Color::Yellow,
        ),
        color(shared, format_number(totals.total_tokens), Color::Yellow),
        color(shared, format_currency(totals.cost), Color::Yellow),
    ];
    if no_cost {
        row.pop();
    }
    row
}

pub(super) fn print_table_from_groups(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeedPolicy,
    shared: &SharedArgs,
) -> Result<()> {
    if groups.is_empty() {
        eprintln!("No Codex usage data found.");
        return Ok(());
    }
    let first_column = match kind {
        AgentReportKind::Daily => "Date",
        AgentReportKind::Weekly => "Week",
        AgentReportKind::Monthly => "Month",
        AgentReportKind::Session => "Session",
    };
    print_box_title(
        &format!(
            "Codex Token Usage Report - {}",
            match kind {
                AgentReportKind::Daily => "Daily",
                AgentReportKind::Weekly => "Weekly",
                AgentReportKind::Monthly => "Monthly",
                AgentReportKind::Session => "Session",
            }
        ),
        shared,
    );
    let terminal_width = crate::terminal_width();
    let (headers, aligns) = codex_table_columns(first_column, shared.no_cost);
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);
    let mut totals = CodexTableTotals::default();
    for (label, group) in groups {
        let (row, input_tokens, cost) = codex_table_row(
            label,
            kind,
            group,
            pricing,
            speed,
            shared.no_cost,
            terminal_width,
        );
        totals.add(group, input_tokens, cost);
        table.push(row);
    }
    table.separator();
    table.push(codex_table_total_row(&totals, shared, shared.no_cost));
    table.print()?;
    let missing_models = codex_missing_pricing_models(groups, pricing);
    print_missing_pricing_warnings_for_models(
        missing_models.iter().map(String::as_str),
        shared.offline,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_cached_input_tokens_excludes_cache_reads_and_creation() {
        assert_eq!(non_cached_input_tokens(100, 60, 30), 10);
        assert_eq!(non_cached_input_tokens(100, 90, 20), 0);
    }

    #[test]
    fn table_rows_and_totals_render_cache_creation_tokens() {
        let group = CodexGroup {
            input_tokens: 100,
            cached_input_tokens: 60,
            cache_creation_tokens: 30,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 105,
            ..CodexGroup::default()
        };
        let shared = SharedArgs {
            no_color: true,
            ..SharedArgs::default()
        };
        let (headers, aligns) = codex_table_columns("Date", false);
        let (row, input_tokens, cost) = codex_table_row(
            "2026-08-20",
            AgentReportKind::Daily,
            &group,
            &PricingMap::default(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            false,
            160,
        );
        let mut totals = CodexTableTotals::default();
        totals.add(&group, input_tokens, cost);
        let total_row = codex_table_total_row(&totals, &shared, false);

        assert_eq!(headers[5], "Cache Create");
        assert_eq!(headers.len(), aligns.len());
        assert_eq!(row[5], "30");
        assert_eq!(total_row[5], "30");
    }

    #[test]
    fn standard_cost_table_at_120_columns_renders_distinguishable_daily_dates() {
        let group = CodexGroup {
            input_tokens: 100,
            cached_input_tokens: 60,
            cache_creation_tokens: 30,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 105,
            ..CodexGroup::default()
        };
        let (headers, aligns) = codex_table_columns("Date", false);
        let (row, _, _) = codex_table_row(
            "2026-08-20",
            AgentReportKind::Daily,
            &group,
            &PricingMap::default(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            false,
            120,
        );
        let (no_cost_headers, no_cost_aligns) = codex_table_columns("Date", true);
        let (no_cost_row, _, _) = codex_table_row(
            "2026-08-20",
            AgentReportKind::Daily,
            &group,
            &PricingMap::default(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            true,
            120,
        );

        assert_eq!(headers[5], "Cache Create");
        assert_eq!(headers.len(), aligns.len());
        assert_eq!(row[0], "2026\n08-20");
        assert_eq!(no_cost_headers.last(), Some(&"Total Tokens"));
        assert_eq!(no_cost_headers.len(), no_cost_aligns.len());
        assert_eq!(no_cost_row[0], "2026\n08-20");
    }

    #[test]
    fn no_cost_table_at_120_columns_renders_distinguishable_daily_dates() {
        let group = CodexGroup {
            input_tokens: 100,
            cached_input_tokens: 60,
            cache_creation_tokens: 30,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 105,
            ..CodexGroup::default()
        };
        let shared = SharedArgs {
            no_color: true,
            ..SharedArgs::default()
        };
        let (headers, aligns) = codex_table_columns("Date", true);
        let (first_row, first_input, first_cost) = codex_table_row(
            "2026-08-20",
            AgentReportKind::Daily,
            &group,
            &PricingMap::default(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            true,
            120,
        );
        let (second_row, second_input, second_cost) = codex_table_row(
            "2026-08-21",
            AgentReportKind::Daily,
            &group,
            &PricingMap::default(),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            true,
            120,
        );
        let mut totals = CodexTableTotals::default();
        totals.add(&group, first_input, first_cost);
        totals.add(&group, second_input, second_cost);
        let total_row = codex_table_total_row(&totals, &shared, true);

        assert_eq!(headers[5], "Cache Create");
        assert_eq!(headers.last(), Some(&"Total Tokens"));
        assert_eq!(headers.len(), aligns.len());
        assert_eq!(first_row.len(), headers.len());
        assert_eq!(second_row.len(), headers.len());
        assert_eq!(total_row.len(), headers.len());
        assert_eq!(first_row[0], "2026\n08-20");
        assert_eq!(second_row[0], "2026\n08-21");
        assert_ne!(first_row[0], second_row[0]);
        assert_eq!(first_row[5], "30");
        assert_eq!(total_row[5], "60");
    }
}
