use std::{
    collections::HashMap,
    io::{self, Read},
    path::Path,
};

use ccusage_core::{
    Context, Diagnostics, MILLIS_PER_MINUTE, ReportWindow, Result, TimestampMs,
    cli::{CostSource, SharedArgs, StatuslineArgs, VisualBurnRate},
    cli_error, format_currency, sum_costs,
};
use serde::Deserialize;

use crate::{
    ClaudeEntry,
    blocks::{calculate_burn_rate, format_remaining_time},
    load_entries_since_with_diagnostics,
};

mod block;
mod cache;
mod context;

use block::{find_active_block, load_statusline_active_block};
use cache::{
    StatuslineCache, cached_statusline_output, mark_statusline_cache_updating, now_millis,
    read_statusline_cache, release_statusline_cache, statusline_cache_path, transcript_mtime_ms,
    write_statusline_cache,
};
use context::{calculate_context_tokens_from_transcript, format_statusline_context};

pub fn run_statusline(args: StatuslineArgs) -> Result<()> {
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    if stdin.trim().is_empty() {
        return Err(cli_error("❌ No input provided"));
    }

    let hook: StatuslineHook =
        serde_json::from_str(stdin.trim()).context("Invalid input format")?;
    let shared = statusline_shared(&args);
    let cache_enabled = args.cache;
    let cache_path = statusline_cache_path(&hook.session_id);
    let transcript_path = Path::new(&hook.transcript_path);
    let current_mtime = transcript_mtime_ms(transcript_path).unwrap_or_default();
    let initial_cache = if cache_enabled {
        read_statusline_cache(&cache_path)
    } else {
        None
    };

    if let Some(cache) = initial_cache.as_ref()
        && let Some(output) =
            cached_statusline_output(cache, current_mtime, now_millis(), args.refresh_interval)
    {
        ccusage_core::outln!("{output}")?;
        return Ok(());
    }

    if cache_enabled {
        mark_statusline_cache_updating(&cache_path, &hook, current_mtime, initial_cache.as_ref());
    }

    let diagnostics = Diagnostics::new("claude");
    let statusline_result = render_statusline(&hook, &args, &shared, &diagnostics);
    diagnostics.emit(args.debug);
    match statusline_result {
        Ok(statusline) => {
            ccusage_core::outln!("{statusline}")?;
            if cache_enabled {
                write_statusline_cache(
                    &cache_path,
                    StatuslineCache::completed(&hook, statusline, current_mtime, now_millis()),
                );
            }
        }
        Err(error) => {
            let fallback = initial_cache
                .as_ref()
                .map(|cache| cache.last_output.as_str())
                .filter(|output| !output.is_empty())
                .unwrap_or("❌ Error generating status");
            let shown = ccusage_core::outln!("{fallback}");
            if cache_enabled {
                release_statusline_cache(&cache_path);
            }
            shown?;
            return Err(error);
        }
    }
    Ok(())
}

fn statusline_shared(args: &StatuslineArgs) -> SharedArgs {
    SharedArgs {
        clock: args.clock,
        offline: args.offline,
        color: args.color,
        mode: args.mode,
        timezone: args.timezone.clone(),
        debug: args.debug,
        pricing_overrides: args.pricing_overrides.clone(),
        ..SharedArgs::default()
    }
}

fn resolve_model_label<'a>(aliases: &'a HashMap<String, String>, display_name: &'a str) -> &'a str {
    aliases
        .get(display_name)
        .map(String::as_str)
        .unwrap_or(display_name)
}

// NOTE(2026-07-08, Claude Code 2.1.119, #1405): Claude Code omits effort for models without an effort setting. Keep the model label when the hook omits it.
fn format_model_segment(model_label: &str, effort: Option<&HookEffort>) -> String {
    match effort {
        Some(effort) if !effort.level.is_empty() => format!("{model_label} ({})", effort.level),
        _ => model_label.to_string(),
    }
}

