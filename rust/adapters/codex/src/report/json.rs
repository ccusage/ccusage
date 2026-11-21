use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    CodexGroup, CodexModelUsage, PricingMap,
    cli::{AgentReportKind, CostMode},
    json_float,
    speed::CodexSpeedPolicy,
};

use super::{
    calculate_group_cost, codex_model_estimated, codex_model_missing_pricing,
    non_cached_input_tokens,
};

pub(super) fn rows_key(kind: AgentReportKind) -> &'static str {
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

pub(super) fn group_json(
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
    let mut models = BTreeMap::new();
    for (model, usage) in &group.models {
        let mut value = model_usage_json(usage);
        if mode != CostMode::Display && codex_model_missing_pricing(model, usage, pricing) {
            value["missingPricing"] = json!(true);
        }
        if mode != CostMode::Display && codex_model_estimated(model, pricing) {
            value["estimatedPricing"] = json!(true);
        }
        models.insert(model.clone(), value);
    }
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

pub(super) fn totals_json<'a>(
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
