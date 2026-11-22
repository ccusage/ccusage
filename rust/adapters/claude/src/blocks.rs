use ccusage_core::cli::SessionLength;
use std::num::NonZeroU64;

use ccusage_core::{
    LoadedEntry, ReportWindow, TimestampMs, TokenCounts, cli::SortOrder, fast::FxHashSet,
    format_rfc3339_millis, json_float, model_aliases,
};
use serde_json::{Value, json};

mod projection;
mod table;
mod time;

pub(crate) use projection::calculate_burn_rate;
use projection::{limit_status, project_block_usage};
pub(crate) use table::{print_active_block_detail, print_blocks_table};
pub(crate) use time::format_remaining_time;

#[derive(Debug, Clone)]
pub(crate) struct SessionBlock {
    id: String,
    pub(crate) start_time: TimestampMs,
    pub(crate) end_time: TimestampMs,
    actual_end_time: Option<TimestampMs>,
    pub(crate) is_active: bool,
    pub(crate) is_gap: bool,
    entries: Vec<LoadedEntry>,
    pub(crate) token_counts: TokenCounts,
    pub(crate) cost_usd: f64,
    models: Vec<String>,
    usage_limit_reset_time: Option<TimestampMs>,
}

pub(crate) fn identify_session_blocks(
    mut entries: Vec<LoadedEntry>,
    session_duration: SessionLength,
    now: TimestampMs,
) -> Vec<SessionBlock> {
    if entries.is_empty() {
        return Vec::new();
    }
    let session_duration = session_duration.as_millis();
    LoadedEntry::sort_by_timestamp(&mut entries);
    let mut blocks = Vec::new();
    let mut current_start: Option<TimestampMs> = None;
    let mut current_entries = Vec::new();

    for entry in entries {
        if let Some(start) = current_start {
            let last_time = current_entries
                .last()
                .map(|entry: &LoadedEntry| entry.timestamp)
                .unwrap_or(start);
            let since_start = entry.timestamp.duration_since(start);
            let starts_fresh = starts_fresh_block(last_time, entry.timestamp, session_duration);
            if since_start > session_duration || starts_fresh {
                blocks.push(create_block(
                    start,
                    std::mem::take(&mut current_entries),
                    now,
                    session_duration,
                ));
                if starts_fresh {
                    blocks.push(create_gap_block(
                        last_time,
                        entry.timestamp,
                        session_duration,
                    ));
                }
                current_start = Some(floor_to_hour(entry.timestamp));
            }
        } else {
            current_start = Some(floor_to_hour(entry.timestamp));
        }
        current_entries.push(entry);
    }

    if let Some(start) = current_start
        && !current_entries.is_empty()
    {
        blocks.push(create_block(start, current_entries, now, session_duration));
    }
    blocks
}

pub(crate) fn starts_fresh_block(prev: TimestampMs, next: TimestampMs, session_ms: i64) -> bool {
    next.duration_since(prev) > session_ms
}

fn floor_to_hour(timestamp: TimestampMs) -> TimestampMs {
    timestamp.floor_to_hour()
}

fn create_block(
    start: TimestampMs,
    entries: Vec<LoadedEntry>,
    now: TimestampMs,
    duration: i64,
) -> SessionBlock {
    let end = start.checked_add_millis(duration).unwrap_or(start);
    let actual_end = entries.last().map(|entry| entry.timestamp);
    let is_active = actual_end.is_some_and(|last| now.duration_since(last) < duration && now < end);
    let mut token_counts = TokenCounts::default();
    let mut cost = 0.0;
    let mut models = Vec::new();
    let mut seen_models = FxHashSet::default();
    let mut usage_limit_reset_time = None;
    for entry in &entries {
        token_counts.add_usage(entry.usage);
        cost += entry.cost;
        if let Some(model) = &entry.model {
            let model = model_aliases::resolve_model_name(model).into_owned();
            if seen_models.insert(model.clone()) {
                models.push(model);
            }
        }
        usage_limit_reset_time = usage_limit_reset_time.or(entry.usage_limit_reset_time);
    }
    SessionBlock {
        id: format_rfc3339_millis(start),
        start_time: start,
        end_time: end,
        actual_end_time: actual_end,
        is_active,
        is_gap: false,
        entries,
        token_counts,
        cost_usd: cost,
        models,
        usage_limit_reset_time,
    }
}

fn create_gap_block(last: TimestampMs, next: TimestampMs, duration: i64) -> SessionBlock {
    let start = last.checked_add_millis(duration).unwrap_or(last);
    SessionBlock {
        id: format!("gap-{}", format_rfc3339_millis(start)),
        start_time: start,
        end_time: next,
        actual_end_time: None,
        is_active: false,
        is_gap: true,
        entries: Vec::new(),
        token_counts: TokenCounts::default(),
        cost_usd: 0.0,
        models: Vec::new(),
        usage_limit_reset_time: None,
    }
}

pub(crate) fn filter_blocks_by_date(blocks: &mut Vec<SessionBlock>, window: &ReportWindow) {
    if !window.is_unbounded() {
        blocks.retain(|block| window.contains_timestamp(block.start_time));
    }
}

pub(crate) fn sort_blocks(blocks: &mut [SessionBlock], order: &SortOrder) {
    blocks.sort_by_key(|block| block.start_time);
    if *order == SortOrder::Desc {
        blocks.reverse();
    }
}

pub(crate) fn block_json(
    block: &SessionBlock,
    token_limit: Option<NonZeroU64>,
    now: TimestampMs,
) -> Value {
    let burn_rate = if block.is_active {
        calculate_burn_rate(block)
    } else {
        None
    };
    let projection = if block.is_active {
        project_block_usage(block, now)
    } else {
        None
    };
    let token_limit_status = projection.and_then(|projection| {
        let limit = token_limit?;
        let status = limit_status(projection.total_tokens, limit);
        let limit = limit.get();
        let percent = projection.total_tokens as f64 / limit as f64 * 100.0;
        Some(json!({
            "limit": limit,
            "projectedUsage": projection.total_tokens,
            "percentUsed": percent,
            "status": status,
        }))
    });
    let mut value = json!({
        "id": block.id,
        "startTime": format_rfc3339_millis(block.start_time),
        "endTime": format_rfc3339_millis(block.end_time),
        "actualEndTime": block.actual_end_time.map(format_rfc3339_millis),
        "isActive": block.is_active,
        "isGap": block.is_gap,
        "entries": block.entries.len(),
        "tokenCounts": {
            "inputTokens": block.token_counts.input_tokens,
            "outputTokens": block.token_counts.output_tokens,
            "cacheCreationInputTokens": block.token_counts.cache_creation_tokens,
            "cacheReadInputTokens": block.token_counts.cache_read_tokens,
        },
        "totalTokens": block.token_counts.total(),
        "costUSD": json_float(block.cost_usd),
        "models": block.models,
        "burnRate": burn_rate,
        "projection": projection,
    });
    if let Some(status) = token_limit_status {
        value["tokenLimitStatus"] = status;
    }
    if let Some(reset_time) = block.usage_limit_reset_time {
        value["usageLimitResetTime"] = json!(format_rfc3339_millis(reset_time));
    }
    value
}

#[cfg(test)]
mod tests;
