use std::num::NonZeroU64;

use ccusage_core::{
    Align, Color, ColumnKind, MILLIS_PER_MINUTE, Result, SimpleTable, TimestampMs, cli::SharedArgs,
    color, format_currency, format_models_multiline, format_number, outln, print_box_title,
    should_use_compact_layout, terminal_style, terminal_width,
};
use jiff::tz::TimeZone;

use super::{
    SessionBlock, calculate_burn_rate, limit_status, project_block_usage,
    projection::LimitStatus,
    time::{format_block_started, format_block_time},
};

const BLOCKS_COMPACT_WIDTH_THRESHOLD: usize = 120;

fn format_block_models(models: &[String]) -> String {
    if models.is_empty() {
        "-".to_string()
    } else {
        format_models_multiline(models)
    }
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
