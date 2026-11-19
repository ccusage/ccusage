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
        println!("{output}");
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
    use std::assert_matches;
    use std::ffi::OsString;

    use ccusage_cli_parser::parse_from_with_config;
    use ccusage_config::ConfigContext;
    use ccusage_core::cli::{Command, PricingOverride};
    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;

    #[test]
    fn transcript_context_uses_the_latest_usage_with_escaped_json_keys() {
        let fixture = ccusage_test_support::Fixture::new();
        let transcript = fixture.write_file(
            "transcript.jsonl",
            concat!(
                r#"{"type":"assistant","message":{"usage":{"input_tokens":10}}}"#, "\n",
                r#"{"t\u0079pe":"assist\u0061nt","message":{"usage":{"input\u005ftokens":20,"cache_read_input_tokens":3}}}"#, "\n",
                r#"{"type":"assistant","message":{"usage":{"input_tokens":"invalid"}}}"#,
            ),
        );

        let context = calculate_context_tokens_from_transcript(
            &transcript,
            None,
            true,
            &SharedArgs::default(),
        )
        .unwrap();

        assert_eq!(context.total_input_tokens, 23);
    }

    #[test]
    fn a_cached_pid_is_alive_only_when_it_names_one_running_process() {
        assert!(process_is_alive(std::process::id()));
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            assert!(!process_is_alive(pid), "pid {pid}");
        }
    }

    #[test]
    fn statusline_invocation_passes_config_pricing_overrides_to_cost_loading() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "ccusage.json": r#"{
                "defaults": {
                    "pricingOverrides": {
                        "third-party-model": {
                            "inputCostPerToken": 0.000001,
                            "outputCostPerToken": 0.000002
                        }
                    }
                },
                "commands": {
                    "statusline": {
                        "pricingOverrides": {
                            "third-party-model": {
                                "outputCostPerToken": 0.000003
                            }
                        }
                    }
                }
            }"#,
            "projects/project/session.jsonl": r#"{"timestamp":"2020-01-01T00:00:00.000Z","sessionId":"session","message":{"id":"message","model":"third-party-model","usage":{"input_tokens":100000,"output_tokens":100000}}}"#,
        });
        Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
            let config_path = fixture.path("ccusage.json");
            let config_path = config_path.to_string_lossy().into_owned();
            let config_args = vec![
                "statusline".to_string(),
                "--config".to_string(),
                config_path.clone(),
            ];
            let config = ConfigContext::from_args(&config_args);
            let command = parse_from_with_config(
                [
                    "ccusage",
                    "statusline",
                    "--config",
                    config_path.as_str(),
                    "--cost-source",
                    "ccusage",
                ]
                .into_iter()
                .map(OsString::from),
                &config,
                env!("CARGO_PKG_VERSION"),
            )
            .unwrap();
            let Command::Statusline(args) = command else {
                panic!("expected statusline command");
            };

            let shared = statusline_shared(&args);
            let pricing = &shared.pricing_overrides["third-party-model"];
            assert_eq!(pricing.input_cost_per_token, Some(0.000001));
            assert_eq!(pricing.output_cost_per_token, Some(0.000003));

            let hook = StatuslineHook {
                session_id: "session".to_string(),
                transcript_path: fixture
                    .path("transcript.jsonl")
                    .to_string_lossy()
                    .into_owned(),
                model: HookModel {
                    id: Some("third-party-model".to_string()),
                    display_name: "Third-party model".to_string(),
                },
                cost: None,
                context_window: Some(HookContext {
                    total_input_tokens: 100_000,
                    context_window_size: 200_000,
                }),
                effort: None,
            };
            let output =
                render_statusline(&hook, &args, &shared, &Diagnostics::new("claude")).unwrap();
            assert!(output.contains("💰 $0.40 session"), "{output}");
        })
    }

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
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "transcript.jsonl": r#"{"type":"assistant","message":{"usage":{"input_tokens":1000}}}"#,
        });

        let mut shared = SharedArgs::default();
        shared.pricing_overrides.insert(
            "test-model-context-limit".to_string(),
            PricingOverride {
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
    fn pricing_92_rate_only_override_keeps_the_statusline_context_limit() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "transcript.jsonl": r#"{"type":"assistant","message":{"usage":{"input_tokens":1000}}}"#,
        });
        let model = "claude-opus-4-8";
        let mut shared = SharedArgs::default();
        let base = calculate_context_tokens_from_transcript(
            &fixture.path("transcript.jsonl"),
            Some(model),
            true,
            &shared,
        )
        .unwrap();
        shared.pricing_overrides.insert(
            model.into(),
            PricingOverride {
                input_cost_per_token: Some(50e-6),
                output_cost_per_token: Some(50e-6),
                ..Default::default()
            },
        );
        let overridden = calculate_context_tokens_from_transcript(
            &fixture.path("transcript.jsonl"),
            Some(model),
            true,
            &shared,
        )
        .unwrap();
        assert_eq!(base.context_window_size, 1_000_000);
        assert_eq!(overridden.context_window_size, base.context_window_size);
    }

    #[test]
    fn colors_statusline_context_percentage_by_threshold() {
        let shared = SharedArgs {
            color: ccusage_core::cli::ColorChoice::Always,
            ..SharedArgs::default()
        };
        let args = StatuslineArgs::default();

        assert_matches!(statusline_context_color(60, &args), Color::Yellow);
        assert!(format_statusline_context(120_000, 200_000, &args, &shared).contains("60%"));
    }

    #[test]
    fn builds_statusline_today_filter_from_timezone() {
        let args = StatuslineArgs {
            timezone: Some("Asia/Tokyo".parse().unwrap()),
            ..StatuslineArgs::default()
        };
        let mut shared = SharedArgs {
            offline: true,
            ..SharedArgs::default()
        };
        shared.pricing_overrides.insert(
            "statusline-model".to_string(),
            PricingOverride {
                input_cost_per_token: Some(1e-6),
                ..Default::default()
            },
        );
        let now = TimestampMs::from_millis(1_779_380_820_000);

        let today_shared = statusline_today_shared(&args, &shared, now);

        assert_eq!(
            today_shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260522")
        );
        assert_eq!(
            today_shared.until.map(|date| date.to_string()).as_deref(),
            Some("20260522")
        );
        assert_eq!(
            today_shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("Asia/Tokyo")
        );
        assert!(
            today_shared
                .pricing_overrides
                .contains_key("statusline-model")
        );
    }

    #[test]
    fn bounds_statusline_block_load_behind_the_session_window() {
        let args = StatuslineArgs {
            timezone: Some("Asia/Tokyo".parse().unwrap()),
            ..StatuslineArgs::default()
        };
        let shared = SharedArgs {
            offline: true,
            ..SharedArgs::default()
        };
        let now = TimestampMs::from_millis(1_779_380_820_000);

        let block_shared = statusline_block_shared(&args, &shared, now, 1);

        assert_eq!(
            block_shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260520")
        );
        assert_eq!(
            statusline_block_shared(&args, &shared, now, 7)
                .since
                .map(|date| date.to_string())
                .as_deref(),
            Some("20260514")
        );
        assert_eq!(block_shared.until, None);
        assert_eq!(
            block_shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("Asia/Tokyo")
        );
        assert!(block_shared.offline);
    }

    #[test]
    fn anchors_statusline_blocks_after_the_latest_long_pause() {
        const HOUR: i64 = 60 * 60 * 1000;
        let now = TimestampMs::from_millis(1_779_380_820_000);
        let at = |hours_ago: i64| TimestampMs::from_millis(now.as_millis() - hours_ago * HOUR);
        let session = 5 * HOUR;

        assert_eq!(
            statusline_block_anchor(&[], now, session),
            BlockAnchor::NoActiveBlock
        );
        assert_eq!(
            statusline_block_anchor(&[at(30), at(6)], now, session),
            BlockAnchor::NoActiveBlock
        );
        assert_eq!(
            statusline_block_anchor(&[at(40), at(30), at(20), at(18), at(1)], now, session),
            BlockAnchor::From(at(1))
        );
        assert_eq!(
            statusline_block_anchor(
                &[at(30), at(20), at(16), at(12), at(8), at(4)],
                now,
                session
            ),
            BlockAnchor::From(at(20))
        );
        assert_eq!(
            statusline_block_anchor(&[at(9), at(4), at(1)], now, session),
            BlockAnchor::NeedsMoreHistory
        );
        assert_eq!(
            statusline_block_anchor(&[at(1), at(-8)], now, session),
            BlockAnchor::NeedsFullHistory
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
