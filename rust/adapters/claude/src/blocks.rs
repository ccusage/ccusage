use ccusage_core::cli::SessionLength;
use std::num::NonZeroU64;

use ccusage_core::{
    Align, Color, ColumnKind, LoadedEntry, MILLIS_PER_HOUR, MILLIS_PER_MINUTE, ReportWindow,
    Result, SimpleTable, TimestampMs, TokenCounts,
    cli::{SharedArgs, SortOrder},
    color,
    fast::FxHashSet,
    format_currency, format_models_multiline, format_number, format_rfc3339_millis, json_float,
    model_aliases, outln, print_box_title, should_use_compact_layout, terminal_style,
    terminal_width,
};
use ccusage_report::{am_pm, hour_12, zoned_parts};
use jiff::tz::TimeZone;
use serde::Serialize;
use serde_json::{Value, json};

const BLOCKS_WARNING_THRESHOLD: f64 = 0.8;
const BLOCKS_COMPACT_WIDTH_THRESHOLD: usize = 120;

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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BurnRate {
    tokens_per_minute: f64,
    pub(crate) tokens_per_minute_for_indicator: f64,
    pub(crate) cost_per_hour: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Projection {
    total_tokens: u64,
    total_cost: f64,
    remaining_minutes: u64,
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
    entries.sort_by_key(|entry| entry.timestamp);
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

fn format_block_models(models: &[String]) -> String {
    if models.is_empty() {
        "-".to_string()
    } else {
        format_models_multiline(models)
    }
}

fn format_block_time(
    block: &SessionBlock,
    compact: bool,
    now: TimestampMs,
    zone: &TimeZone,
) -> Result<String> {
    let start = format_block_start(block.start_time, compact, zone)?;
    if block.is_gap {
        let end = format_block_end(block.end_time, compact, zone)?;
        let duration = block.end_time.duration_since(block.start_time) / MILLIS_PER_HOUR;
        return Ok(if compact {
            format!("{start}-{end}\n({duration}h gap)")
        } else {
            format!("{start} - {end} ({duration}h gap)")
        });
    }

    if block.is_active {
        let elapsed = now.duration_since(block.start_time) / MILLIS_PER_MINUTE;
        let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
        let elapsed_hours = elapsed / 60;
        let elapsed_minutes = elapsed.rem_euclid(60);
        let remaining_hours = remaining / 60;
        let remaining_minutes = remaining.rem_euclid(60);
        return Ok(if compact {
            format!(
                "{start}\n({elapsed_hours}h{elapsed_minutes}m/{remaining_hours}h{remaining_minutes}m)"
            )
        } else {
            format!(
                "{start} ({elapsed_hours}h {elapsed_minutes}m elapsed, {remaining_hours}h {remaining_minutes}m remaining)"
            )
        });
    }

    let duration = block
        .actual_end_time
        .map(|end| end.duration_since(block.start_time) / MILLIS_PER_MINUTE)
        .unwrap_or(0);
    let hours = duration / 60;
    let minutes = duration.rem_euclid(60);
    Ok(if compact {
        if hours > 0 {
            format!("{start}\n({hours}h{minutes}m)")
        } else {
            format!("{start}\n({minutes}m)")
        }
    } else if hours > 0 {
        format!("{start} ({hours}h {minutes}m)")
    } else {
        format!("{start} ({minutes}m)")
    })
}

fn format_block_start(timestamp: TimestampMs, compact: bool, zone: &TimeZone) -> Result<String> {
    let parts = zoned_parts(timestamp, zone)?;
    Ok(if compact {
        format!(
            "{:02}/{:02}, {:02}:{:02} {}",
            parts.month,
            parts.day,
            hour_12(parts.hour),
            parts.minute,
            am_pm(parts.hour)
        )
    } else {
        format!(
            "{}/{}/{}, {}:{:02}:{:02} {}",
            parts.month,
            parts.day,
            parts.year,
            hour_12(parts.hour),
            parts.minute,
            parts.second,
            am_pm(parts.hour)
        )
    })
}

fn format_block_end(timestamp: TimestampMs, compact: bool, zone: &TimeZone) -> Result<String> {
    if compact {
        let parts = zoned_parts(timestamp, zone)?;
        Ok(format!(
            "{:02}:{:02} {}",
            hour_12(parts.hour),
            parts.minute,
            am_pm(parts.hour)
        ))
    } else {
        format_block_start(timestamp, false, zone)
    }
}

fn format_block_started(timestamp: TimestampMs, zone: &TimeZone) -> Result<String> {
    let parts = zoned_parts(timestamp, zone)?;
    Ok(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    ))
}

pub(crate) fn print_blocks_table(
    blocks: &[SessionBlock],
    token_limit: Option<NonZeroU64>,
    shared: &SharedArgs,
    now: TimestampMs,
    zone: &TimeZone,
) -> Result<()> {
    if blocks.is_empty() {
        ccusage_core::errln!("No Claude usage data found.");
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = ccusage_core::stdout_is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        BLOCKS_COMPACT_WIDTH_THRESHOLD,
    );
    let actual_limit = token_limit;
    print_box_title("Claude Code Token Usage Report - Session Blocks", shared)?;
    let mut headers = vec!["Block Start", "Duration/Status", "Models", "Tokens"];
    let mut columns = vec![
        ColumnKind::Primary,
        ColumnKind::Detail,
        ColumnKind::Models,
        ColumnKind::Numeric,
    ];
    if actual_limit.is_some() {
        headers.push("%");
        columns.push(ColumnKind::Numeric);
    }
    if !shared.no_cost {
        headers.push("Cost");
        columns.push(ColumnKind::Numeric);
    }
    let mut table = SimpleTable::new(headers, columns, terminal_style(shared))
        .with_terminal_width(terminal_width);
    for block in blocks {
        if block.is_gap {
            let mut row = vec![
                color(
                    shared,
                    format_block_time(block, compact, now, zone)?,
                    Color::Grey,
                ),
                color(shared, "(inactive)", Color::Grey),
                color(shared, "-", Color::Grey),
                color(shared, "-", Color::Grey),
            ];
            if actual_limit.is_some() {
                row.push(color(shared, "-", Color::Grey));
            }
            if !shared.no_cost {
                row.push(color(shared, "-", Color::Grey));
            }
            table.push(row);
            continue;
        }
        let total = block.token_counts.total();
        let mut row = vec![
            format_block_time(block, compact, now, zone)?,
            if block.is_active {
                color(shared, "ACTIVE", Color::Green)
            } else {
                String::new()
            },
            format_block_models(&block.models),
            format_number(total),
        ];
        if let Some(limit) = actual_limit {
            let percentage = total as f64 / limit.get() as f64 * 100.0;
            let percent_text = format!("{percentage:.1}%");
            row.push(if limit_status(total, limit) == LimitStatus::Exceeds {
                color(shared, percent_text, Color::Red)
            } else {
                percent_text
            });
        }
        if !shared.no_cost {
            row.push(format_currency(block.cost_usd));
        }
        table.push(row);

        if block.is_active {
            if let Some(limit) = actual_limit {
                table.separator();
                let remaining = limit.get().saturating_sub(total);
                let remaining_percent =
                    (limit.get().saturating_sub(total) as f64 / limit.get() as f64) * 100.0;
                let mut remaining_row = vec![
                    color(
                        shared,
                        format!("(assuming {} token limit)", format_number(limit.get())),
                        Color::Grey,
                    ),
                    color(shared, "REMAINING", Color::Blue),
                    String::new(),
                    if remaining > 0 {
                        format_number(remaining)
                    } else {
                        color(shared, "0", Color::Red)
                    },
                ];
                remaining_row.push(if remaining_percent > 0.0 {
                    format!("{remaining_percent:.1}%")
                } else {
                    color(shared, "0.0%", Color::Red)
                });
                if !shared.no_cost {
                    remaining_row.push(String::new());
                }
                table.push_aligned(remaining_row, Align::Right);
            }

            if let Some(projection) = project_block_usage(block, now) {
                table.separator();
                let mut projected_row = vec![
                    color(shared, "(assuming current burn rate)", Color::Grey),
                    color(shared, "PROJECTED", Color::Yellow),
                    String::new(),
                    match actual_limit {
                        Some(limit)
                            if limit_status(projection.total_tokens, limit)
                                == LimitStatus::Exceeds =>
                        {
                            color(shared, format_number(projection.total_tokens), Color::Red)
                        }
                        _ => format_number(projection.total_tokens),
                    },
                ];
                if let Some(limit) = actual_limit {
                    let percentage = projection.total_tokens as f64 / limit.get() as f64 * 100.0;
                    projected_row.push(format!("{percentage:.1}%"));
                }
                if !shared.no_cost {
                    projected_row.push(format_currency(projection.total_cost));
                }
                table.push_aligned(projected_row, Align::Right);
            }
        }
    }
    table.print()?;
    Ok(())
}

pub(crate) fn print_active_block_detail(
    block: &SessionBlock,
    token_limit: Option<NonZeroU64>,
    shared: &SharedArgs,
    now: TimestampMs,
    zone: &TimeZone,
) -> Result<()> {
    print_box_title("Current Session Block Status", shared)?;
    let elapsed = now.duration_since(block.start_time) / MILLIS_PER_MINUTE;
    let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
    outln!(
        "Block Started:   {}",
        format_block_started(block.start_time, zone)?
    )?;
    outln!(
        "Time Elapsed:    {}h {}m",
        elapsed / 60,
        elapsed.rem_euclid(60)
    )?;
    outln!(
        "Time Remaining:  {}",
        color(
            shared,
            format!("{}h {}m", remaining / 60, remaining.rem_euclid(60)),
            Color::Green,
        )
    )?;
    outln!()?;
    outln!("{}", color(shared, "Current Usage:", Color::Blue))?;
    outln!(
        "  Input Tokens:     {}",
        format_number(block.token_counts.input_tokens)
    )?;
    outln!(
        "  Output Tokens:    {}",
        format_number(block.token_counts.output_tokens)
    )?;
    if !shared.no_cost {
        outln!("  Total Cost:       {}", format_currency(block.cost_usd))?;
    }

    if let Some(rate) = calculate_burn_rate(block) {
        outln!()?;
        outln!("{}", color(shared, "Burn Rate:", Color::Blue))?;
        outln!(
            "  Tokens/minute:    {}",
            format_number(rate.tokens_per_minute.round() as u64)
        )?;
        if !shared.no_cost {
            outln!(
                "  Cost/hour:        {}",
                format_currency(rate.cost_per_hour)
            )?;
        }
    }

    if let Some(projection) = project_block_usage(block, now) {
        outln!()?;
        outln!(
            "{}",
            color(
                shared,
                "Projected Usage (if current rate continues):",
                Color::Blue
            )
        )?;
        outln!(
            "  Total Tokens:     {}",
            format_number(projection.total_tokens)
        )?;
        if !shared.no_cost {
            outln!(
                "  Total Cost:       {}",
                format_currency(projection.total_cost)
            )?;
        }

        if let Some(limit) = token_limit {
            let status = match limit_status(projection.total_tokens, limit) {
                LimitStatus::Exceeds => color(shared, "EXCEEDS LIMIT", Color::Red),
                LimitStatus::Warning => color(shared, "WARNING", Color::Yellow),
                LimitStatus::Ok => color(shared, "OK", Color::Green),
            };
            let limit = limit.get();
            let current = block.token_counts.total();
            let remaining_tokens = limit.saturating_sub(current);
            let percent = projection.total_tokens as f64 / limit as f64 * 100.0;
            outln!()?;
            outln!("{}", color(shared, "Token Limit Status:", Color::Blue))?;
            outln!("  Limit:            {} tokens", format_number(limit))?;
            outln!(
                "  Current Usage:    {} ({:.1}%)",
                format_number(current),
                current as f64 / limit as f64 * 100.0
            )?;
            outln!(
                "  Remaining:        {} tokens",
                format_number(remaining_tokens)
            )?;
            outln!("  Projected Usage:  {percent:.1}% {status}")?;
        }
    }
    Ok(())
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
    let non_cache = (block.token_counts.input_tokens + block.token_counts.output_tokens) as f64;
    Some(BurnRate {
        tokens_per_minute: total_tokens / duration_minutes,
        tokens_per_minute_for_indicator: non_cache / duration_minutes,
        cost_per_hour: block.cost_usd / duration_minutes * 60.0,
    })
}

fn project_block_usage(block: &SessionBlock, now: TimestampMs) -> Option<Projection> {
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
enum LimitStatus {
    Exceeds,
    Warning,
    Ok,
}

fn limit_status(projected: u64, limit: NonZeroU64) -> LimitStatus {
    let limit = limit.get();
    if projected > limit {
        LimitStatus::Exceeds
    } else if projected as f64 > limit as f64 * BLOCKS_WARNING_THRESHOLD {
        LimitStatus::Warning
    } else {
        LimitStatus::Ok
    }
}

pub(crate) fn format_remaining_time(minutes: i64) -> String {
    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours > 0 {
        format!("{hours}h {mins}m left")
    } else {
        format!("{mins}m left")
    }
}

#[cfg(test)]
mod tests;
