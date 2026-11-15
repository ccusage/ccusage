use serde_json::json;

use ccusage_adapter_common::{
    rows::UnifiedRow,
    source::{ProgressOwner, STANDARD_REPORTS, SourceRows, UnifiedRequest, UsageSource},
};
use ccusage_core::{
    ModelBreakdown, PricingMap, Result,
    cli::{AgentReportKind, CostMode},
    log_level,
    progress::{UsageLoadAgent, track_usage_load},
};

use crate::{
    CodexGroup, aggregate_events, calculate_codex_model_cost, calculate_group_cost,
    codex_model_missing_pricing, filter_events_by_date, load_codex_events_with_detection_progress,
    load_groups_with_driver, non_cached_input_tokens, resolve_codex_speed,
};

pub struct Codex;

pub static SOURCE: Codex = Codex;

impl UsageSource for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn reports(&self) -> &'static [AgentReportKind] {
        STANDARD_REPORTS
    }

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        let load = || unified_rows(request);
        match request.progress {
            ProgressOwner::Source => {
                track_usage_load(UsageLoadAgent("Codex"), request.shared.json, load)
            }
            ProgressOwner::Caller => load(),
        }
    }
}

fn unified_rows(request: &UnifiedRequest<'_>) -> Result<SourceRows> {
    let shared = request.shared;
    let owned_pricing = request.pricing.is_none().then(|| {
        PricingMap::load_with_overrides(
            shared.offline,
            log_level() != Some(0),
            shared.pricing_overrides.iter(),
        )
    });
    let pricing = request
        .pricing
        .or(owned_pricing.as_ref())
        .expect("Codex pricing is available");
    let speed = resolve_codex_speed(request.options.codex_speed);

    let (rows, detected) = if shared.since.is_none() && shared.until.is_none() {
        let groups = load_groups_with_driver(shared, request.kind, request.multi_source_driver)?;
        let detected = !groups.is_empty();
        let rows = groups
            .iter()
            .map(|(period, group)| {
                unified_row_from_group(period, group, pricing, speed, shared.mode)
            })
            .collect();
        (rows, detected)
    } else {
        let (mut events, detected) =
            load_codex_events_with_detection_progress(shared, ProgressOwner::Caller)?;
        filter_events_by_date(&mut events, shared)?;
        let groups = aggregate_events(&events, request.kind, shared.timezone.as_deref())?;
        let rows = groups
            .iter()
            .map(|(period, group)| {
                unified_row_from_group(period, group, pricing, speed, shared.mode)
            })
            .collect();
        (rows, detected)
    };

    Ok(SourceRows {
        rows,
        detected,
        warnings: Vec::new(),
        claude_counted: None,
    })
}

pub fn unified_row_from_group<S>(
    period: &str,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: S,
    mode: CostMode,
) -> UnifiedRow
where
    S: Into<crate::CodexSpeedPolicy> + Copy,
{
    let speed = speed.into();
    let mut model_breakdowns = group
        .models
        .iter()
        .map(|(model, usage)| {
            let input = non_cached_input_tokens(
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_creation_tokens,
            );
            ModelBreakdown {
                model_name: model.clone(),
                input_tokens: input,
                output_tokens: usage.output_tokens,
                cache_creation_tokens: usage.cache_creation_tokens,
                cache_read_tokens: usage.cached_input_tokens,
                cache_creation_1h_tokens: None,
                extra_total_tokens: 0,
                cost: calculate_codex_model_cost(model, usage, pricing, speed),
                missing_pricing: mode != CostMode::Display
                    && codex_model_missing_pricing(model, usage, pricing),
                stored_cost: None,
            }
        })
        .collect::<Vec<_>>();
    model_breakdowns.sort_by(|left, right| right.cost.total_cmp(&left.cost));

    UnifiedRow {
        period: period.to_string(),
        agent: "codex",
        models_used: group.models.keys().cloned().collect(),
        input_tokens: non_cached_input_tokens(
            group.input_tokens,
            group.cached_input_tokens,
            group.cache_creation_tokens,
        ),
        output_tokens: group.output_tokens,
        cache_creation_tokens: group.cache_creation_tokens,
        cache_read_tokens: group.cached_input_tokens,
        total_tokens: group.total_tokens,
        total_cost: calculate_group_cost(group, pricing, speed),
        metadata: Some(json!({
            "lastActivity": group.last_activity,
            "reasoningOutputTokens": group.reasoning_output_tokens,
        })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns,
    }
}
