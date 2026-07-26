use std::ffi::OsString;

use serde_json::{Value, json};

use crate::help::{help_text, help_text_for_args};
use crate::*;

fn parse(args: &[&str]) -> Cli {
    Cli::parse_from(args.iter().map(OsString::from)).unwrap()
}

fn parse_with_config(args: &[&str], config: &dyn CliConfig) -> Cli {
    Cli::parse_from_with_config(
        args.iter().map(OsString::from),
        config,
        5.0,
        env!("CARGO_PKG_VERSION"),
    )
    .unwrap()
}

fn parse_error(args: &[&str]) -> String {
    match Cli::parse_from(args.iter().map(OsString::from)) {
        Ok(_) => panic!("expected parse error"),
        Err(error) => error,
    }
}

#[derive(Default)]
struct TestConfig {
    shared_json: Option<bool>,
    shared_order: Option<SortOrder>,
    shared_since: Option<&'static str>,
    shared_timezone: Option<&'static str>,
    shared_compact: Option<bool>,
    weekly_start: Option<WeekDay>,
    blocks_active: Option<bool>,
    blocks_token_limit: Option<&'static str>,
    blocks_session_length: Option<f64>,
    statusline_visual_burn_rate: Option<VisualBurnRate>,
    statusline_cost_source: Option<CostSource>,
    statusline_refresh_interval: Option<u64>,
    codex_speed: Option<CodexSpeed>,
    pi_path: Option<&'static str>,
    open_claw_path: Option<&'static str>,
}

impl CliConfig for TestConfig {
    fn apply_shared(&self, shared: &mut SharedArgs) {
        if let Some(json) = self.shared_json {
            shared.json = json;
        }
        if let Some(order) = self.shared_order {
            shared.order = order;
        }
        if let Some(since) = self.shared_since {
            shared.since = Some(since.to_string());
        }
        if let Some(timezone) = self.shared_timezone {
            shared.timezone = Some(timezone.to_string());
        }
        if let Some(compact) = self.shared_compact {
            shared.compact = compact;
        }
    }

    fn apply_weekly_args(&self, args: &mut WeeklyArgs) {
        if let Some(start_of_week) = self.weekly_start {
            args.start_of_week = start_of_week;
        }
    }

    fn apply_blocks_args(&self, args: &mut BlocksArgs) {
        if let Some(active) = self.blocks_active {
            args.active = active;
        }
        if let Some(token_limit) = self.blocks_token_limit {
            args.token_limit = Some(token_limit.to_string());
        }
        if let Some(session_length) = self.blocks_session_length {
            args.session_length = session_length;
        }
    }

    fn apply_statusline_args(&self, args: &mut StatuslineArgs) {
        if let Some(visual_burn_rate) = self.statusline_visual_burn_rate {
            args.visual_burn_rate = visual_burn_rate;
        }
        if let Some(cost_source) = self.statusline_cost_source {
            args.cost_source = cost_source;
        }
        if let Some(refresh_interval) = self.statusline_refresh_interval {
            args.refresh_interval = refresh_interval;
        }
    }

    fn apply_agent_args(
        &self,
        codex_speed: &mut CodexSpeed,
        pi_path: Option<&mut Option<String>>,
        open_claw_path: Option<&mut Option<String>>,
    ) {
        if let Some(speed) = self.codex_speed {
            *codex_speed = speed;
        }
        if let (Some(path), Some(pi_path)) = (self.pi_path, pi_path) {
            *pi_path = Some(path.to_string());
        }
        if let (Some(path), Some(open_claw_path)) = (self.open_claw_path, open_claw_path) {
            *open_claw_path = Some(path.to_string());
        }
    }
}

fn shared_snapshot(shared: &SharedArgs) -> Value {
    json!({
        "since": shared.since.as_deref(),
        "until": shared.until.as_deref(),
        "json": shared.json,
        "mode": format!("{:?}", shared.mode),
        "debug": shared.debug,
        "debugSamples": shared.debug_samples,
        "order": format!("{:?}", shared.order),
        "breakdown": shared.breakdown,
        "offline": shared.offline,
        "noOffline": shared.no_offline,
        "color": shared.color,
        "noColor": shared.no_color,
        "timezone": shared.timezone.as_deref(),
        "jq": shared.jq.as_deref(),
        "config": shared.config.as_ref().map(|path| path.to_string_lossy().to_string()),
        "compact": shared.compact,
        "singleThread": shared.single_thread,
    })
}

