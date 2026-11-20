use std::{
    borrow::Cow,
    collections::HashMap,
    env, fs,
    io::{self, Read},
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ccusage_adapter_common::{jsonl, units::read_json};
use ccusage_core::{
    Color, Context, Diagnostics, MILLIS_PER_DAY, MILLIS_PER_MINUTE, ReportWindow, Result,
    TimestampMs,
    cli::{CostSource, SharedArgs, StatuslineArgs, VisualBurnRate},
    cli_error, color, format_currency, format_number, format_rfc3339_millis,
    pricing::PricingMap,
    sum_costs,
};
use serde::{Deserialize, Serialize};

use crate::{
    ClaudeEntry,
    blocks::{
        SessionBlock, calculate_burn_rate, format_remaining_time, identify_session_blocks,
        starts_fresh_block,
    },
    load_entries_since_with_diagnostics,
};

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

    let diagnostics = Diagnostics::for_report("claude", shared.clock);
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

const STATUSLINE_BLOCK_LOOKBACK_DAYS: [i64; 3] = [1, 7, 30];

fn load_statusline_active_block(
    args: &StatuslineArgs,
    shared: &SharedArgs,
    now: TimestampMs,
    diagnostics: &Diagnostics,
) -> Option<SessionBlock> {
    let session_ms = ccusage_core::cli::SessionLength::DEFAULT.as_millis();
    for lookback_days in STATUSLINE_BLOCK_LOOKBACK_DAYS {
        let window_shared = statusline_block_shared(args, shared, now, lookback_days);
        let mut entries =
            load_entries_since_with_diagnostics(&window_shared, None, diagnostics).ok()?;
        entries.sort_by_key(|entry| entry.timestamp);
        let timestamps = entries
            .iter()
            .map(|entry| entry.timestamp)
            .collect::<Vec<_>>();
        match statusline_block_anchor(&timestamps, now, session_ms) {
            BlockAnchor::NoActiveBlock => return None,
            BlockAnchor::From(start) => {
                entries.retain(|entry| entry.timestamp >= start);
                return find_active_block(entries, now);
            }
            BlockAnchor::NeedsMoreHistory => {}
            BlockAnchor::NeedsFullHistory => break,
        }
    }
    find_active_block(
        load_entries_since_with_diagnostics(shared, None, diagnostics).ok()?,
        now,
    )
}

fn find_active_block(entries: Vec<ClaudeEntry>, now: TimestampMs) -> Option<SessionBlock> {
    identify_session_blocks(
        entries.into_iter().map(|entry| entry.entry).collect(),
        ccusage_core::cli::SessionLength::DEFAULT,
        now,
    )
    .into_iter()
    .find(|block| block.is_active && !block.is_gap)
}

#[derive(Debug, PartialEq)]
enum BlockAnchor {
    NoActiveBlock,
    From(TimestampMs),
    NeedsMoreHistory,
    NeedsFullHistory,
}

fn statusline_block_anchor(
    timestamps: &[TimestampMs],
    now: TimestampMs,
    session_ms: i64,
) -> BlockAnchor {
    let Some(&last) = timestamps.last() else {
        return BlockAnchor::NoActiveBlock;
    };
    if last > now {
        return BlockAnchor::NeedsFullHistory;
    }
    if now.duration_since(last) >= session_ms {
        return BlockAnchor::NoActiveBlock;
    }
    timestamps
        .windows(2)
        .rposition(|pair| starts_fresh_block(pair[0], pair[1], session_ms))
        .map_or(BlockAnchor::NeedsMoreHistory, |index| {
            BlockAnchor::From(timestamps[index + 1])
        })
}

fn statusline_block_shared(
    args: &StatuslineArgs,
    shared: &SharedArgs,
    now: TimestampMs,
    lookback_days: i64,
) -> SharedArgs {
    let lookback =
        ccusage_core::cli::SessionLength::DEFAULT.as_millis() + lookback_days * MILLIS_PER_DAY;
    let since = ccusage_core::date_at(
        TimestampMs::from_millis(now.as_millis() - lookback),
        &ccusage_core::resolve_tz(args.timezone.as_ref()),
    )
    .into();
    SharedArgs {
        since: Some(since),
        timezone: args.timezone.clone(),
        ..shared.clone()
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

fn format_statusline_context(
    input_tokens: u64,
    context_limit: u64,
    args: &StatuslineArgs,
    shared: &SharedArgs,
) -> String {
    let percentage = if context_limit == 0 {
        0
    } else {
        ((input_tokens as f64 / context_limit as f64) * 100.0).round() as u64
    };
    let context_color = statusline_context_color(percentage, args);
    format!(
        "{} ({})",
        format_number(input_tokens),
        color(shared, format!("{percentage}%"), context_color)
    )
}

fn statusline_context_color(percentage: u64, args: &StatuslineArgs) -> Color {
    if percentage < u64::from(args.context_thresholds.low()) {
        Color::Green
    } else if percentage < u64::from(args.context_thresholds.medium()) {
        Color::Yellow
    } else {
        Color::Red
    }
}

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;

fn calculate_context_tokens_from_transcript(
    path: &Path,
    model_id: Option<&str>,
    offline: bool,
    shared: &SharedArgs,
) -> Option<HookContext> {
    let marker = ccusage_core::fast::LinePrefilter::all(&[br#""input_tokens""#]);
    let usage = jsonl::read_last(path, |line| {
        let bytes = line.raw();
        if marker.matches(bytes)
            && let Some(usage) = context_usage(bytes)
        {
            return ControlFlow::Break(usage);
        }
        ControlFlow::Continue(())
    })
    .ok()??;
    let total_input_tokens =
        usage.input_tokens? + usage.cache_creation_input_tokens + usage.cache_read_input_tokens;
    let context_window_size = model_id
        .filter(|model_id| !model_id.is_empty())
        .and_then(|model_id| {
            PricingMap::load_with_overrides(offline, false, shared.pricing_overrides.iter())
                .context_limit(model_id)
        })
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
    Some(HookContext {
        total_input_tokens,
        context_window_size,
    })
}

#[derive(Deserialize)]
struct ContextRecord<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    message: ContextMessage,
}

#[derive(Deserialize)]
struct ContextMessage {
    usage: TranscriptContextUsage,
}

#[derive(Deserialize)]
struct TranscriptContextUsage {
    input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_input_tokens: u64,
}

fn context_usage(bytes: &[u8]) -> Option<TranscriptContextUsage> {
    let bytes = bytes.trim_ascii();
    let start = bytes.iter().position(|byte| matches!(byte, b'{' | b'['))?;
    let end = bytes.iter().rposition(|byte| matches!(byte, b'}' | b']'))? + 1;
    if end <= start
        || !std::str::from_utf8(&bytes[..start]).ok()?.trim().is_empty()
        || !std::str::from_utf8(&bytes[end..]).ok()?.trim().is_empty()
    {
        return None;
    }
    let record = serde_json::from_slice::<ContextRecord<'_>>(&bytes[start..end]).ok()?;
    let usage = record.message.usage;
    (record.kind == "assistant" && usage.input_tokens.is_some()).then_some(usage)
}

#[derive(Debug, Deserialize, Serialize)]
struct StatuslineCache {
    date: String,
    #[serde(rename = "lastOutput")]
    last_output: String,
    #[serde(rename = "lastUpdateTime")]
    last_update_time: u64,
    #[serde(rename = "transcriptPath")]
    transcript_path: String,
    #[serde(rename = "transcriptMtime")]
    transcript_mtime: u64,
    #[serde(rename = "isUpdating", default)]
    is_updating: bool,
    pid: Option<u32>,
}

impl StatuslineCache {
    fn completed(
        hook: &StatuslineHook,
        last_output: String,
        transcript_mtime: u64,
        last_update_time: u64,
    ) -> Self {
        Self {
            date: format_cache_date(last_update_time),
            last_output,
            last_update_time,
            transcript_path: hook.transcript_path.clone(),
            transcript_mtime,
            is_updating: false,
            pid: None,
        }
    }

    fn updating(hook: &StatuslineHook, transcript_mtime: u64, previous: Option<&Self>) -> Self {
        let now = now_millis();
        Self {
            date: format_cache_date(now),
            last_output: previous
                .map(|cache| cache.last_output.clone())
                .unwrap_or_default(),
            last_update_time: previous
                .map(|cache| cache.last_update_time)
                .unwrap_or_default(),
            transcript_path: hook.transcript_path.clone(),
            transcript_mtime,
            is_updating: true,
            pid: Some(std::process::id()),
        }
    }
}

fn cached_statusline_output(
    cache: &StatuslineCache,
    current_mtime: u64,
    now: u64,
    refresh_interval: u64,
) -> Option<&str> {
    if cache.last_output.is_empty() {
        return None;
    }
    let expired =
        now.saturating_sub(cache.last_update_time) >= refresh_interval.saturating_mul(1000);
    let file_modified = cache.transcript_mtime != current_mtime;
    if expired || file_modified {
        if cache.is_updating && cache.pid.is_some_and(process_is_alive) {
            return Some(cache.last_output.as_str());
        }
        return None;
    }
    Some(cache.last_output.as_str())
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

#[cfg(not(unix))]
fn process_is_alive(pid: u32) -> bool {
    pid == std::process::id()
}

fn statusline_cache_path(session_id: &str) -> PathBuf {
    env::temp_dir()
        .join("ccusage-semaphore")
        .join(format!("{session_id}.lock"))
}

fn read_statusline_cache(path: &Path) -> Option<StatuslineCache> {
    read_json(path).ok()
}

fn write_statusline_cache(path: &Path, cache: StatuslineCache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&cache) {
        let _ = fs::write(path, bytes);
    }
}

fn mark_statusline_cache_updating(
    path: &Path,
    hook: &StatuslineHook,
    transcript_mtime: u64,
    previous: Option<&StatuslineCache>,
) {
    write_statusline_cache(
        path,
        StatuslineCache::updating(hook, transcript_mtime, previous),
    );
}

fn release_statusline_cache(path: &Path) {
    if let Some(mut cache) = read_statusline_cache(path) {
        cache.is_updating = false;
        cache.pid = None;
        write_statusline_cache(path, cache);
    }
}

fn transcript_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn format_cache_date(millis: u64) -> String {
    format_rfc3339_millis(TimestampMs::from_millis(
        i64::try_from(millis).unwrap_or(i64::MAX),
    ))
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
