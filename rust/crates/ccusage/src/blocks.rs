use std::io::IsTerminal;

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    Align, Color, LoadedEntry, MILLIS_PER_HOUR, MILLIS_PER_MINUTE, Result, SimpleTable,
    TimestampMs, TokenCounts, am_pm,
    cli::{SharedArgs, SortOrder},
    color,
    fast::FxHashSet,
    format_currency, format_date, format_models_multiline, format_number, format_rfc3339_millis,
    format_utc_second, hour_12, json_float, local_parts, print_box_title,
    should_use_compact_layout, terminal_width, utc_now,
};

/// A block is warned about once it passes this share of the token limit.
const BLOCKS_WARNING_THRESHOLD: f64 = 0.8;
/// Terminal width below which the blocks table drops columns.
const BLOCKS_COMPACT_WIDTH_THRESHOLD: usize = 120;

#[derive(Debug, Clone)]
pub struct SessionBlock {
    pub id: String,
    pub start_time: TimestampMs,
    pub end_time: TimestampMs,
    pub actual_end_time: Option<TimestampMs>,
    pub is_active: bool,
    pub is_gap: bool,
    pub entries: Vec<LoadedEntry>,
    pub token_counts: TokenCounts,
    pub cost_usd: f64,
    pub models: Vec<String>,
    pub usage_limit_reset_time: Option<TimestampMs>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnRate {
    pub tokens_per_minute: f64,
    pub tokens_per_minute_for_indicator: f64,
    pub cost_per_hour: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    pub total_tokens: u64,
    pub total_cost: f64,
    pub remaining_minutes: u64,
}

pub fn identify_session_blocks(
    mut entries: Vec<LoadedEntry>,
    session_duration_hours: f64,
) -> Vec<SessionBlock> {
    if entries.is_empty() {
        return Vec::new();
    }
    let session_duration = (session_duration_hours * MILLIS_PER_HOUR as f64) as i64;
    entries.sort_by_key(|entry| entry.timestamp);
    let now = utc_now();
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
            let since_last = entry.timestamp.duration_since(last_time);
            if since_start > session_duration || since_last > session_duration {
                blocks.push(create_block(
                    start,
                    std::mem::take(&mut current_entries),
                    now,
                    session_duration,
                ));
                if since_last > session_duration {
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
        token_counts.add_usage(entry.data.message.usage);
        cost += entry.cost;
        if let Some(model) = &entry.model {
            let model = crate::model_aliases::resolve_model_name(model).into_owned();
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

pub fn filter_blocks_by_date(blocks: &mut Vec<SessionBlock>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    blocks.retain(|block| {
        let date = format_date(block.start_time, shared.timezone.as_deref()).replace('-', "");
        shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
    });
}

pub fn sort_blocks(blocks: &mut [SessionBlock], order: &SortOrder) {
    blocks.sort_by_key(|block| block.start_time);
    if *order == SortOrder::Desc {
        blocks.reverse();
    }
}

pub fn block_json(block: &SessionBlock, token_limit: Option<&str>, max_tokens: u64) -> Value {
    let burn_rate = if block.is_active {
        calculate_burn_rate(block)
    } else {
        None
    };
    let projection = if block.is_active {
        project_block_usage(block)
    } else {
        None
    };
    let token_limit_status = projection.and_then(|projection| {
        let limit = token_limit.and_then(|_| parse_token_limit(token_limit, max_tokens))?;
        let percent = projection.total_tokens as f64 / limit as f64 * 100.0;
        Some(json!({
            "limit": limit,
            "projectedUsage": projection.total_tokens,
            "percentUsed": percent,
            "status": if projection.total_tokens > limit { "exceeds" } else if projection.total_tokens as f64 > limit as f64 * BLOCKS_WARNING_THRESHOLD { "warning" } else { "ok" },
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

fn format_block_time(block: &SessionBlock, compact: bool) -> String {
    let start = format_local_block_start(block.start_time, compact);
    if block.is_gap {
        let end = format_local_block_end(block.end_time, compact);
        let duration = block.end_time.duration_since(block.start_time) / MILLIS_PER_HOUR;
        return if compact {
            format!("{start}-{end}\n({duration}h gap)")
        } else {
            format!("{start} - {end} ({duration}h gap)")
        };
    }

    if block.is_active {
        let now = utc_now();
        let elapsed = now.duration_since(block.start_time) / MILLIS_PER_MINUTE;
        let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
        let elapsed_hours = elapsed / 60;
        let elapsed_minutes = elapsed.rem_euclid(60);
        let remaining_hours = remaining / 60;
        let remaining_minutes = remaining.rem_euclid(60);
        return if compact {
            format!(
                "{start}\n({elapsed_hours}h{elapsed_minutes}m/{remaining_hours}h{remaining_minutes}m)"
            )
        } else {
            format!(
                "{start} ({elapsed_hours}h {elapsed_minutes}m elapsed, {remaining_hours}h {remaining_minutes}m remaining)"
            )
        };
    }

    let duration = block
        .actual_end_time
        .map(|end| end.duration_since(block.start_time) / MILLIS_PER_MINUTE)
        .unwrap_or(0);
    let hours = duration / 60;
    let minutes = duration.rem_euclid(60);
    if compact {
        if hours > 0 {
            format!("{start}\n({hours}h{minutes}m)")
        } else {
            format!("{start}\n({minutes}m)")
        }
    } else if hours > 0 {
        format!("{start} ({hours}h {minutes}m)")
    } else {
        format!("{start} ({minutes}m)")
    }
}

fn format_local_block_start(timestamp: TimestampMs, compact: bool) -> String {
    let parts = local_parts(timestamp);
    if compact {
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
    }
}

fn format_local_block_end(timestamp: TimestampMs, compact: bool) -> String {
    let parts = local_parts(timestamp);
    if compact {
        format!(
            "{:02}:{:02} {}",
            hour_12(parts.hour),
            parts.minute,
            am_pm(parts.hour)
        )
    } else {
        format_local_block_start(timestamp, false)
    }
}

pub fn print_blocks_table(
    blocks: &[SessionBlock],
    token_limit: Option<&str>,
    max_tokens: u64,
    shared: &SharedArgs,
) -> Result<()> {
    if blocks.is_empty() {
        eprintln!("No Claude usage data found.");
        return Ok(());
    }
    let terminal_width = terminal_width();
    let is_tty = std::io::stdout().is_terminal();
    let compact = should_use_compact_layout(
        shared,
        is_tty,
        terminal_width,
        BLOCKS_COMPACT_WIDTH_THRESHOLD,
    );
    let actual_limit = parse_token_limit(token_limit, max_tokens);
    print_box_title("Claude Code Token Usage Report - Session Blocks", shared);
    let mut headers = vec!["Block Start", "Duration/Status", "Models", "Tokens"];
    let mut aligns = vec![Align::Left, Align::Left, Align::Left, Align::Right];
    if actual_limit.is_some_and(|limit| limit > 0) {
        headers.push("%");
        aligns.push(Align::Right);
    }
    headers.push("Cost");
    aligns.push(Align::Right);
    if shared.no_cost {
        headers.pop();
        aligns.pop();
    }
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(terminal_width);
    for block in blocks {
        if block.is_gap {
            let mut row = vec![
                color(shared, format_block_time(block, compact), Color::Grey),
                color(shared, "(inactive)", Color::Grey),
                color(shared, "-", Color::Grey),
                color(shared, "-", Color::Grey),
            ];
            if actual_limit.is_some_and(|limit| limit > 0) {
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
            format_block_time(block, compact),
            if block.is_active {
                color(shared, "ACTIVE", Color::Green)
            } else {
                String::new()
            },
            format_block_models(&block.models),
            format_number(total),
        ];
        if let Some(limit) = actual_limit.filter(|limit| *limit > 0) {
            let percentage = total as f64 / limit as f64 * 100.0;
            let percent_text = format!("{percentage:.1}%");
            row.push(if percentage > 100.0 {
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
            if let Some(limit) = actual_limit.filter(|limit| *limit > 0) {
                table.separator();
                let remaining = limit.saturating_sub(total);
                let remaining_percent = (limit.saturating_sub(total) as f64 / limit as f64) * 100.0;
                let mut remaining_row = vec![
                    color(
                        shared,
                        format!("(assuming {} token limit)", format_number(limit)),
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
                table.push(remaining_row);
            }

            if let Some(projection) = project_block_usage(block) {
                table.separator();
                let mut projected_row = vec![
                    color(shared, "(assuming current burn rate)", Color::Grey),
                    color(shared, "PROJECTED", Color::Yellow),
                    String::new(),
                    match actual_limit {
                        Some(limit) if limit > 0 && projection.total_tokens > limit => {
                            color(shared, format_number(projection.total_tokens), Color::Red)
                        }
                        _ => format_number(projection.total_tokens),
                    },
                ];
                if let Some(limit) = actual_limit.filter(|limit| *limit > 0) {
                    let percentage = projection.total_tokens as f64 / limit as f64 * 100.0;
                    projected_row.push(format!("{percentage:.1}%"));
                }
                if !shared.no_cost {
                    projected_row.push(format_currency(projection.total_cost));
                }
                table.push(projected_row);
            }
        }
    }
    table.print()?;
    Ok(())
}

pub fn print_active_block_detail(
    block: &SessionBlock,
    token_limit: Option<&str>,
    max_tokens: u64,
    shared: &SharedArgs,
) {
    print_box_title("Current Session Block Status", shared);
    let now = utc_now();
    let elapsed = now.duration_since(block.start_time) / MILLIS_PER_MINUTE;
    let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
    println!("Block Started:   {}", format_utc_second(block.start_time));
    println!(
        "Time Elapsed:    {}h {}m",
        elapsed / 60,
        elapsed.rem_euclid(60)
    );
    println!(
        "Time Remaining:  {}",
        color(
            shared,
            format!("{}h {}m", remaining / 60, remaining.rem_euclid(60)),
            Color::Green,
        )
    );
    println!();
    println!("{}", color(shared, "Current Usage:", Color::Blue));
    println!(
        "  Input Tokens:     {}",
        format_number(block.token_counts.input_tokens)
    );
    println!(
        "  Output Tokens:    {}",
        format_number(block.token_counts.output_tokens)
    );
    if !shared.no_cost {
        println!("  Total Cost:       {}", format_currency(block.cost_usd));
    }

    if let Some(rate) = calculate_burn_rate(block) {
        println!();
        println!("{}", color(shared, "Burn Rate:", Color::Blue));
        println!(
            "  Tokens/minute:    {}",
            format_number(rate.tokens_per_minute.round() as u64)
        );
        if !shared.no_cost {
            println!(
                "  Cost/hour:        {}",
                format_currency(rate.cost_per_hour)
            );
        }
    }

    if let Some(projection) = project_block_usage(block) {
        println!();
        println!(
            "{}",
            color(
                shared,
                "Projected Usage (if current rate continues):",
                Color::Blue
            )
        );
        println!(
            "  Total Tokens:     {}",
            format_number(projection.total_tokens)
        );
        if !shared.no_cost {
            println!(
                "  Total Cost:       {}",
                format_currency(projection.total_cost)
            );
        }

        if let Some(limit) = parse_token_limit(token_limit, max_tokens) {
            let current = block.token_counts.total();
            let remaining_tokens = limit.saturating_sub(current);
            let percent = projection.total_tokens as f64 / limit as f64 * 100.0;
            let status = if projection.total_tokens > limit {
                color(shared, "EXCEEDS LIMIT", Color::Red)
            } else if projection.total_tokens as f64 > limit as f64 * BLOCKS_WARNING_THRESHOLD {
                color(shared, "WARNING", Color::Yellow)
            } else {
                color(shared, "OK", Color::Green)
            };
            println!();
            println!("{}", color(shared, "Token Limit Status:", Color::Blue));
            println!("  Limit:            {} tokens", format_number(limit));
            println!(
                "  Current Usage:    {} ({:.1}%)",
                format_number(current),
                current as f64 / limit as f64 * 100.0
            );
            println!(
                "  Remaining:        {} tokens",
                format_number(remaining_tokens)
            );
            println!("  Projected Usage:  {percent:.1}% {status}");
        }
    }
}

pub fn calculate_burn_rate(block: &SessionBlock) -> Option<BurnRate> {
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

fn project_block_usage(block: &SessionBlock) -> Option<Projection> {
    if !block.is_active || block.is_gap {
        return None;
    }
    let burn = calculate_burn_rate(block)?;
    let remaining_minutes =
        (block.end_time.duration_since(utc_now()) as f64 / MILLIS_PER_MINUTE as f64).round();
    let total_tokens =
        block.token_counts.total() as f64 + burn.tokens_per_minute * remaining_minutes;
    let total_cost = block.cost_usd + (burn.cost_per_hour / 60.0) * remaining_minutes;
    Some(Projection {
        total_tokens: total_tokens.round() as u64,
        total_cost: (total_cost * 100.0).round() / 100.0,
        remaining_minutes: remaining_minutes as u64,
    })
}

fn parse_token_limit(value: Option<&str>, max_tokens: u64) -> Option<u64> {
    match value {
        None | Some("") | Some("max") => (max_tokens > 0).then_some(max_tokens),
        Some(value) => value.parse().ok(),
    }
}

pub fn format_remaining_time(minutes: i64) -> String {
    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours > 0 {
        format!("{hours}h {mins}m left")
    } else {
        format!("{mins}m left")
    }
}