fn cli_snapshot(cli: Cli) -> Value {
    json!({
        "shared": shared_snapshot(&cli.shared),
        "command": command_snapshot(cli.command),
    })
}

fn command_snapshot(command: Option<Command>) -> Value {
    match command {
        None => Value::Null,
        Some(Command::All(args)) => agent_command_snapshot("all", args),
        Some(Command::Daily(args)) => json!({
            "type": "daily",
            "shared": shared_snapshot(&args.shared),
            "instances": args.instances,
            "project": args.project,
            "projectAliases": args.project_aliases,
        }),
        Some(Command::Monthly(shared)) => json!({
            "type": "monthly",
            "shared": shared_snapshot(&shared),
        }),
        Some(Command::Weekly(args)) => json!({
            "type": "weekly",
            "shared": shared_snapshot(&args.shared),
            "startOfWeek": format!("{:?}", args.start_of_week),
        }),
        Some(Command::Session(args)) => json!({
            "type": "session",
            "shared": shared_snapshot(&args.shared),
            "id": args.id,
        }),
        Some(Command::Blocks(args)) => json!({
            "type": "blocks",
            "shared": shared_snapshot(&args.shared),
            "active": args.active,
            "recent": args.recent,
            "tokenLimit": args.token_limit,
            "sessionLength": args.session_length,
        }),
        Some(Command::Statusline(args)) => json!({
            "type": "statusline",
            "offline": args.offline,
            "noOffline": args.no_offline,
            "visualBurnRate": format!("{:?}", args.visual_burn_rate),
            "costSource": format!("{:?}", args.cost_source),
            "cache": args.cache,
            "noCache": args.no_cache,
            "refreshInterval": args.refresh_interval,
            "contextLowThreshold": args.context_low_threshold,
            "contextMediumThreshold": args.context_medium_threshold,
            "config": args.config.as_ref().map(|path| path.to_string_lossy().to_string()),
            "debug": args.debug,
        }),
        Some(Command::Codex(args)) => agent_command_snapshot("codex", args),
        Some(Command::OpenCode(args)) => agent_command_snapshot("opencode", args),
        Some(Command::Amp(args)) => agent_command_snapshot("amp", args),
        Some(Command::Droid(args)) => agent_command_snapshot("droid", args),
        Some(Command::Codebuff(args)) => agent_command_snapshot("codebuff", args),
        Some(Command::Hermes(args)) => agent_command_snapshot("hermes", args),
        Some(Command::Pi(args)) => agent_command_snapshot("pi", args),
        Some(Command::Goose(args)) => agent_command_snapshot("goose", args),
        Some(Command::Kilo(args)) => agent_command_snapshot("kilo", args),
        Some(Command::Copilot(args)) => agent_command_snapshot("copilot", args),
        Some(Command::Gemini(args)) => agent_command_snapshot("gemini", args),
        Some(Command::Kimi(args)) => agent_command_snapshot("kimi", args),
        Some(Command::Qwen(args)) => agent_command_snapshot("qwen", args),
        Some(Command::OpenClaw(args)) => agent_command_snapshot("openclaw", args),
    }
}

fn agent_command_snapshot(agent: &str, args: AgentCommandArgs) -> Value {
    json!({
        "type": agent,
        "shared": shared_snapshot(&args.shared),
        "kind": format!("{:?}", args.kind),
        "sections": args.sections.map(|sections| sections
            .into_iter()
            .map(|section| format!("{section:?}"))
            .collect::<Vec<_>>()),
        "byAgent": args.by_agent,
        "piPath": args.pi_path,
        "openClawPath": args.open_claw_path,
        "codexSpeed": format!("{:?}", args.codex_speed),
    })
}

#[test]
fn parses_root_daily_as_all_agent_report() {
    let cli = parse(&["ccusage", "daily", "--json", "--since", "20260102"]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
}

#[test]
fn parses_unified_sections_and_by_agent_flags() {
    let cli = parse(&[
        "ccusage",
        "daily",
        "--json",
        "--sections",
        "monthly,session",
        "--by-agent",
    ]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Monthly, AgentReportKind::Session][..])
    );
    assert!(args.by_agent);
}

