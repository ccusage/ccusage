use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    CodexGroup, CodexModelUsage, CodexServiceTier, CodexTimestampedUsage, CodexUsageBucket, Color,
    ColumnKind, PricingMap, Result, SimpleTable, attach_unpriced_models,
    cli::{AgentReportKind, CostMode, SharedArgs},
    color, format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    json_float, missing_pricing_model_for_token_total, print_box_title,
    print_missing_pricing_warnings_for_models, sanitize_terminal_text, sum_costs,
};

use super::speed::CodexSpeedPolicy;

pub(super) fn report_from_groups(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    mode: CostMode,
) -> Value {
    let pricing = pricing.filter(|_| mode != CostMode::Display);
    let rows = groups
        .iter()
        .map(|(period, group)| group_json(period, group, kind, pricing, speed, mode))
        .collect::<Vec<_>>();
    let mut totals = totals_json(groups.values(), pricing, speed);
    if mode != CostMode::Display {
        attach_unpriced_models(&mut totals, codex_missing_pricing_models(groups, pricing));
    }
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
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    mode: CostMode,
) -> Value {
    let pricing = pricing.filter(|_| mode != CostMode::Display);
    let cost = calculate_group_cost(group, pricing, speed);
    let input_tokens = non_cached_input_tokens(
        group.input_tokens,
        group.cached_input_tokens,
        group.cache_creation_tokens,
    );
    let models = group
        .models
        .iter()
        .map(|(model, usage)| {
            let mut value = model_usage_json(usage);
            if mode != CostMode::Display && codex_model_missing_pricing(model, usage, pricing) {
                value["missingPricing"] = json!(true);
            }
            (model.clone(), value)
        })
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

pub(super) fn session_detail_json(
    session_id: &str,
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    mode: CostMode,
) -> Value {
    let mut row = group_json(
        session_id,
        group,
        AgentReportKind::Session,
        pricing,
        speed,
        mode,
    );
    if let Some(object) = row.as_object_mut()
        && let Some(cost) = object.remove("costUSD")
    {
        object.insert("totalCost".to_string(), cost);
    }
    row
}

pub(super) fn non_cached_input_tokens(
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
) -> u64 {
    input_tokens.saturating_sub(cached_input_tokens.saturating_add(cache_creation_tokens))
}

fn model_usage_json(usage: &CodexModelUsage) -> Value {
    json!({
        "inputTokens": non_cached_input_tokens(
            usage.usage.input_tokens,
            usage.usage.cached_input_tokens,
            usage.usage.cache_creation_tokens,
        ),
        "cacheCreationTokens": usage.usage.cache_creation_tokens,
        "cacheReadTokens": usage.usage.cached_input_tokens,
        "outputTokens": usage.usage.output_tokens,
        "reasoningOutputTokens": usage.reasoning_output_tokens,
        "totalTokens": usage.total_tokens,
        "isFallback": usage.is_fallback,
    })
}

fn totals_json<'a>(
    groups: impl Iterator<Item = &'a CodexGroup>,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
) -> Value {
    let mut input = 0u64;
    let mut cached = 0u64;
    let mut creation = 0u64;
    let mut output = 0u64;
    let mut reasoning = 0u64;
    let mut total = 0u64;
    let mut cost = 0.0;
    for group in groups {
        input = input.saturating_add(non_cached_input_tokens(
            group.input_tokens,
            group.cached_input_tokens,
            group.cache_creation_tokens,
        ));
        cached = cached.saturating_add(group.cached_input_tokens);
        creation = creation.saturating_add(group.cache_creation_tokens);
        output = output.saturating_add(group.output_tokens);
        reasoning = reasoning.saturating_add(group.reasoning_output_tokens);
        total = total.saturating_add(group.total_tokens);
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

pub(super) fn calculate_codex_model_cost(
    model: &str,
    usage: &CodexModelUsage,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
) -> f64 {
    let Some(handle) = pricing.and_then(|table| table.find_handle(model)) else {
        return 0.0;
    };
    if !usage.timestamped_usage.is_empty() {
        return sum_costs(
            usage
                .timestamped_usage
                .iter()
                .map(|(timestamp, timestamped_usage)| {
                    handle
                        .with_rates_at(Some(crate::TimestampMs::from_millis(*timestamp)), |rates| {
                            calculate_codex_usage_cost(*timestamped_usage, rates, speed)
                        })
                }),
        );
    }
    handle.with_rates_at(None, |rates| {
        calculate_codex_usage_cost(
            CodexTimestampedUsage {
                usage: usage.usage,
                recorded_usage: usage.recorded_usage,
            },
            rates,
            speed,
        )
    })
}

fn calculate_codex_usage_cost(
    usage: CodexTimestampedUsage,
    pricing: &crate::Pricing,
    speed: CodexSpeedPolicy,
) -> f64 {
    let CodexTimestampedUsage {
        usage: total_usage,
        recorded_usage,
    } = usage;
    let recorded_flex_usage = recorded_usage[CodexServiceTier::Flex];
    let recorded_fast_usage = recorded_usage[CodexServiceTier::Fast];
    let recorded_ultrafast_usage = recorded_usage[CodexServiceTier::Ultrafast];
    let standard_cost = calculate_codex_bucket_cost(&total_usage, pricing);
    let unclassified_usage = total_usage.saturating_sub(recorded_usage.total());
    let empty = CodexUsageBucket::default();
    let (flex_usage, fast_usage, ultrafast_usage) = match speed {
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard) => {
            return standard_cost;
        }
        CodexSpeedPolicy::Forced(CodexServiceTier::Flex) => (total_usage, empty, empty),
        CodexSpeedPolicy::Forced(CodexServiceTier::Fast) => (empty, total_usage, empty),
        CodexSpeedPolicy::Forced(CodexServiceTier::Ultrafast) => (empty, empty, total_usage),
        CodexSpeedPolicy::Auto(CodexServiceTier::Standard) => (
            recorded_flex_usage,
            recorded_fast_usage,
            recorded_ultrafast_usage,
        ),
        CodexSpeedPolicy::Auto(CodexServiceTier::Flex) => (
            recorded_flex_usage + unclassified_usage,
            recorded_fast_usage,
            recorded_ultrafast_usage,
        ),
        CodexSpeedPolicy::Auto(CodexServiceTier::Fast) => (
            recorded_flex_usage,
            recorded_fast_usage + unclassified_usage,
            recorded_ultrafast_usage,
        ),
        CodexSpeedPolicy::Auto(CodexServiceTier::Ultrafast) => (
            recorded_flex_usage,
            recorded_fast_usage,
            recorded_ultrafast_usage + unclassified_usage,
        ),
    };
    standard_cost
        + calculate_codex_bucket_cost(&flex_usage, pricing) * (pricing.flex_multiplier - 1.0)
        + calculate_codex_bucket_cost(&fast_usage, pricing) * (pricing.fast_multiplier - 1.0)
        + calculate_codex_bucket_cost(&ultrafast_usage, pricing)
            * (pricing.ultrafast_multiplier - 1.0)
}

fn calculate_codex_bucket_cost(usage: &CodexUsageBucket, pricing: &crate::Pricing) -> f64 {
    let cache_read = pricing.cache_read_rate_at(pricing.input);
    let cache_creation = pricing.cache_creation_rate();
    let rates = pricing
        .long_context
        .map(|context| context.rates)
        .unwrap_or_default();
    let long_input_rate = rates.input.unwrap_or(pricing.input);
    let long_output_rate = rates.output.unwrap_or(pricing.output);
    let long_cache_read = rates
        .cache_read
        .unwrap_or_else(|| pricing.cache_read_rate_at(long_input_rate));
    let long_cache_creation_rate = rates.cache_create.unwrap_or(cache_creation);
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

pub fn calculate_group_cost(
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
) -> f64 {
    sum_costs(
        group
            .models
            .iter()
            .map(|(model, usage)| calculate_codex_model_cost(model, usage, pricing, speed)),
    )
}

pub(super) fn codex_model_missing_pricing(
    model: &str,
    usage: &CodexModelUsage,
    pricing: Option<&PricingMap>,
) -> bool {
    missing_pricing_model_for_token_total(
        Some(model),
        usage.total_tokens.max(
            usage
                .usage
                .input_tokens
                .saturating_add(usage.usage.output_tokens),
        ),
        pricing,
    )
    .is_some()
}

pub(crate) fn codex_missing_pricing_models(
    groups: &BTreeMap<String, CodexGroup>,
    pricing: Option<&PricingMap>,
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
) -> (Vec<&'static str>, Vec<ColumnKind>) {
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
    if no_cost {
        headers.pop();
        columns.pop();
    }
    (headers, columns)
}

fn codex_table_row(
    label: &str,
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    no_cost: bool,
) -> (Vec<String>, u64, f64) {
    let input_tokens = non_cached_input_tokens(
        group.input_tokens,
        group.cached_input_tokens,
        group.cache_creation_tokens,
    );
    let cost = calculate_group_cost(group, pricing, speed);
    let models = format_models_multiline(&group.models.keys().cloned().collect::<Vec<_>>());
    let mut row = vec![
        sanitize_terminal_text(label),
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
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(group.cache_creation_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(group.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(group.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(group.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(group.total_tokens);
        self.cost += cost;
    }
}

fn push_codex_breakdown_rows(
    table: &mut SimpleTable,
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    shared: &SharedArgs,
) {
    for row in enabled_codex_breakdown_rows(group, pricing, speed, shared) {
        table.push(row);
    }
}

fn enabled_codex_breakdown_rows(
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    shared: &SharedArgs,
) -> Vec<Vec<String>> {
    if shared.breakdown {
        codex_breakdown_rows(group, pricing, speed, shared)
    } else {
        Vec::new()
    }
}

fn codex_breakdown_rows(
    group: &CodexGroup,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    shared: &SharedArgs,
) -> Vec<Vec<String>> {
    let mut models = group
        .models
        .iter()
        .map(|(model, usage)| {
            (
                model,
                usage,
                calculate_codex_model_cost(model, usage, pricing, speed),
            )
        })
        .collect::<Vec<_>>();
    models.sort_by(|(_, _, cost_a), (_, _, cost_b)| cost_b.total_cmp(cost_a));
    let mut rows = Vec::with_capacity(models.len());
    for (model, usage, cost) in models {
        let input_tokens = non_cached_input_tokens(
            usage.usage.input_tokens,
            usage.usage.cached_input_tokens,
            usage.usage.cache_creation_tokens,
        );
        let mut row = vec![
            color(shared, format_breakdown_model_label(model), Color::Grey),
            String::new(),
            color(shared, format_number(input_tokens), Color::Grey),
            color(
                shared,
                format_number(usage.usage.output_tokens),
                Color::Grey,
            ),
            color(
                shared,
                format_number(usage.reasoning_output_tokens),
                Color::Grey,
            ),
            color(
                shared,
                format_number(usage.usage.cache_creation_tokens),
                Color::Grey,
            ),
            color(
                shared,
                format_number(usage.usage.cached_input_tokens),
                Color::Grey,
            ),
            color(shared, format_number(usage.total_tokens), Color::Grey),
            color(shared, format_currency(cost), Color::Grey),
        ];
        if shared.no_cost {
            row.pop();
        }
        rows.push(row);
    }
    rows
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
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
    shared: &SharedArgs,
) -> Result<()> {
    let pricing = pricing.filter(|_| shared.mode != CostMode::Display);
    if groups.is_empty() {
        ccusage_core::errln!("No Codex usage data found.");
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
    )?;
    let terminal_width = crate::terminal_width();
    let (headers, columns) = codex_table_columns(first_column, shared.no_cost);
    let mut table = SimpleTable::new(headers, columns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width)
        .with_date_compaction(true);
    if kind == AgentReportKind::Daily {
        table = table.with_date_compaction_at(120);
    }
    let mut totals = CodexTableTotals::default();
    for (label, group) in groups {
        let (row, input_tokens, cost) =
            codex_table_row(label, group, pricing, speed, shared.no_cost);
        totals.add(group, input_tokens, cost);
        table.push(row);
        push_codex_breakdown_rows(&mut table, group, pricing, speed, shared);
    }
    table.separator();
    table.push(codex_table_total_row(&totals, shared, shared.no_cost));
    table.print()?;
    if shared.mode != CostMode::Display {
        let missing_models = codex_missing_pricing_models(groups, pricing);
        print_missing_pricing_warnings_for_models(
            missing_models.iter().map(String::as_str),
            shared.offline,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
