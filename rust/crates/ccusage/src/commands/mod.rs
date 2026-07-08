use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::pricing::PricingMap;
use crate::{
    BucketKind, Color, Context, DEFAULT_RECENT_DAYS, DEFAULT_SESSION_DURATION_HOURS,
    MILLIS_PER_DAY, MILLIS_PER_MINUTE, Result, SessionAccumulator, TimestampMs, block_json,
    calculate_burn_rate,
    cli::{
        BlocksArgs, CostSource, DailyArgs, SessionArgs, SharedArgs, SortOrder, StatuslineArgs,
        VisualBurnRate, WeekDay, WeeklyArgs,
    },
    color,
    fast::FxHashMap,
    filter_and_sort_summaries, filter_blocks_by_date, format_currency, format_date, format_number,
    format_remaining_time, format_rfc3339_millis, group_project_output, identify_session_blocks,
    load_daily_summaries, load_entries, print_active_block_detail, print_blocks_table,
    print_json_or_jq, print_usage_table, session_summary_json, sort_blocks, sort_summaries,
    summarize_by_key, summarize_summaries_by_bucket, summary_json, total_usage_tokens, totals_json,
    utc_now, wants_json,
};

pub(crate) fn run_daily(args: DailyArgs) -> Result<()> {
    let shared = args.shared.clone();
    let mut rows = load_daily_summaries(
        &shared,
        args.project.as_deref(),
        args.instances || args.project.is_some(),
    )?;
    filter_and_sort_summaries(&mut rows, &shared, |row| {
        row.date.as_deref().unwrap_or_default()
    });

    if wants_json(&shared) {
        if args.instances && rows.iter().any(|row| row.project.is_some()) {
            let output = json!({
                "projects": group_project_output(&rows),
                "totals": totals_json(&rows),
            });
            print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        } else {
            let output = json!({
                "daily": rows.iter().map(summary_json).collect::<Vec<_>>(),
                "totals": totals_json(&rows),
            });
            print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        }
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - Daily",
        "Date",
        &rows,
        &shared,
        args.instances,
        args.project_aliases.as_deref(),
    )?;
    Ok(())
}