#[test]
fn parses_root_sections_and_by_agent_flags_without_daily_token() {
    let cli = parse(&[
        "ccusage",
        "--json",
        "--sections",
        "monthly,session",
        "--by-agent",
    ]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Monthly, AgentReportKind::Session][..])
    );
    assert!(args.by_agent);
}

#[test]
fn parses_inline_unified_sections_value() {
    let cli = parse(&["ccusage", "daily", "--sections=daily,monthly"]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily, AgentReportKind::Monthly][..])
    );
}

#[test]
fn ignores_empty_unified_section_tokens() {
    let cli = parse(&["ccusage", "daily", "--sections", "daily,,monthly,"]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily, AgentReportKind::Monthly][..])
    );
}

#[test]
fn dedupes_unified_section_tokens_preserving_first_occurrence() {
    let cli = parse(&["ccusage", "daily", "--sections", "daily,daily,monthly"]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily, AgentReportKind::Monthly][..])
    );
}

#[test]
fn parses_top_level_session_sections_as_all_agent_report_without_id() {
    let cli = parse(&[
        "ccusage",
        "session",
        "--sections",
        "daily,weekly",
        "--by-agent",
    ]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily, AgentReportKind::Weekly][..])
    );
    assert!(args.by_agent);
}

#[test]
fn rejects_sections_and_by_agent_with_top_level_session_id() {
    let sections_error = parse_error(&["ccusage", "session", "--id", "abc", "--sections", "daily"]);
    assert_eq!(
        sections_error,
        "The --sections and --by-agent options cannot be used with session --id."
    );

    let by_agent_error = parse_error(&["ccusage", "session", "--id", "abc", "--by-agent"]);
    assert_eq!(
        by_agent_error,
        "The --sections and --by-agent options cannot be used with session --id."
    );
}

#[test]
fn rejects_unified_only_flags_on_per_agent_subcommands() {
    let sections_error = parse_error(&["ccusage", "codex", "daily", "--sections", "daily"]);
    assert_eq!(sections_error, "Unknown codex option '--sections'");

    let by_agent_error = parse_error(&["ccusage", "codex", "daily", "--by-agent"]);
    assert_eq!(by_agent_error, "Unknown codex option '--by-agent'");
}

#[test]
fn rejects_invalid_unified_section_token() {
    let error = parse_error(&["ccusage", "daily", "--sections", "daily,yearly"]);

    assert_eq!(
        error,
        "Invalid --sections value 'yearly'. Expected one or more of: daily, weekly, monthly, session."
    );
}

#[test]
fn preserves_shared_config_defaults_with_unified_sections() {
    let config = TestConfig {
        shared_json: Some(true),
        shared_since: Some("20260102"),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "monthly", "--sections", "daily"], &config);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily][..])
    );
}

