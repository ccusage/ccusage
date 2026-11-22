use std::num::NonZeroU64;

use ccusage_core::{MILLIS_PER_MINUTE, TimestampMs};
use serde::Serialize;

use super::SessionBlock;

const BLOCKS_WARNING_THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BurnRate {
    pub(super) tokens_per_minute: f64,
    pub(crate) tokens_per_minute_for_indicator: f64,
    pub(crate) cost_per_hour: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Projection {
    pub(super) total_tokens: u64,
    pub(super) total_cost: f64,
    remaining_minutes: u64,
}

pub(crate) fn calculate_burn_rate(block: &SessionBlock) -> Option<BurnRate> {
    if block.entries.is_empty() || block.is_gap {
        return None;
    }
    let first = block.entries.first()?.timestamp;
    let last = block.entries.last()?.timestamp;
    let duration_minutes = last.duration_since(first) as f64 / MILLIS_PER_MINUTE as f64;
    if duration_minutes <= 0.0 {
        return None;
    }
    let total_tokens = block.token_counts.total() as f64;
    let non_cache = block
        .token_counts
        .input_tokens
        .saturating_add(block.token_counts.output_tokens) as f64;
    Some(BurnRate {
        tokens_per_minute: total_tokens / duration_minutes,
        tokens_per_minute_for_indicator: non_cache / duration_minutes,
        cost_per_hour: block.cost_usd / duration_minutes * 60.0,
    })
}

pub(super) fn project_block_usage(block: &SessionBlock, now: TimestampMs) -> Option<Projection> {
    if !block.is_active || block.is_gap {
        return None;
    }
    let burn = calculate_burn_rate(block)?;
    let remaining_minutes =
        (block.end_time.duration_since(now) as f64 / MILLIS_PER_MINUTE as f64).round();
    let total_tokens =
        block.token_counts.total() as f64 + burn.tokens_per_minute * remaining_minutes;
    let total_cost = block.cost_usd + (burn.cost_per_hour / 60.0) * remaining_minutes;
    Some(Projection {
        total_tokens: total_tokens.round() as u64,
        total_cost: (total_cost * 100.0).round() / 100.0,
        remaining_minutes: remaining_minutes as u64,
    })
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum LimitStatus {
    Exceeds,
    Warning,
    Ok,
}

pub(super) fn limit_status(projected: u64, limit: NonZeroU64) -> LimitStatus {
    let limit = limit.get();
    if projected > limit {
        LimitStatus::Exceeds
    } else if projected as f64 > limit as f64 * BLOCKS_WARNING_THRESHOLD {
        LimitStatus::Warning
    } else {
        LimitStatus::Ok
    }
}