pub(crate) fn run_bucket(shared: SharedArgs, kind: BucketKind) -> Result<()> {
    let entries = load_entries(&shared, None)?;
    let mut daily = summarize_by_key(
        &entries,
        |entry| entry.date.clone(),
        |key| (key.to_string(), None),
    )?;
    filter_and_sort_summaries(&mut daily, &shared, |row| {
        row.date.as_deref().unwrap_or_default()
    });

    let mut buckets = summarize_summaries_by_bucket(&daily, kind, WeekDay::Sunday);
    sort_summaries(&mut buckets, &shared.order, |row| match kind {
        BucketKind::Monthly => row.month.as_deref().unwrap_or_default(),
        BucketKind::Weekly => row.week.as_deref().unwrap_or_default(),
    });

    if wants_json(&shared) {
        let key = match kind {
            BucketKind::Monthly => "monthly",
            BucketKind::Weekly => "weekly",
        };
        let output = json!({
            key: buckets.iter().map(summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&buckets),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    let (title, col) = match kind {
        BucketKind::Monthly => ("Claude Code Token Usage Report - Monthly", "Month"),
        BucketKind::Weekly => ("Claude Code Token Usage Report - Weekly", "Week"),
    };
    print_usage_table(title, col, &buckets, &shared, false, None)?;
    Ok(())
}

pub(crate) fn run_weekly(args: WeeklyArgs) -> Result<()> {
    let shared = args.shared.clone();
    let entries = load_entries(&shared, None)?;
    let mut daily = summarize_by_key(
        &entries,
        |entry| entry.date.clone(),
        |key| (key.to_string(), None),
    )?;
    filter_and_sort_summaries(&mut daily, &shared, |row| {
        row.date.as_deref().unwrap_or_default()
    });
    let mut weekly = summarize_summaries_by_bucket(&daily, BucketKind::Weekly, args.start_of_week);
    sort_summaries(&mut weekly, &shared.order, |row| {
        row.week.as_deref().unwrap_or_default()
    });

    if wants_json(&shared) {
        let output = json!({
            "weekly": weekly.iter().map(summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&weekly),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - Weekly",
        "Week",
        &weekly,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

pub(crate) fn run_session(args: SessionArgs) -> Result<()> {
    let shared = args.shared.clone();
    if let Some(id) = args.id {
        return run_session_id(&id, &shared);
    }

    let mut session_shared = shared.clone();
    session_shared.order = SortOrder::Desc;
    let entries = load_entries(&session_shared, None)?;
    let mut grouped = Vec::<SessionAccumulator>::new();
    let mut group_indexes = FxHashMap::<(Arc<str>, Arc<str>), usize>::default();
    for entry in &entries {
        let key = (
            Arc::clone(&entry.project_path),
            Arc::clone(&entry.session_id),
        );
        let index = *group_indexes.entry(key).or_insert_with(|| {
            let index = grouped.len();
            grouped.push(SessionAccumulator::default());
            index
        });
        grouped[index].add_entry(entry);
    }

    let mut rows = Vec::with_capacity(grouped.len());
    for group in grouped {
        rows.push(group.into_summary()?);
    }
    if session_shared.since.is_some() || session_shared.until.is_some() {
        rows.retain(|row| {
            let date = row
                .last_activity
                .as_deref()
                .unwrap_or_default()
                .replace('-', "");
            session_shared
                .since
                .as_ref()
                .is_none_or(|since| &date >= since)
                && session_shared
                    .until
                    .as_ref()
                    .is_none_or(|until| &date <= until)
        });
    }
    rows.retain(|row| {
        row.input_tokens + row.output_tokens + row.cache_creation_tokens + row.cache_read_tokens > 0
    });
    rows.sort_by(|a, b| match session_shared.order {
        SortOrder::Asc => a.total_cost.total_cmp(&b.total_cost),
        SortOrder::Desc => b.total_cost.total_cmp(&a.total_cost),
    });

    if wants_json(&session_shared) {
        let output = json!({
            "sessions": rows.iter().map(session_summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&rows),
        });
        print_json_or_jq(output, session_shared.jq.as_deref(), session_shared.no_cost)?;
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - By Session",
        "Session",
        &rows,
        &session_shared,
        false,
        None,
    )?;
    Ok(())
}

fn run_session_id(id: &str, shared: &SharedArgs) -> Result<()> {
    let entries = load_entries(shared, None)?;
    let mut session_entries = entries
        .into_iter()
        .filter(|entry| {
            entry.data.session_id.as_deref() == Some(id) || entry.session_id.as_ref() == id
        })
        .collect::<Vec<_>>();
    session_entries.sort_by_key(|entry| entry.timestamp);

    if session_entries.is_empty() {
        if wants_json(shared) {
            println!("null");
        } else {
            eprintln!("No session found with ID: {id}");
        }
        return Ok(());
    }

    let total_cost = session_entries.iter().map(|entry| entry.cost).sum::<f64>();
    let total_tokens = session_entries
        .iter()
        .map(|entry| total_usage_tokens(entry.data.message.usage))
        .sum::<u64>();

    if wants_json(shared) {
        let output = json!({
            "sessionId": id,
            "totalCost": total_cost,
            "totalTokens": total_tokens,
            "entries": session_entries.iter().map(|entry| json!({
                "timestamp": entry.data.timestamp,
                "inputTokens": entry.data.message.usage.input_tokens,
                "outputTokens": entry.data.message.usage.output_tokens,
                "cacheCreationTokens": entry.data.message.usage.cache_creation_input_tokens,
                "cacheReadTokens": entry.data.message.usage.cache_read_input_tokens,
                "model": entry.data.message.model.as_deref().unwrap_or("unknown"),
                "costUSD": entry.data.cost_usd.unwrap_or(0.0),
            })).collect::<Vec<_>>(),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    println!("Claude Code Session Usage - {id}");
    if !shared.no_cost {
        println!("Total Cost: {}", format_currency(total_cost));
    }
    println!("Total Tokens: {}", format_number(total_tokens));
    println!("Total Entries: {}", session_entries.len());
    Ok(())
}

pub(crate) fn run_blocks(args: BlocksArgs) -> Result<()> {
    if args.session_length <= 0.0 {
        return Err(crate::cli_error("Session length must be a positive number"));
    }
    let shared = args.shared.clone();
    let entries = load_entries(&shared, None)?;
    let mut blocks = identify_session_blocks(entries, args.session_length);
    filter_blocks_by_date(&mut blocks, &shared);
    sort_blocks(&mut blocks, &shared.order);

    if args.recent {
        let cutoff = utc_now()
            .checked_sub_millis(DEFAULT_RECENT_DAYS * MILLIS_PER_DAY)
            .unwrap_or(TimestampMs::UNIX_EPOCH);
        blocks.retain(|block| block.start_time >= cutoff || block.is_active);
    }

    if args.active {
        blocks.retain(|block| block.is_active);
    }

    let max_tokens = blocks
        .iter()
        .filter(|block| !block.is_gap && !block.is_active)
        .map(|block| block.token_counts.total())
        .max()
        .unwrap_or(0);

    if wants_json(&shared) {
        let output = json!({
            "blocks": blocks.iter().map(|block| block_json(block, args.token_limit.as_deref(), max_tokens)).collect::<Vec<_>>(),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    if args.active && blocks.is_empty() {
        println!("No active session block found.");
        return Ok(());
    }
    if args.active && blocks.len() == 1 {
        print_active_block_detail(&blocks[0], args.token_limit.as_deref(), max_tokens, &shared);
        return Ok(());
    }
    print_blocks_table(&blocks, args.token_limit.as_deref(), max_tokens, &shared)?;
    Ok(())
}

pub(crate) fn run_statusline(args: StatuslineArgs) -> Result<()> {
    if args.context_low_threshold >= args.context_medium_threshold {
        return Err(crate::cli_error(format!(
            "Context low threshold ({}) must be less than medium threshold ({})",
            args.context_low_threshold, args.context_medium_threshold
        )));
    }

    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    if stdin.trim().is_empty() {
        return Err(crate::cli_error("❌ No input provided"));
    }

    let hook: StatuslineHook =
        serde_json::from_str(stdin.trim()).context("Invalid input format")?;
    let shared = SharedArgs {
        offline: args.offline && !args.no_offline,
        ..SharedArgs::default()
    };
    let cache_enabled = args.cache && !args.no_cache;
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
        println!("{output}");
        return Ok(());
    }

    if cache_enabled {
        mark_statusline_cache_updating(&cache_path, &hook, current_mtime, initial_cache.as_ref());
    }

    let statusline_result = render_statusline(&hook, &args, &shared);
    match statusline_result {
        Ok(statusline) => {
            println!("{statusline}");
            if cache_enabled {
                write_statusline_cache(
                    &cache_path,
                    StatuslineCache::completed(&hook, statusline, current_mtime, now_millis()),
                );
            }
        }
        Err(error) => {
            if let Some(cache) = initial_cache
                .as_ref()
                .filter(|cache| !cache.last_output.is_empty())
            {
                println!("{}", cache.last_output);
            } else {
                println!("❌ Error generating status");
            }
            if cache_enabled {
                release_statusline_cache(&cache_path);
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Resolve the model label shown in the statusline.
///
/// Looks up the model's `display_name` in the user-configured alias map and
/// returns the matching short label when present. Long identifiers such as AWS
/// Bedrock inference profile ARNs can therefore be displayed as a concise name.
/// Falls back to the original `display_name` when no alias matches.
fn resolve_model_label<'a>(
    aliases: &'a std::collections::HashMap<String, String>,
    display_name: &'a str,
) -> &'a str {
    aliases
        .get(display_name)
        .map(String::as_str)
        .unwrap_or(display_name)
}

/// Format the model segment of the statusline.
///
/// Appends the reasoning effort level reported by Claude Code (e.g.
/// `Fable 5 (high)`) when present. Claude Code omits the `effort` field for
/// models without the effort parameter, so the segment falls back to the bare
/// model label in that case.
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
) -> Result<String> {
    let session_cost = match args.cost_source {
        CostSource::Cc => hook.cost.as_ref().map(|cost| cost.total_cost_usd),
        CostSource::Ccusage => calculate_session_cost(&hook.session_id, shared).ok(),
        CostSource::Auto => hook
            .cost
            .as_ref()
            .map(|cost| cost.total_cost_usd)
            .or_else(|| calculate_session_cost(&hook.session_id, shared).ok()),
        CostSource::Both => None,
    };

    let ccusage_cost = if args.cost_source == CostSource::Both {
        calculate_session_cost(&hook.session_id, shared).ok()
    } else {
        None
    };
    let cc_cost = if args.cost_source == CostSource::Both {
        hook.cost.as_ref().map(|cost| cost.total_cost_usd)
    } else {
        None
    };

    let today_shared = statusline_today_shared(args, shared, utc_now());
    let today_cost = load_entries(&today_shared, None)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    entry.date.replace('-', "") == today_shared.since.as_deref().unwrap_or_default()
                })
                .map(|entry| entry.cost)
                .sum::<f64>()
        })
        .unwrap_or(0.0);

    let blocks = load_entries(shared, None)
        .map(|entries| identify_session_blocks(entries, DEFAULT_SESSION_DURATION_HOURS))
        .unwrap_or_default();
    let active_block = blocks.iter().find(|block| block.is_active && !block.is_gap);
    let (block_info, burn_rate_info) = if let Some(block) = active_block {
        let remaining = block.end_time.duration_since(utc_now()) / MILLIS_PER_MINUTE;
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
    let today = format_date(now, args.timezone.as_deref()).replace('-', "");
    SharedArgs {
        since: Some(today.clone()),
        until: Some(today),
        offline: shared.offline,
        pricing_overrides: shared.pricing_overrides.clone(),
        timezone: args.timezone.clone(),
        ..SharedArgs::default()
    }
}

fn calculate_session_cost(session_id: &str, shared: &SharedArgs) -> Result<f64> {
    Ok(load_entries(shared, None)?
        .into_iter()
        .filter(|entry| {
            entry.data.session_id.as_deref() == Some(session_id)
                || entry.session_id.as_ref() == session_id
        })
        .map(|entry| entry.cost)
        .sum())
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
    if percentage < u64::from(args.context_low_threshold) {
        Color::Green
    } else if percentage < u64::from(args.context_medium_threshold) {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn calculate_context_tokens_from_transcript(
    path: &Path,
    model_id: Option<&str>,
    offline: bool,
    shared: &SharedArgs,
) -> Option<HookContext> {
    let content = fs::read_to_string(path).ok()?;
    let mut pricing: Option<PricingMap> = None;
    for line in content.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = value
            .get("message")
            .and_then(|message| message.get("usage"))
        else {
            continue;
        };
        let Some(input_tokens) = usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let context_window_size = model_id
            .filter(|model_id| !model_id.is_empty())
            .and_then(|model_id| {
                pricing
                    .get_or_insert_with(|| {
                        PricingMap::load_with_overrides(
                            offline,
                            false,
                            shared.pricing_overrides.iter(),
                        )
                    })
                    .context_limit(model_id)
            })
            .unwrap_or(200_000);
        return Some(HookContext {
            total_input_tokens: input_tokens + cache_creation + cache_read,
            context_window_size,
        });
    }
    None
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
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
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
    serde_json::from_slice(&fs::read(path).ok()?).ok()
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
    format_rfc3339_millis(TimestampMs::from_millis(millis.min(i64::MAX as u64) as i64))
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
mod tests {
    use ccusage_test_support::fs_fixture;

    use super::*;

    #[test]
    fn calculates_context_tokens_from_latest_assistant_transcript_line() {
        let fixture = fs_fixture!({
            "transcript.jsonl": [
                r#"{"type":"assistant","message":{"usage":{"input_tokens":1000,"output_tokens":999}}}"#,
                r#"not json"#,
                r#"{"type":"assistant","message":{"usage":{"input_tokens":2000,"cache_creation_input_tokens":100,"cache_read_input_tokens":50,"output_tokens":888}}}"#,
            ]
            .join("\n"),
        });

        let context = calculate_context_tokens_from_transcript(
            &fixture.path("transcript.jsonl"),
            None,
            true,
            &SharedArgs::default(),
        )
        .unwrap();

        assert_eq!(context.total_input_tokens, 2150);
        assert_eq!(context.context_window_size, 200_000);
    }

    #[test]
    fn uses_model_context_limit_for_transcript_context_tokens() {
        let fixture = fs_fixture!({
            "transcript.jsonl": r#"{"type":"assistant","message":{"usage":{"input_tokens":1000}}}"#,
        });

        let mut shared = SharedArgs::default();
        shared.pricing_overrides.insert(
            "test-model-context-limit".to_string(),
            ccusage_cli::PricingOverride {
                max_input_tokens: Some(1_500_000),
                ..Default::default()
            },
        );

        let context = calculate_context_tokens_from_transcript(
            &fixture.path("transcript.jsonl"),
            Some("test-model-context-limit"),
            true,
            &shared,
        )
        .unwrap();

        assert_eq!(context.total_input_tokens, 1000);
        assert_eq!(context.context_window_size, 1_500_000);
    }

    #[test]
    fn colors_statusline_context_percentage_by_threshold() {
        let shared = SharedArgs {
            color: true,
            ..SharedArgs::default()
        };
        let args = StatuslineArgs::default();

        assert!(matches!(statusline_context_color(60, &args), Color::Yellow));
        assert!(format_statusline_context(120_000, 200_000, &args, &shared).contains("60%"));
    }

    #[test]
    fn builds_statusline_today_filter_from_timezone() {
        let args = StatuslineArgs {
            timezone: Some("Asia/Tokyo".to_string()),
            ..StatuslineArgs::default()
        };
        let mut shared = SharedArgs {
            offline: true,
            ..SharedArgs::default()
        };
        shared.pricing_overrides.insert(
            "statusline-model".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(1e-6),
                ..Default::default()
            },
        );
        let now = TimestampMs::from_millis(1_779_380_820_000);

        let today_shared = statusline_today_shared(&args, &shared, now);

        assert_eq!(today_shared.since.as_deref(), Some("20260522"));
        assert_eq!(today_shared.until.as_deref(), Some("20260522"));
        assert_eq!(today_shared.timezone.as_deref(), Some("Asia/Tokyo"));
        assert!(
            today_shared
                .pricing_overrides
                .contains_key("statusline-model")
        );
    }

    #[test]
    fn reuses_statusline_cache_while_fresh_and_transcript_unchanged() {
        let cache = StatuslineCache {
            date: "2026-01-01T00:00:00.000Z".to_string(),
            last_output: "cached status".to_string(),
            last_update_time: 10_000,
            transcript_path: "/tmp/transcript.jsonl".to_string(),
            transcript_mtime: 123,
            is_updating: false,
            pid: None,
        };

        assert_eq!(
            cached_statusline_output(&cache, 123, 10_500, 1),
            Some("cached status")
        );
    }

    #[test]
    fn invalidates_statusline_cache_when_transcript_changes() {
        let cache = StatuslineCache {
            date: "2026-01-01T00:00:00.000Z".to_string(),
            last_output: "cached status".to_string(),
            last_update_time: 10_000,
            transcript_path: "/tmp/transcript.jsonl".to_string(),
            transcript_mtime: 123,
            is_updating: false,
            pid: None,
        };

        assert_eq!(cached_statusline_output(&cache, 456, 10_500, 1), None);
    }

    #[test]
    fn returns_stale_statusline_cache_while_live_process_is_updating() {
        let cache = StatuslineCache {
            date: "2026-01-01T00:00:00.000Z".to_string(),
            last_output: "stale status".to_string(),
            last_update_time: 10_000,
            transcript_path: "/tmp/transcript.jsonl".to_string(),
            transcript_mtime: 123,
            is_updating: true,
            pid: Some(std::process::id()),
        };

        assert_eq!(
            cached_statusline_output(&cache, 456, 20_000, 1),
            Some("stale status")
        );
    }

    #[test]
    fn resolves_model_label_from_alias_map() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert(
            "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
                .to_string(),
            "claude-opus-4-6".to_string(),
        );

        assert_eq!(
            resolve_model_label(
                &aliases,
                "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
            ),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn falls_back_to_display_name_when_no_alias_matches() {
        let aliases = std::collections::HashMap::new();

        assert_eq!(resolve_model_label(&aliases, "Opus 4.1"), "Opus 4.1");
    }

    #[test]
    fn appends_effort_level_to_model_segment() {
        let effort = HookEffort {
            level: "high".to_string(),
        };

        assert_eq!(
            format_model_segment("Fable 5", Some(&effort)),
            "Fable 5 (high)"
        );
    }

    #[test]
    fn omits_effort_from_model_segment_when_absent_or_empty() {
        let empty = HookEffort {
            level: String::new(),
        };

        assert_eq!(format_model_segment("Fable 5", None), "Fable 5");
        assert_eq!(format_model_segment("Fable 5", Some(&empty)), "Fable 5");
    }

    #[test]
    fn parses_statusline_hook_with_and_without_effort() {
        let with_effort: StatuslineHook = serde_json::from_str(
            r#"{
                "session_id": "session",
                "transcript_path": "/tmp/transcript.jsonl",
                "model": {"id": "claude-fable-5", "display_name": "Fable 5"},
                "effort": {"level": "xhigh"}
            }"#,
        )
        .unwrap();
        let without_effort: StatuslineHook = serde_json::from_str(
            r#"{
                "session_id": "session",
                "transcript_path": "/tmp/transcript.jsonl",
                "model": {"id": "claude-sonnet-4-20250514", "display_name": "Sonnet 4"}
            }"#,
        )
        .unwrap();

        assert_eq!(
            with_effort.effort.map(|effort| effort.level).as_deref(),
            Some("xhigh")
        );
        assert!(without_effort.effort.is_none());
    }
}
