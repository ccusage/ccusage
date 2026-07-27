use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    Align, CodexGroup, CodexModelUsage, CodexServiceTier, CodexUsageBucket, Color, PricingMap,
    Result, SimpleTable,
    cli::{AgentReportKind, SharedArgs},
    color, format_currency, format_models_multiline, format_number, json_float,
    missing_pricing_model_for_token_total, print_box_title,
    print_missing_pricing_warnings_for_models,
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
    let input_tokens = non_cached_input_tokens(group.input_tokens, group.cached_input_tokens);
    let models = group
        .models
        .iter()
        .map(|(model, usage)| (model.clone(), model_usage_json(usage)))
        .collect::<BTreeMap<_, _>>();
    let mut row = json!({
        period_key(kind): period,
        "inputTokens": input_tokens,
        "cacheCreationTokens": 0,
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

pub fn non_cached_input_tokens(input_tokens: u64, cached_input_tokens: u64) -> u64 {
    input_tokens.saturating_sub(cached_input_tokens)
}

fn model_usage_json(usage: &CodexModelUsage) -> Value {
    json!({
        "inputTokens": non_cached_input_tokens(usage.input_tokens, usage.cached_input_tokens),
        "cacheCreationTokens": 0,
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
    let mut output = 0;
    let mut reasoning = 0;
    let mut total = 0;
    let mut cost = 0.0;
    for group in groups {
        input += non_cached_input_tokens(group.input_tokens, group.cached_input_tokens);
        cached += group.cached_input_tokens;
        output += group.output_tokens;
        reasoning += group.reasoning_output_tokens;
        total += group.total_tokens;
        cost += calculate_group_cost(group, pricing, speed);
    }
    json!({
        "inputTokens": input,
        "cacheCreationTokens": 0,
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
    let Some(pricing) = pricing.find(model) else {
        return 0.0;
    };
    let total_usage = model_usage_bucket(usage);
    let standard_cost = calculate_codex_bucket_cost(&total_usage, &pricing);
    let fast_usage = match speed.into() {
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard) => return standard_cost,
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast) => total_usage,
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard) => usage.recorded_fast_usage,
        CodexSpeedPolicy::Auto(CodexServiceTier::Fast) => {
            subtract_codex_usage_bucket(total_usage, usage.recorded_standard_usage)
        }
    };
    standard_cost
        + calculate_codex_bucket_cost(&fast_usage, &pricing) * (pricing.fast_multiplier - 1.0)
}

fn model_usage_bucket(usage: &CodexModelUsage) -> CodexUsageBucket {
    CodexUsageBucket {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        output_tokens: usage.output_tokens,
        long_context_input_tokens: usage.long_context_input_tokens,
        long_context_cached_input_tokens: usage.long_context_cached_input_tokens,
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
        output_tokens: total.output_tokens.saturating_sub(excluded.output_tokens),
        long_context_input_tokens: total
            .long_context_input_tokens
            .saturating_sub(excluded.long_context_input_tokens),
        long_context_cached_input_tokens: total
            .long_context_cached_input_tokens
            .saturating_sub(excluded.long_context_cached_input_tokens),
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
    let long_input = usage.long_context_input_tokens.min(usage.input_tokens);
    let long_cached = usage
        .long_context_cached_input_tokens
        .min(usage.cached_input_tokens)
        .min(long_input);
    let long_output = usage.long_context_output_tokens.min(usage.output_tokens);
    let short_non_cached =
        (usage.input_tokens - long_input).saturating_sub(usage.cached_input_tokens - long_cached);
    let long_non_cached = long_input - long_cached;
    short_non_cached as f64 * pricing.input
        + (usage.cached_input_tokens - long_cached) as f64 * cache_read
        + (usage.output_tokens - long_output) as f64 * pricing.output
        + long_non_cached as f64 * long_input_rate
        + long_cached as f64 * long_cache_read
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
    let mut headers = vec![
        first_column,
        "Models",
        "Input",
        "Output",
        "Reasoning",
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
    ];
    if shared.no_cost {
        headers.pop();
        aligns.pop();
    }
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(crate::terminal_width())
        .with_date_compaction(true);
    let mut total_input = 0;
    let mut total_cached = 0;
    let mut total_output = 0;
    let mut total_reasoning = 0;
    let mut total_tokens = 0;
    let mut total_cost = 0.0;
    for (label, group) in groups {
        let input_tokens = non_cached_input_tokens(group.input_tokens, group.cached_input_tokens);
        let cost = calculate_group_cost(group, pricing, speed);
        total_input += input_tokens;
        total_cached += group.cached_input_tokens;
        total_output += group.output_tokens;
        total_reasoning += group.reasoning_output_tokens;
        total_tokens += group.total_tokens;
        total_cost += cost;
        let models = format_models_multiline(&group.models.keys().cloned().collect::<Vec<_>>());
        let mut row = vec![
            label.clone(),
            models,
            format_number(input_tokens),
            format_number(group.output_tokens),
            format_number(group.reasoning_output_tokens),
            format_number(group.cached_input_tokens),
            format_number(group.total_tokens),
            format_currency(cost),
        ];
        if shared.no_cost {
            row.pop();
        }
        table.push(row);
    }
    table.separator();
    let mut total_row = vec![
        color(shared, "Total", Color::Yellow),
        String::new(),
        color(shared, format_number(total_input), Color::Yellow),
        color(shared, format_number(total_output), Color::Yellow),
        color(shared, format_number(total_reasoning), Color::Yellow),
        color(shared, format_number(total_cached), Color::Yellow),
        color(shared, format_number(total_tokens), Color::Yellow),
        color(shared, format_currency(total_cost), Color::Yellow),
    ];
    if shared.no_cost {
        total_row.pop();
    }
    table.push(total_row);
    table.print()?;
    let missing_models = codex_missing_pricing_models(groups, pricing);
    print_missing_pricing_warnings_for_models(
        missing_models.iter().map(String::as_str),
        shared.offline,
    );
    Ok(())
}
