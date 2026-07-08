use std::{fmt, io};

mod adapter;
mod blocks;
mod cli;
mod commands;
mod config;
mod config_schema;
mod cost;
mod date_utils;
mod fast;
mod home;
mod logger;
mod model_aliases;
mod output;
mod path_utils;
mod pricing;
mod progress;
mod project_names;
mod summary;
mod types;
mod utils;

pub(crate) use adapter::claude::{
    chunk_file_indexes_by_size, collect_files_with_extension, collect_usage_files,
    filter_loaded_entries_by_date, load_daily_summaries, load_entries,
};
pub(crate) use adapter::read_files_parallel;
pub(crate) use blocks::{
    block_json, calculate_burn_rate, filter_blocks_by_date, format_remaining_time,
    identify_session_blocks, print_active_block_detail, print_blocks_table, sort_blocks,
};
pub(crate) use cost::{
    calculate_cost, calculate_cost_for_usage, calculate_cost_from_pricing,
    missing_pricing_model_for_candidates, missing_pricing_model_for_token_total,
    missing_pricing_model_for_usage,
};
pub(crate) use date_utils::*;
pub(crate) use logger::{debug_log, log_level};
pub(crate) use output::{
    format_currency, format_models_multiline, format_number, group_project_output, json_float,
    print_json_or_jq, print_missing_pricing_warnings, print_missing_pricing_warnings_for_models,
    print_usage_table, session_summary_json, should_use_compact_layout, summary_json, totals_json,
    wants_json,
};
pub(crate) use project_names::{format_project_name, parse_project_aliases, short_model_name};
pub(crate) use summary::{
    BucketKind, SessionAccumulator, filter_and_sort_summaries, sort_summaries, summarize_by_key,
    summarize_summaries_by_bucket, week_start,
};
pub(crate) use types::*;
pub(crate) use utils::{
    apply_total_token_fallback, json_value_u64, non_empty_json_string, total_usage_tokens,
};

pub(crate) use ccusage_terminal::{Align, Color, SimpleTable};
use ccusage_terminal::{TerminalStyle, terminal_width};
use cli::{AgentCommandArgs, AgentReportKind, Command};
use pricing::{Pricing, PricingMap};

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const DEFAULT_SESSION_DURATION_HOURS: f64 = 5.0;
const DEFAULT_RECENT_DAYS: i64 = 3;
const BLOCKS_WARNING_THRESHOLD: f64 = 0.8;
const USAGE_COMPACT_WIDTH_THRESHOLD: usize = 100;
const BLOCKS_COMPACT_WIDTH_THRESHOLD: usize = 120;

type Result<T> = std::result::Result<T, CliError>;

#[derive(Debug)]
struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

fn cli_error(message: impl Into<String>) -> CliError {
    CliError(message.into())
}

fn terminal_style(shared: &cli::SharedArgs) -> TerminalStyle {
    TerminalStyle {
        color: shared.color,
        log_level: log_level(),
        no_color: shared.no_color,
    }
}

fn color(shared: &cli::SharedArgs, value: impl AsRef<str>, color: Color) -> String {
    ccusage_terminal::color(terminal_style(shared), value, color)
}

fn print_box_title(title: &str, shared: &cli::SharedArgs) {
    ccusage_terminal::print_box_title(title, terminal_style(shared));
}

trait Context<T> {
    fn context(self, message: impl Into<String>) -> Result<T>;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: fmt::Display,
{
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|error| cli_error(format!("{}: {error}", message.into())))
    }
}