#[test]
fn parses_root_session_as_all_agent_report_without_id() {
    let cli = parse(&["ccusage", "session", "--json"]);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn applies_config_defaults_and_command_options_before_cli_options() {
    let config = TestConfig {
        shared_json: Some(true),
        shared_order: Some(SortOrder::Desc),
        shared_since: Some("20260102"),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "daily", "--order", "asc"], &config);
    let Some(Command::All(args)) = cli.command else {
        panic!("expected all-agent command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
    assert_eq!(args.shared.order, SortOrder::Asc);
}

#[test]
fn applies_agent_namespace_config_to_codex_speed() {
    let config = TestConfig {
        codex_speed: Some(CodexSpeed::Fast),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "codex", "daily"], &config);
    let Some(Command::Codex(args)) = cli.command else {
        panic!("expected codex command");
    };
    assert_eq!(args.codex_speed, CodexSpeed::Fast);
}

#[test]
fn applies_config_file_passed_after_agent_command() {
    let config = TestConfig {
        shared_json: Some(true),
        shared_timezone: Some("Asia/Tokyo"),
        shared_since: Some("20260101"),
        codex_speed: Some(CodexSpeed::Standard),
        ..TestConfig::default()
    };

    let cli = parse_with_config(
        &[
            "ccusage",
            "codex",
            "monthly",
            "--config",
            "/tmp/ccusage.json",
        ],
        &config,
    );
    let Some(Command::Codex(args)) = cli.command else {
        panic!("expected codex command");
    };
    assert_eq!(args.kind, AgentReportKind::Monthly);
    assert!(args.shared.json);
    assert_eq!(args.shared.timezone.as_deref(), Some("Asia/Tokyo"));
    assert_eq!(args.shared.since.as_deref(), Some("20260101"));
    assert_eq!(args.codex_speed, CodexSpeed::Standard);
}

#[test]
fn applies_schema_documented_config_file_options() {
    let config = TestConfig {
        shared_json: Some(true),
        shared_compact: Some(true),
        weekly_start: Some(WeekDay::Monday),
        blocks_active: Some(true),
        blocks_token_limit: Some("500000"),
        blocks_session_length: Some(6.0),
        statusline_visual_burn_rate: Some(VisualBurnRate::EmojiText),
        statusline_cost_source: Some(CostSource::Both),
        statusline_refresh_interval: Some(3),
        pi_path: Some("/tmp/pi-sessions"),
        open_claw_path: Some("/tmp/openclaw"),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "claude", "weekly"], &config);
    let Some(Command::Weekly(args)) = cli.command else {
        panic!("expected weekly command");
    };
    assert!(args.shared.json);
    assert!(args.shared.compact);
    assert_eq!(args.start_of_week, WeekDay::Monday);

    let cli = parse_with_config(&["ccusage", "claude", "blocks"], &config);
    let Some(Command::Blocks(args)) = cli.command else {
        panic!("expected blocks command");
    };
    assert!(args.active);
    assert_eq!(args.token_limit.as_deref(), Some("500000"));
    assert_eq!(args.session_length, 6.0);

    let cli = parse_with_config(&["ccusage", "claude", "statusline"], &config);
    let Some(Command::Statusline(args)) = cli.command else {
        panic!("expected statusline command");
    };
    assert_eq!(args.visual_burn_rate, VisualBurnRate::EmojiText);
    assert_eq!(args.cost_source, CostSource::Both);
    assert_eq!(args.refresh_interval, 3);

    let cli = parse_with_config(&["ccusage", "pi", "daily"], &config);
    let Some(Command::Pi(args)) = cli.command else {
        panic!("expected pi command");
    };
    assert_eq!(args.pi_path.as_deref(), Some("/tmp/pi-sessions"));

    let cli = parse_with_config(&["ccusage", "openclaw", "daily"], &config);
    let Some(Command::OpenClaw(args)) = cli.command else {
        panic!("expected openclaw command");
    };
    assert_eq!(args.open_claw_path.as_deref(), Some("/tmp/openclaw"));
}

#[test]
fn root_help_lists_agent_namespaces_without_nested_commands() {
    let help = help_text();
    let agents = [
        "claude", "codex", "opencode", "amp", "droid", "codebuff", "hermes", "pi", "goose", "kilo",
        "copilot", "gemini", "kimi", "qwen", "openclaw",
    ];

    for agent in agents {
        assert!(help.contains(&format!("\n  {agent} ")));
        assert!(!help.contains(&format!("\n  {agent} daily")));
    }
}

#[test]
fn root_help_lists_command_descriptions_and_follow_up_help_commands() {
    let help = help_text();

    assert!(help.contains("codex                      Show Codex token usage commands"));
    assert!(help.contains("For more info, run any command with the `--help` flag:"));
    assert!(help.contains("ccusage codex --help"));
    assert!(!help.contains("ccusage codex daily --help"));
}

#[test]
fn contextual_codex_help_lists_speed_choices() {
    let help = help_text_for_args(&[
        "ccusage".to_string(),
        "codex".to_string(),
        "daily".to_string(),
        "--help".to_string(),
    ]);

    assert!(help.contains("Show Codex token usage grouped by day"));
    assert!(help.contains("USAGE:\n  ccusage codex daily <OPTIONS>"));
    assert!(help.contains("choices: auto | standard | fast"));
}

#[test]
fn contextual_help_strips_path_like_program_name() {
    let help = help_text_for_args(&[
        "/usr/local/bin/ccusage".to_string(),
        "codex".to_string(),
        "daily".to_string(),
    ]);

    assert!(help.contains("USAGE:\n  ccusage codex daily <OPTIONS>"));
}

#[test]
fn contextual_help_strips_windows_program_name() {
    let help = help_text_for_args(&[
        "C:\\Tools\\ccusage.exe".to_string(),
        "codex".to_string(),
        "daily".to_string(),
    ]);

    assert!(help.contains("USAGE:\n  ccusage codex daily <OPTIONS>"));
}