fn render_statusline(
    hook: &StatuslineHook,
    args: &StatuslineArgs,
    shared: &SharedArgs,
    diagnostics: &Diagnostics,
) -> Result<String> {
    let now = shared.clock.now().into();
    let needs_history = matches!(args.cost_source, CostSource::Ccusage | CostSource::Both)
        || (args.cost_source == CostSource::Auto && hook.cost.is_none());
    let history = needs_history
        .then(|| load_entries_since_with_diagnostics(shared, None, diagnostics).ok())
        .flatten();
    let ccusage_cost = history
        .as_deref()
        .map(|entries| calculate_session_cost(&hook.session_id, entries));
    let cc_cost = hook.cost.as_ref().map(|cost| cost.total_cost_usd);
    let session_cost = match args.cost_source {
        CostSource::Cc => cc_cost,
        CostSource::Ccusage => ccusage_cost,
        CostSource::Auto => cc_cost.or(ccusage_cost),
        CostSource::Both => None,
    };

    let today_shared = statusline_today_shared(args, shared, now);
    let today_cost = if let Some(entries) = history.as_ref() {
        let window = ReportWindow::from_shared(&today_shared);
        let (since, until) = window.bounds_ms();
        sum_costs(
            entries
                .iter()
                .filter(|entry| {
                    let timestamp = entry.timestamp.as_millis();
                    since.is_none_or(|since| timestamp >= since)
                        && until.is_none_or(|until| timestamp < until)
                })
                .map(|entry| entry.cost),
        )
    } else {
        load_entries_since_with_diagnostics(&today_shared, None, diagnostics)
            .map(|entries| sum_costs(entries.iter().map(|entry| entry.cost)))
            .unwrap_or(0.0)
    };

    let active_block = if let Some(entries) = history {
        find_active_block(entries, now)
    } else {
        load_statusline_active_block(args, shared, now, diagnostics)
    };
    let (block_info, burn_rate_info) = if let Some(block) = active_block.as_ref() {
        let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
        let mut burn = String::new();
        if let Some(rate) = calculate_burn_rate(block) {
            let mut segments = vec![format!("{}/hr", format_currency(rate.cost_per_hour))];
            let status = if rate.tokens_per_minute_for_indicator < 2000.0 {
                ("🟢", "Normal")
            } else if rate.tokens_per_minute_for_indicator < 5000.0 {
                ("⚠️", "Moderate")
            } else {
                ("🚨", "High")
            };
            if matches!(
                args.visual_burn_rate,
                VisualBurnRate::Emoji | VisualBurnRate::EmojiText
            ) {
                segments.push(status.0.to_string());
            }
            if matches!(
                args.visual_burn_rate,
                VisualBurnRate::Text | VisualBurnRate::EmojiText
            ) {
                segments.push(format!("({})", status.1));
            }
            burn = format!(" | 🔥 {}", segments.join(" "));
        }
        (
            format!(
                "{} block ({})",
                format_currency(block.cost_usd),
                format_remaining_time(remaining)
            ),
            burn,
        )
    } else {
        ("No active block".to_string(), String::new())
    };

    let context_info = hook
        .context_window
        .as_ref()
        .map(|context| (context.total_input_tokens, context.context_window_size))
        .or_else(|| {
            calculate_context_tokens_from_transcript(
                Path::new(&hook.transcript_path),
                hook.model.id.as_deref(),
                shared.offline,
                shared,
            )
            .map(|context| (context.total_input_tokens, context.context_window_size))
        })
        .map(|(total_input_tokens, context_window_size)| {
            format_statusline_context(total_input_tokens, context_window_size, args, shared)
        });

    let session_display = if args.cost_source == CostSource::Both {
        format!(
            "({} cc / {} ccusage)",
            cc_cost
                .map(format_currency)
                .unwrap_or_else(|| "N/A".to_string()),
            ccusage_cost
                .map(format_currency)
                .unwrap_or_else(|| "N/A".to_string())
        )
    } else {
        session_cost
            .map(format_currency)
            .unwrap_or_else(|| "N/A".to_string())
    };

    let model_label = resolve_model_label(&args.model_label_aliases, &hook.model.display_name);
    let model_segment = format_model_segment(model_label, hook.effort.as_ref());

    Ok(format!(
        "🤖 {} | 💰 {} session / {} today / {}{} | 🧠 {}",
        model_segment,
        session_display,
        format_currency(today_cost),
        block_info,
        burn_rate_info,
        context_info.unwrap_or_else(|| "N/A".to_string())
    ))
}

fn statusline_today_shared(
    args: &StatuslineArgs,
    shared: &SharedArgs,
    now: TimestampMs,
) -> SharedArgs {
    let today =
        ccusage_core::date_at(now, &ccusage_core::resolve_tz(args.timezone.as_ref())).into();
    SharedArgs {
        clock: shared.clock,
        since: Some(today),
        until: Some(today),
        offline: shared.offline,
        pricing_overrides: shared.pricing_overrides.clone(),
        timezone: args.timezone.clone(),
        ..SharedArgs::default()
    }
}

fn calculate_session_cost(session_id: &str, entries: &[ClaudeEntry]) -> f64 {
    sum_costs(
        entries
            .iter()
            .filter(|entry| {
                entry.recorded_session_id.as_deref() == Some(session_id)
                    || entry.session_id.as_ref() == session_id
            })
            .map(|entry| entry.cost),
    )
}

#[derive(Debug, Deserialize)]
struct StatuslineHook {
    session_id: String,
    transcript_path: String,
    model: HookModel,
    cost: Option<HookCost>,
    context_window: Option<HookContext>,
    effort: Option<HookEffort>,
}

#[derive(Debug, Deserialize)]
struct HookModel {
    id: Option<String>,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct HookEffort {
    level: String,
}

#[derive(Debug, Deserialize)]
struct HookCost {
    total_cost_usd: f64,
}

#[derive(Debug, Deserialize)]
struct HookContext {
    total_input_tokens: u64,
    context_window_size: u64,
}

#[cfg(test)]
mod tests;