fn main() -> Result<()> {
    let cli = cli::parse();
    match cli.command {
        Some(Command::All(args)) => adapter::all::run(args),
        Some(Command::Daily(args)) => commands::run_daily(args),
        Some(Command::Monthly(shared)) => commands::run_bucket(shared, BucketKind::Monthly),
        Some(Command::Weekly(args)) => commands::run_weekly(args),
        Some(Command::Session(args)) => commands::run_session(args),
        Some(Command::Blocks(args)) => commands::run_blocks(args),
        Some(Command::Statusline(args)) => commands::run_statusline(args),
        Some(Command::Codex(args)) => adapter::codex::run(args),
        Some(Command::OpenCode(args)) => adapter::opencode::run(args),
        Some(Command::Amp(args)) => adapter::amp::run(args),
        Some(Command::Droid(args)) => adapter::droid::run(args),
        Some(Command::Codebuff(args)) => adapter::codebuff::run(args),
        Some(Command::Hermes(args)) => adapter::hermes::run(args),
        Some(Command::Pi(args)) => adapter::pi::run(args),
        Some(Command::Goose(args)) => adapter::goose::run(args),
        Some(Command::Kilo(args)) => adapter::kilo::run(args),
        Some(Command::Qwen(args)) => adapter::qwen::run(args),
        Some(Command::Copilot(args)) => adapter::copilot::run(args),
        Some(Command::Gemini(args)) => adapter::gemini::run(args),
        Some(Command::Kimi(args)) => adapter::kimi::run(args),
        Some(Command::OpenClaw(args)) => adapter::openclaw::run(args),
        None => {
            let args = AgentCommandArgs {
                shared: cli.shared,
                kind: AgentReportKind::Daily,
                sections: None,
                by_agent: false,
                pi_path: None,
                open_claw_path: None,
                codex_speed: cli::CodexSpeed::Auto,
            };
            adapter::all::run(args)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, sync::Arc};

    use ccusage_test_support::{EnvVarGuard, fs_fixture};
    use serde_json::json;

    use super::*;
    use crate::{
        cli::{CostMode, SharedArgs, SortOrder, WeekDay},
        cost::tiered_cost,
    };

    #[test]
    fn formats_numbers_with_commas() {
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn calculates_tiered_cost() {
        assert!((tiered_cost(300_000, 3e-6, Some(6e-6)) - 1.2).abs() < f64::EPSILON);
    }

    #[test]
    fn formats_windows_user_project_paths_like_typescript() {
        let aliases = HashMap::new();

        assert_eq!(
            format_project_name(r"C:\Users\phaedrus\Development\ccusage", &aliases),
            "ccusage"
        );
        assert_eq!(
            format_project_name(r"\Users\phaedrus\Development\ccusage", &aliases),
            "ccusage"
        );
    }

    #[test]
    fn gets_week_start() {
        assert_eq!(
            week_start("2024-01-03", WeekDay::Sunday).unwrap(),
            "2023-12-31"
        );
        assert_eq!(
            week_start("2024-01-03", WeekDay::Monday).unwrap(),
            "2024-01-01"
        );
    }

    #[test]
    fn balances_file_chunks_by_size() {
        let fixture = fs_fixture!({
            "large-a.jsonl": "x".repeat(100),
            "small-a.jsonl": "x",
            "small-b.jsonl": "x",
            "large-b.jsonl": "x".repeat(100),
        });
        let files = [
            "large-a.jsonl",
            "small-a.jsonl",
            "small-b.jsonl",
            "large-b.jsonl",
        ]
        .into_iter()
        .map(|name| fixture.path(name))
        .collect::<Vec<_>>();

        let chunks = chunk_file_indexes_by_size(&files, 2);
        assert_eq!(chunks.len(), 2);
        let mut indexes = chunks.iter().flatten().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        assert_eq!(indexes, vec![0, 1, 2, 3]);

        let chunk_sizes = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|index| fs::metadata(&files[*index]).unwrap().len())
                    .sum::<u64>()
            })
            .collect::<Vec<_>>();
        assert_eq!(chunk_sizes, vec![101, 101]);
    }

    #[test]
    fn sorts_usage_files_like_typescript_path_strings() {
        let fixture = fs_fixture!({
            "projects/project1/session-a.jsonl": "",
            "projects/project1/session-a/subagents/agent-a.jsonl": "",
        });
        let parent_file = fixture.path("projects/project1/session-a.jsonl");
        let subagent_file = fixture.path("projects/project1/session-a/subagents/agent-a.jsonl");

        let files = adapter::claude::usage_files(&[fixture.root().to_path_buf()], None);

        assert_eq!(files, vec![parent_file, subagent_file]);
    }

    #[test]
    fn formats_dates_with_timezone() {
        let timestamp = parse_ts_timestamp("2024-08-04T23:30:00.000Z").unwrap();

        assert_eq!(format_date(timestamp, Some("UTC")), "2024-08-04");
        assert_eq!(format_date(timestamp, Some("Asia/Tokyo")), "2024-08-05");
        assert_eq!(format_utc_minute(timestamp), "2024-08-04 23:30");
        assert_eq!(format_utc_second(timestamp), "2024-08-04 23:30:00");
        assert_eq!(format_rfc3339_millis(timestamp), "2024-08-04T23:30:00.000Z");
    }

    #[test]
    fn parses_timestamp_offsets() {
        assert_eq!(
            parse_ts_timestamp("2024-08-05T08:30:00.000+09:00").unwrap(),
            parse_ts_timestamp("2024-08-04T23:30:00.000Z").unwrap()
        );
        assert_eq!(
            parse_ts_timestamp("2024-08-04T16:30:00-07:00").unwrap(),
            parse_ts_timestamp("2024-08-04T23:30:00Z").unwrap()
        );
    }

    #[test]
    fn extracts_compact_jsonl_timestamp() {
        let timestamp = adapter::claude::timestamp_from_line(
            r#"{"timestamp":"2026-05-11T12:34:56.789Z","message":{}}"#,
        )
        .unwrap();

        assert_eq!(format_rfc3339_millis(timestamp), "2026-05-11T12:34:56.789Z");
        assert!(
            adapter::claude::timestamp_from_line(r#"{"timestamp": "2026-05-11T12:34:56.789Z"}"#)
                .is_none()
        );
    }

    #[test]
    fn keeps_most_complete_duplicate_usage_entry() {
        let fixture = fs_fixture!({
            "projects/project1/session1/chat.jsonl": [
                r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"requestId":"req_456","costUSD":0.001}"#,
                r#"{"timestamp":"2025-01-10T10:00:01.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"requestId":"req_456","costUSD":0.01}"#,
            ]
            .join("\n"),
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 250);
        assert_eq!(entries[0].cost, 0.01);
    }

    #[test]
    fn dedupes_usage_entries_by_message_id_without_request_id() {
        let fixture = fs_fixture!({
            "projects/project1/session1/chat.jsonl": [
                r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
                r#"{"timestamp":"2025-01-10T10:00:01.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"costUSD":0.01}"#,
            ]
            .join("\n"),
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 250);
        assert_eq!(entries[0].cost, 0.01);
    }

    #[test]
    fn accepts_projects_directory_in_claude_config_dir() {
        let fixture = fs_fixture!({
            "projects/project1/session1/chat.jsonl": r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.path("projects"));
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project.as_ref(), "project1");
    }

    #[test]
    fn loads_daily_summaries_like_loaded_entry_aggregation() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": [
                r#"{"timestamp":"2025-01-10T09:59:00.000Z","version":"not-semver","message":{"id":"bad","model":"claude-opus-4-6","usage":{"input_tokens":999,"output_tokens":999,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"bad","costUSD":9}"#,
                r#"{"timestamp":"2025-01-10T10:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"requestId":"req_456","costUSD":0.001}"#,
                r#"{"timestamp":"2025-01-10T10:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"requestId":"req_456","costUSD":0.01}"#,
                r#"{"timestamp":"2025-01-11T11:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_789","model":"<synthetic>","usage":{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_789","costUSD":0.02}"#,
            ]
            .join("\n"),
            "projects/project-b/session-b/chat.jsonl": r#"{"timestamp":"2025-01-10T12:00:00.000Z","version":"1.2.3","sessionId":"session-b","message":{"id":"msg_b","model":"claude-sonnet-4-20250514","usage":{"input_tokens":20,"output_tokens":30,"cache_creation_input_tokens":4,"cache_read_input_tokens":2}},"requestId":"req_b","costUSD":0.04}"#,
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, None).unwrap();
        let daily = load_daily_summaries(&shared, None, false).unwrap();
        let grouped_daily = load_daily_summaries(&shared, None, true).unwrap();

        let expected_daily = summarize_by_key(
            &entries,
            |entry| entry.date.clone(),
            |key| (key.to_string(), None),
        )
        .unwrap();
        assert_eq!(
            daily.iter().map(summary_json).collect::<Vec<_>>(),
            expected_daily.iter().map(summary_json).collect::<Vec<_>>()
        );

        let expected_grouped_daily = summarize_by_key(
            &entries,
            |entry| format!("{}\0{}", entry.date, entry.project),
            |key| {
                let mut parts = key.split('\0');
                (
                    parts.next().unwrap_or_default().to_string(),
                    parts.next().map(str::to_string),
                )
            },
        )
        .unwrap();
        assert_eq!(
            grouped_daily.iter().map(summary_json).collect::<Vec<_>>(),
            expected_grouped_daily
                .iter()
                .map(summary_json)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn keeps_nested_agent_progress_in_daily_model_order() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-03-10T06:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_fast","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":10,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"speed":"fast"}},"requestId":"req_fast","costUSD":0.03}"#,
                r#"{"sessionId":"session-a","version":"1.2.3","type":"progress","data":{"message":{"type":"assistant","timestamp":"2026-03-10T06:00:01.000Z","message":{"model":"claude-haiku-4-5-20251001","id":"msg_haiku","type":"message","role":"assistant","content":[],"usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","uuid":"nested"}},"timestamp":"2026-03-10T06:00:01.001Z"}"#,
                r#"{"timestamp":"2026-03-10T06:00:02.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_opus","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":30,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_opus","costUSD":0.09}"#,
            ]
            .join("\n"),
            "projects/project-a/session-a/subagents/agent-a.jsonl": r#"{"timestamp":"2026-03-10T06:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_haiku","model":"claude-haiku-4-5-20251001","role":"assistant","usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","costUSD":0.06}"#,
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let daily = load_daily_summaries(&shared, None, false).unwrap();

        assert_eq!(daily.len(), 1);
        assert_eq!(
            daily[0].models_used,
            vec![
                "claude-opus-4-6-fast",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-6",
            ]
        );
        assert_eq!(daily[0].total_cost, 0.18);
    }

    #[test]
    fn uses_direct_subagent_cost_for_duplicate_daily_agent_progress() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": r#"{"sessionId":"session-a","version":"1.2.3","type":"progress","data":{"message":{"type":"assistant","timestamp":"2026-03-10T06:00:01.000Z","message":{"model":"claude-haiku-4-5-20251001","id":"msg_haiku","type":"message","role":"assistant","content":[],"usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","uuid":"nested"}},"timestamp":"2026-03-10T06:00:01.001Z"}"#,
            "projects/project-a/session-a/subagents/agent-a.jsonl": r#"{"timestamp":"2026-03-10T06:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_haiku","model":"claude-haiku-4-5-20251001","role":"assistant","usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","costUSD":0.06}"#,
        });

        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, None).unwrap();
        let daily = load_daily_summaries(&shared, None, false).unwrap();

        let expected_daily = summarize_by_key(
            &entries,
            |entry| entry.date.clone(),
            |key| (key.to_string(), None),
        )
        .unwrap();
        assert_eq!(
            daily.iter().map(summary_json).collect::<Vec<_>>(),
            expected_daily.iter().map(summary_json).collect::<Vec<_>>()
        );
    }

    #[test]
    fn loads_codex_token_count_events() {
        let fixture = fs_fixture!({
            "sessions/codex-session.jsonl": [
                r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
                r#"{"timestamp":"2026-01-02T00:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150},"model":"gpt-5"}}}"#,
            ]
            .join("\n"),
        });

        let events =
            adapter::codex::load_codex_events_from_directory(&fixture.path("sessions"), true)
                .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "codex-session");
        assert_eq!(events[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].cached_input_tokens, 10);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].reasoning_output_tokens, 0);
        assert_eq!(events[0].total_tokens, 150);
    }

    #[test]
    fn loads_codex_token_count_events_in_parallel() {
        let fixture = fs_fixture!({
            "sessions/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5"}}"#,
                r#"{"timestamp":"2026-01-02T00:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150},"model":"gpt-5"}}}"#,
            ]
            .join("\n"),
            "sessions/session-b.jsonl": [
                r#"{"timestamp":"2026-01-02T00:01:00.000Z","type":"turn_context","payload":{"model":"gpt-5-mini"}}"#,
                r#"{"timestamp":"2026-01-02T00:01:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":40,"cached_input_tokens":4,"output_tokens":20,"reasoning_output_tokens":2,"total_tokens":62},"model":"gpt-5-mini"}}}"#,
            ]
            .join("\n"),
        });

        let single_thread_events =
            adapter::codex::load_codex_events_from_directory(&fixture.path("sessions"), true)
                .unwrap();
        let parallel_events =
            adapter::codex::load_codex_events_from_directory(&fixture.path("sessions"), false)
                .unwrap();

        assert_eq!(parallel_events.len(), 2);
        assert_eq!(parallel_events, single_thread_events);
        assert_eq!(parallel_events[0].session_id, "session-a");
        assert_eq!(parallel_events[1].session_id, "session-b");
    }

    #[test]
    fn builds_codex_daily_json_report() {
        let pricing = PricingMap::load_embedded();
        let events = vec![CodexTokenUsageEvent {
            session_id: "codex-session".to_string(),
            timestamp: "2026-01-02T00:00:01.000Z".to_string(),
            model: Some("gpt-5".to_string()),
            input_tokens: 100,
            cached_input_tokens: 10,
            output_tokens: 50,
            reasoning_output_tokens: 0,
            total_tokens: 150,
            is_fallback_model: false,
        }];

        let report = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            cli::CodexSpeed::Standard,
        )
        .unwrap();

        assert_eq!(report["daily"][0]["date"], "2026-01-02");
        assert_eq!(report["daily"][0]["inputTokens"], 90);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 0);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["reasoningOutputTokens"], 0);
        assert_eq!(report["daily"][0]["totalTokens"], 150);
        assert_eq!(report["daily"][0]["models"]["gpt-5"]["inputTokens"], 90);
        assert_eq!(
            report["daily"][0]["models"]["gpt-5"]["cacheCreationTokens"],
            0
        );
        assert_eq!(report["daily"][0]["models"]["gpt-5"]["cacheReadTokens"], 10);
        assert_eq!(report["daily"][0]["costUSD"], json!(0.00061375));
        assert_eq!(report["totals"]["costUSD"], json!(0.00061375));
    }

    #[test]
    fn prices_codex_versioned_models_like_typescript_adapter() {
        let pricing = PricingMap::load_embedded();
        let events = vec![CodexTokenUsageEvent {
            session_id: "codex-session".to_string(),
            timestamp: "2026-01-02T00:00:01.000Z".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            input_tokens: 120,
            cached_input_tokens: 30,
            output_tokens: 11,
            reasoning_output_tokens: 3,
            total_tokens: 131,
            is_fallback_model: false,
        }];

        let report = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            None,
            &pricing,
            cli::CodexSpeed::Standard,
        )
        .unwrap();

        assert_eq!(report["daily"][0]["costUSD"], json!(0.00031675));
    }

    #[test]
    fn applies_codex_fast_speed_multiplier_to_costs() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "cache_read_input_token_cost": 0.0000005,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
        );
        let events = vec![CodexTokenUsageEvent {
            session_id: "codex-session".to_string(),
            timestamp: "2026-01-02T00:00:01.000Z".to_string(),
            model: Some("gpt-test".to_string()),
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 15,
            is_fallback_model: false,
        }];

        let standard = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            None,
            &pricing,
            cli::CodexSpeed::Standard,
        )
        .unwrap();
        let fast = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            None,
            &pricing,
            cli::CodexSpeed::Fast,
        )
        .unwrap();

        assert_eq!(standard["daily"][0]["costUSD"], json!(0.000019));
        assert_eq!(fast["daily"][0]["costUSD"], json!(0.000038));
    }

    #[test]
    fn prices_codex_gpt_5_4_like_typescript_adapter() {
        let pricing = PricingMap::load_embedded();
        let events = vec![CodexTokenUsageEvent {
            session_id: "codex-session".to_string(),
            timestamp: "2026-03-18T00:00:01.000Z".to_string(),
            model: Some("gpt-5.4".to_string()),
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 10,
            reasoning_output_tokens: 0,
            total_tokens: 110,
            is_fallback_model: false,
        }];

        let standard = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            None,
            &pricing,
            cli::CodexSpeed::Standard,
        )
        .unwrap();
        let fast = adapter::codex::report_json(
            &events,
            AgentReportKind::Daily,
            None,
            &pricing,
            cli::CodexSpeed::Fast,
        )
        .unwrap();

        assert!((standard["daily"][0]["costUSD"].as_f64().unwrap() - 0.00031).abs() < f64::EPSILON);
        assert!((fast["daily"][0]["costUSD"].as_f64().unwrap() - 0.00062).abs() < f64::EPSILON);
    }

    #[test]
    fn loads_amp_thread_usage_events() {
        let fixture = fs_fixture!({
            "threads/thread.json": r#"{"id":"thread-a","messages":[{"role":"assistant","messageId":2,"usage":{"cacheCreationInputTokens":20,"cacheReadInputTokens":10}}],"usageLedger":{"events":[{"id":"event-a","timestamp":"2026-05-01T01:02:03.000Z","model":"claude-sonnet-4-20250514","credits":1.25,"tokens":{"input":100,"output":50},"toMessageId":2}]}}"#,
        });

        let entries = adapter::amp::read_thread_file(
            &fixture.path("threads/thread.json"),
            parse_tz(Some("UTC")).as_ref(),
            CostMode::Display,
            None,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-05-01");
        assert_eq!(entries[0].session_id.as_ref(), "thread-a");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].credits, Some(1.25));
    }

    #[test]
    fn builds_amp_daily_json_report() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("thread-a".to_string()),
                timestamp: "2026-05-01T01:02:03.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: 20,
                        cache_read_input_tokens: 10,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some("event-a".to_string()),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: parse_ts_timestamp("2026-05-01T01:02:03.000Z").unwrap(),
            date: "2026-05-01".to_string(),
            project: Arc::from("amp"),
            session_id: Arc::from("thread-a"),
            project_path: Arc::from("Amp"),
            cost: 0.02,
            extra_total_tokens: 0,
            credits: Some(1.25),
            message_count: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        };

        let rows = adapter::amp::summarize_entries(&[entry], AgentReportKind::Daily).unwrap();
        let report = adapter::amp::report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["date"], "2026-05-01");
        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
        assert_eq!(report["daily"][0]["credits"], json!(1.25));
        assert_eq!(report["daily"][0]["totalCost"], json!(0.02));
        assert_eq!(report["totals"]["credits"], json!(1.25));
    }

    #[test]
    fn loads_pi_agent_jsonl_usage_entries() {
        let fixture = fs_fixture!({
            "sessions/project-a/prefix_session-a.jsonl": [
                r#"{"type":"message","timestamp":"2026-04-22T01:02:02.000Z","message":{"role":"user","usage":{"input":999,"output":999}}}"#,
                r#"{"type":"message","timestamp":"2026-04-22T01:02:03.000Z","message":{"role":"assistant","model":"gpt-5.4","usage":{"input":100,"output":50,"cacheRead":10,"cacheWrite":20,"totalTokens":180,"cost":{"total":0.05}}}}"#,
            ]
            .join("\n"),
        });

        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let sessions_path = fixture.path("sessions");
        let entries = adapter::pi::load_entries(&shared, sessions_path.to_str(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-04-22");
        assert_eq!(entries[0].project.as_ref(), "project-a");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("[pi] gpt-5.4"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].cost, 0.05);
    }

    #[test]
    fn builds_pi_daily_json_report() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-04-22T01:02:03.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: 20,
                        cache_read_input_tokens: 10,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("[pi] gpt-5.4".to_string()),
                    id: None,
                },
                cost_usd: Some(0.05),
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: parse_ts_timestamp("2026-04-22T01:02:03.000Z").unwrap(),
            date: "2026-04-22".to_string(),
            project: Arc::from("project-a"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("project-a"),
            cost: 0.05,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("[pi] gpt-5.4".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        };

        let rows = adapter::pi::summarize_entries(&[entry], AgentReportKind::Daily).unwrap();
        let report = adapter::pi::report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["date"], "2026-04-22");
        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
        assert_eq!(report["daily"][0]["totalCost"], json!(0.05));
        assert_eq!(report["daily"][0]["modelsUsed"], json!(["[pi] gpt-5.4"]));
        assert_eq!(
            report["daily"][0]["modelBreakdowns"],
            json!([{
                "modelName": "[pi] gpt-5.4",
                "inputTokens": 100,
                "outputTokens": 50,
                "cacheCreationTokens": 20,
                "cacheReadTokens": 10,
                "cost": 0.05
            }])
        );
    }

    #[test]
    fn builds_opencode_daily_json_report() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("opencode-session".to_string()),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: 20,
                        cache_read_input_tokens: 10,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some("msg-1".to_string()),
                },
                cost_usd: Some(0.02),
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: parse_ts_timestamp("2026-01-02T00:00:00.000Z").unwrap(),
            date: "2026-01-02".to_string(),
            project: Arc::from("opencode"),
            session_id: Arc::from("opencode-session"),
            project_path: Arc::from("OpenCode"),
            cost: 0.02,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        };

        let report =
            adapter::opencode::report_json(&[entry], AgentReportKind::Daily, &SortOrder::Asc)
                .unwrap();

        assert_eq!(report["daily"][0]["date"], "2026-01-02");
        assert_eq!(report["daily"][0]["inputTokens"], 100);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 20);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 10);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
        assert_eq!(report["daily"][0]["totalCost"], json!(0.02));
        assert_eq!(
            report["daily"][0]["modelsUsed"],
            json!(["claude-sonnet-4-20250514"])
        );
    }

    #[test]
    fn extracts_usage_limit_reset_time_from_raw_line() {
        let line = r#"{"timestamp":"2025-01-10T10:00:00.000Z","isApiErrorMessage":true,"message":{"content":[{"text":"Claude AI usage limit reached|1736503200 remaining"}],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let reset_time =
            adapter::claude::usage_limit_reset_time_from_line(line, Some(true)).unwrap();

        assert_eq!(
            format_rfc3339_millis(reset_time),
            "2025-01-10T10:00:00.000Z"
        );
        assert!(adapter::claude::usage_limit_reset_time_from_line(line, Some(false)).is_none());
        assert!(
            adapter::claude::usage_limit_reset_time_from_line(
                r#"{"message":{"content":[{"text":"Claude AI usage limit reached|0"}]}}"#,
                Some(true)
            )
            .is_none()
        );
    }
}