#[test]
fn contextual_agent_help_lists_agent_subcommands() {
    let help = help_text_for_args(&["ccusage".to_string(), "claude".to_string()]);

    assert!(help.contains("USAGE:\n  ccusage claude <COMMANDS>"));
    assert!(help.contains("daily       Show usage report grouped by date"));
    assert!(help.contains("statusline  Display compact status line for Claude Code hooks"));
    assert!(help.contains("ccusage claude statusline --help"));
    assert!(!help.contains("ccusage claude daily <OPTIONS>"));
}

#[test]
fn contextual_all_agent_help_lists_color_options() {
    let help = help_text_for_args(&["ccusage".to_string(), "daily".to_string()]);

    assert!(help.contains("--color"));
    assert!(help.contains("--no-color"));
}

#[test]
fn contextual_root_session_help_lists_id_option() {
    let help = help_text_for_args(&["ccusage".to_string(), "session".to_string()]);

    assert!(help.contains("--id"));
}

#[test]
fn contextual_statusline_help_lists_choice_options() {
    let help = help_text_for_args(&["ccusage".to_string(), "statusline".to_string()]);

    assert!(help.contains("choices: off | emoji | text | emoji-text"));
    assert!(help.contains("choices: auto | ccusage | cc | both"));
}

#[test]
fn snapshots_root_and_contextual_help_text() {
    insta::assert_snapshot!("root_help", help_text());
    insta::assert_snapshot!(
        "claude_agent_help",
        help_text_for_args(&["ccusage".to_string(), "claude".to_string()])
    );
    insta::assert_snapshot!(
        "codex_daily_help",
        help_text_for_args(&[
            "ccusage".to_string(),
            "codex".to_string(),
            "daily".to_string(),
        ])
    );
    insta::assert_snapshot!(
        "statusline_help",
        help_text_for_args(&["ccusage".to_string(), "statusline".to_string()])
    );
}

#[test]
fn snapshots_representative_cli_parse_shapes() {
    let cases = vec![
        json!({
            "case": "default all-agent daily",
            "cli": cli_snapshot(parse(&["ccusage"])),
        }),
        json!({
            "case": "root daily with shared flags",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "--json",
                "--since=20260102",
                "--until",
                "20260110",
                "--mode",
                "calculate",
                "--debug",
                "--debug-samples",
                "9",
                "--order",
                "desc",
                "--breakdown",
                "--offline",
                "--no-offline",
                "--color",
                "--no-color",
                "--timezone",
                "Asia/Tokyo",
                "--jq",
                ".totals",
                "--compact",
                "--single-thread",
                "daily",
            ])),
        }),
        json!({
            "case": "claude weekly monday",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "claude",
                "weekly",
                "--start-of-week",
                "monday",
            ])),
        }),
        json!({
            "case": "claude daily project instances",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "claude",
                "daily",
                "--instances",
                "--project",
                "repo",
                "--project-aliases",
                "repo=Repository",
            ])),
        }),
        json!({
            "case": "codex monthly fast",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "codex",
                "monthly",
                "--speed=fast",
            ])),
        }),
        json!({
            "case": "opencode weekly",
            "cli": cli_snapshot(parse(&["ccusage", "opencode", "weekly", "--json"])),
        }),
        json!({
            "case": "pi session path",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "pi",
                "session",
                "--pi-path",
                "/tmp/pi-sessions",
            ])),
        }),
        json!({
            "case": "openclaw session path",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "openclaw",
                "session",
                "--open-claw-path=/tmp/openclaw",
            ])),
        }),
        json!({
            "case": "blocks active recent",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "blocks",
                "--active",
                "--recent",
                "--token-limit",
                "max",
                "--session-length=6.5",
            ])),
        }),
        json!({
            "case": "statusline thresholds",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "statusline",
                "--no-offline",
                "--visual-burn-rate",
                "emoji-text",
                "--cost-source",
                "both",
                "--no-cache",
                "--refresh-interval",
                "3",
                "--context-low-threshold",
                "45",
                "--context-medium-threshold",
                "75",
                "--debug",
            ])),
        }),
    ];

    insta::assert_json_snapshot!(cases);
}

