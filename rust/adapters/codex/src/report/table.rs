use crate::{
    CodexGroup, Color, ColumnKind, PricingMap, SimpleTable, cli::SharedArgs, color,
    format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    sanitize_terminal_text, speed::CodexSpeedPolicy,
};

use super::{calculate_codex_model_cost, calculate_group_cost, non_cached_input_tokens};

pub(super) fn codex_table_columns(
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

pub(super) fn codex_table_row(
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
pub(super) struct CodexTableTotals {
    input_tokens: u64,
    cache_creation_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    cost: f64,
}

impl CodexTableTotals {
    pub(super) fn add(&mut self, group: &CodexGroup, input_tokens: u64, cost: f64) {
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

pub(super) fn push_codex_breakdown_rows(
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
    ccusage_core::sort::stable_by(&mut models, |(_, _, cost_a), (_, _, cost_b)| {
        cost_b.total_cmp(cost_a)
    });
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

pub(super) fn codex_table_total_row(
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

#[cfg(test)]
mod tests;
