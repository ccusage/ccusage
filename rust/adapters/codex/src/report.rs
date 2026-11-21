use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    CodexGroup, CodexModelUsage, CodexTimestampedUsage, PricingMap, Result, SimpleTable,
    attach_unpriced_models,
    cli::{AgentReportKind, CostMode, SharedArgs},
    print_box_title, print_missing_pricing_warnings_for_models, sum_costs,
};

use super::speed::CodexSpeedPolicy;

mod cost;
mod json;
mod table;

use cost::calculate_codex_usage_cost;
use json::{group_json, rows_key, totals_json};
use table::{
    CodexTableTotals, codex_table_columns, codex_table_row, codex_table_total_row,
    push_codex_breakdown_rows,
};

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

pub(super) fn calculate_codex_model_cost(
    model: &str,
    usage: &CodexModelUsage,
    pricing: Option<&PricingMap>,
    speed: CodexSpeedPolicy,
) -> f64 {
    let Some(handle) = codex_model_price(model, usage, pricing) else {
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

fn codex_model_price(
    model: &str,
    usage: &CodexModelUsage,
    pricing: Option<&PricingMap>,
) -> Option<crate::pricing::PricingHandle> {
    pricing?
        .pricing_identity(usage.priced_as.as_deref().unwrap_or(model))
        .handle
}

pub(super) fn codex_model_missing_pricing(
    model: &str,
    usage: &CodexModelUsage,
    pricing: Option<&PricingMap>,
) -> bool {
    let tokens = usage.total_tokens.max(
        usage
            .usage
            .input_tokens
            .saturating_add(usage.usage.output_tokens),
    );
    pricing.is_some() && tokens != 0 && codex_model_price(model, usage, pricing).is_none()
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