#[test]
fn snapshots_cli_parse_error_guidance() {
    let cases = vec![
        json!({
            "args": ["ccusage", "--daily"],
            "error": parse_error(&["ccusage", "--daily"]),
        }),
        json!({
            "args": ["ccusage", "daily", "--agent", "codex"],
            "error": parse_error(&["ccusage", "daily", "--agent", "codex"]),
        }),
        json!({
            "args": ["ccusage", "codex", "blocks"],
            "error": parse_error(&["ccusage", "codex", "blocks"]),
        }),
        json!({
            "args": ["ccusage", "--mode", "bad"],
            "error": parse_error(&["ccusage", "--mode", "bad"]),
        }),
        json!({
            "args": ["ccusage", "blocks", "--session-length", "abc"],
            "error": parse_error(&["ccusage", "blocks", "--session-length", "abc"]),
        }),
        json!({
            "args": ["ccusage", "statusline", "--visual-burn-rate", "loud"],
            "error": parse_error(&[
                "ccusage",
                "statusline",
                "--visual-burn-rate",
                "loud",
            ]),
        }),
        json!({
            "args": ["ccusage", "pi", "weekly"],
            "error": parse_error(&["ccusage", "pi", "weekly"]),
        }),
    ];

    insta::assert_json_snapshot!(cases);
}

#[test]
fn cargo_version_is_independent_from_release_version() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.0.0");
}

#[test]
fn parses_claude_daily_options() {
    let cli = parse(&[
        "ccusage",
        "claude",
        "daily",
        "--json",
        "--mode",
        "display",
        "--instances",
        "--project",
        "repo",
    ]);
    let Some(Command::Daily(args)) = cli.command else {
        panic!("expected daily command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.mode, CostMode::Display);
    assert!(args.instances);
    assert_eq!(args.project.as_deref(), Some("repo"));
}

#[test]
fn rejects_removed_locale_option() {
    let result = Cli::parse_from(
        ["ccusage", "--locale", "en-CA"]
            .into_iter()
            .map(OsString::from),
    );
    assert!(result.is_err());
}

#[test]
fn parses_blocks_defaults_and_values() {
    let cli = parse(&[
        "ccusage",
        "blocks",
        "-a",
        "--token-limit=max",
        "--session-length",
        "6",
    ]);
    let Some(Command::Blocks(args)) = cli.command else {
        panic!("expected blocks command");
    };
    assert!(args.active);
    assert_eq!(args.token_limit.as_deref(), Some("max"));
    assert_eq!(args.session_length, 6.0);
}

#[test]
fn parses_claude_blocks_short_active_option() {
    let cli = parse(&["ccusage", "claude", "blocks", "-a"]);
    let Some(Command::Blocks(args)) = cli.command else {
        panic!("expected blocks command");
    };
    assert!(args.active);
}

#[test]
fn parses_statusline_options() {
    let cli = parse(&[
        "ccusage",
        "statusline",
        "--no-cache",
        "--timezone",
        "Asia/Tokyo",
        "--visual-burn-rate",
        "emoji-text",
        "--cost-source",
        "both",
    ]);
    let Some(Command::Statusline(args)) = cli.command else {
        panic!("expected statusline command");
    };
    assert!(args.offline);
    assert!(args.no_cache);
    assert_eq!(args.timezone.as_deref(), Some("Asia/Tokyo"));
    assert_eq!(args.visual_burn_rate, VisualBurnRate::EmojiText);
    assert_eq!(args.cost_source, CostSource::Both);
}

#[test]
fn parses_codex_default_daily_options() {
    let cli = parse(&["ccusage", "codex", "--json", "--since", "20260102"]);
    let Some(Command::Codex(args)) = cli.command else {
        panic!("expected codex command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
}

#[test]
fn parses_codex_speed_option() {
    let cli = parse(&["ccusage", "codex", "daily", "--speed", "fast"]);
    let Some(Command::Codex(args)) = cli.command else {
        panic!("expected codex command");
    };
    assert_eq!(args.codex_speed, CodexSpeed::Fast);
}

#[test]
fn parses_legacy_colon_agent_commands() {
    let cli = parse(&["ccusage", "codex:monthly", "--json"]);
    let Some(Command::Codex(args)) = cli.command else {
        panic!("expected codex command");
    };
    assert_eq!(args.kind, AgentReportKind::Monthly);
    assert!(args.shared.json);
}

#[test]
fn rejects_report_flag_aliases_with_guidance() {
    let error = parse_error(&["ccusage", "--daily"]);
    assert_eq!(
        error,
        "Report flags like --daily are not supported. Use \"ccusage daily\" instead."
    );
}

#[test]
fn rejects_agent_filter_options_with_guidance() {
    let error = parse_error(&["ccusage", "daily", "--agent", "codex"]);
    assert_eq!(
        error,
        "Agent filters like --agent are not supported. Use \"ccusage <agent> <report>\", for example \"ccusage codex daily\"."
    );
}

#[test]
fn rejects_unsupported_agent_reports_with_guidance() {
    let error = parse_error(&["ccusage", "codex", "blocks"]);
    assert_eq!(
        error,
        "The \"blocks\" report is only available for Claude Code usage.\nUse \"ccusage codex daily\" for Codex usage reports."
    );
}

#[test]
fn parses_claude_namespace_session_options() {
    let cli = parse(&["ccusage", "claude", "session", "--json", "--id", "abc"]);
    let Some(Command::Session(args)) = cli.command else {
        panic!("expected claude session command");
    };
    assert!(args.shared.json);
    assert_eq!(args.id.as_deref(), Some("abc"));
}

#[test]
fn parses_top_level_session_id_lookup() {
    let cli = parse(&["ccusage", "session", "--json", "--id", "abc"]);
    let Some(Command::Session(args)) = cli.command else {
        panic!("expected session command");
    };
    assert!(args.shared.json);
    assert_eq!(args.id.as_deref(), Some("abc"));
}

#[test]
fn parses_opencode_weekly_options() {
    let cli = parse(&["ccusage", "opencode", "weekly", "--json"]);
    let Some(Command::OpenCode(args)) = cli.command else {
        panic!("expected opencode command");
    };
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert!(args.shared.json);
}

#[test]
fn parses_amp_session_options() {
    let cli = parse(&["ccusage", "amp", "session", "--json"]);
    let Some(Command::Amp(args)) = cli.command else {
        panic!("expected amp command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_droid_session_options() {
    let cli = parse(&["ccusage", "droid", "session", "--json"]);
    let Some(Command::Droid(args)) = cli.command else {
        panic!("expected droid command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_codebuff_session_options() {
    let cli = parse(&["ccusage", "codebuff", "session", "--json"]);
    let Some(Command::Codebuff(args)) = cli.command else {
        panic!("expected codebuff command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_qwen_session_options() {
    let cli = parse(&["ccusage", "qwen", "session", "--json"]);
    let Some(Command::Qwen(args)) = cli.command else {
        panic!("expected qwen command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_pi_session_options() {
    let cli = parse(&[
        "ccusage",
        "pi",
        "session",
        "--json",
        "--pi-path",
        "/tmp/pi-sessions",
    ]);
    let Some(Command::Pi(args)) = cli.command else {
        panic!("expected pi command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(args.pi_path.as_deref(), Some("/tmp/pi-sessions"));
}

#[test]
fn parses_kilo_session_options() {
    let cli = parse(&["ccusage", "kilo", "session", "--json"]);
    let Some(Command::Kilo(args)) = cli.command else {
        panic!("expected kilo command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_goose_session_options() {
    let cli = parse(&["ccusage", "goose", "session", "--json"]);
    let Some(Command::Goose(args)) = cli.command else {
        panic!("expected goose command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_copilot_session_options() {
    let cli = parse(&["ccusage", "copilot", "session", "--json"]);
    let Some(Command::Copilot(args)) = cli.command else {
        panic!("expected copilot command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_gemini_session_options() {
    let cli = parse(&["ccusage", "gemini", "session", "--json"]);
    let Some(Command::Gemini(args)) = cli.command else {
        panic!("expected gemini command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_kimi_session_options() {
    let cli = parse(&["ccusage", "kimi", "session", "--json"]);
    let Some(Command::Kimi(args)) = cli.command else {
        panic!("expected kimi command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_openclaw_session_options() {
    let cli = parse(&[
        "ccusage",
        "openclaw",
        "session",
        "--json",
        "--open-claw-path",
        "/tmp/openclaw",
    ]);
    let Some(Command::OpenClaw(args)) = cli.command else {
        panic!("expected openclaw command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(args.open_claw_path.as_deref(), Some("/tmp/openclaw"));
}
