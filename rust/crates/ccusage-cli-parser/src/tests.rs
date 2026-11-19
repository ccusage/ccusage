use std::{ffi::OsString, num::NonZeroU32};

use serde_json::{Value, json};

use crate::help::{help_text, help_text_for_args};
use crate::*;
use ccusage_cli::*;
use ccusage_test_support::fs_fixture;

fn parse(args: &[&str]) -> Command {
    parse_with_config(args, &NoConfig)
}

fn parse_with_config(args: &[&str], config: &dyn CliConfig) -> Command {
    parse_from_with_config(
        args.iter().map(OsString::from),
        config,
        env!("CARGO_PKG_VERSION"),
    )
    .unwrap()
}

fn parse_error(args: &[&str]) -> String {
    match parse_from_with_config(
        args.iter().map(OsString::from),
        &NoConfig,
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(_) => panic!("expected parse error"),
        Err(error) => error,
    }
}

#[test]
fn misplaced_short_active_is_an_unknown_root_option_for_blocks() {
    assert_eq!(
        parse_error(&["ccusage", "-a", "blocks"]),
        "Unknown option '-a'"
    );
}

#[test]
fn unsupported_agent_report_guidance_precedes_a_config_error() {
    let fixture = fs_fixture!({"cwd/.keep": ""});
    let path = fixture.path("missing.json");
    let args = ["codex", "blocks", "--config", path.to_str().unwrap()];
    let config = ccusage_config::ConfigContext::from_args(&args.map(str::to_string));
    let result = parse_from_with_config(
        std::iter::once(OsString::from("ccusage")).chain(args.map(OsString::from)),
        &config,
        env!("CARGO_PKG_VERSION"),
    );
    let Err(error) = result else {
        panic!("expected the unsupported report to be rejected");
    };
    assert_eq!(
        error,
        "The \"blocks\" report is only available for Claude Code usage.\nUse \"ccusage codex daily\" for Codex usage reports."
    );
}

#[test]
fn blocks_reject_invalid_token_limits() {
    for value in ["abc", "0"] {
        assert!(
            parse_error(&["ccusage", "blocks", "--token-limit", value])
                .contains("Invalid value for --token-limit")
        );
    }
}

#[test]
fn blocks_reject_invalid_session_lengths() {
    for value in ["NaN", "inf", "0", "1e300", "0.0000000001"] {
        assert!(
            parse_error(&["ccusage", "blocks", "--session-length", value])
                .contains("Invalid value for --session-length")
        );
    }
}

#[test]
fn blocks_reject_negative_limits() {
    for flag in ["--token-limit=-1", "--session-length=-1"] {
        assert!(parse_error(&["ccusage", "blocks", flag]).contains("Invalid value"));
    }
}

#[test]
fn statusline_rejects_unordered_context_thresholds() {
    assert!(
        parse_error(&["ccusage", "statusline", "--context-low-threshold", "80"])
            .contains("must be less than medium threshold")
    );
}

#[test]
fn statusline_threshold_overrides_are_validated_together() {
    let Command::Statusline(_) = parse(&[
        "ccusage",
        "statusline",
        "--context-low-threshold",
        "90",
        "--context-medium-threshold",
        "95",
    ]) else {
        panic!("expected statusline")
    };
}

#[test]
fn statusline_merges_config_thresholds_before_validation() {
    use ccusage_config::ConfigContext;
    use ccusage_test_support::EnvGuard;
    let fixture = fs_fixture!({
        "home/.claude/ccusage.json": r#"{"commands":{"statusline":{"contextLowThreshold":85}}}"#,
        "explicit.json": r#"{"commands":{"statusline":{"contextLowThreshold":85}}}"#,
    });
    let _env = EnvGuard::set_many([
        ("HOME", Some(fixture.path("home").into_os_string())),
        (
            "CLAUDE_CONFIG_DIR",
            Some(fixture.path("home/.claude").into_os_string()),
        ),
        (
            "XDG_CONFIG_HOME",
            Some(fixture.path("home/.config").into_os_string()),
        ),
    ]);
    for explicit in [false, true] {
        let path = fixture.path("explicit.json").display().to_string();
        let mut args = vec!["ccusage", "statusline", "--context-medium-threshold", "95"];
        if explicit {
            args.extend(["--config", &path]);
        }
        let config = ConfigContext::from_args(
            &args[1..]
                .iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>(),
        );
        let Command::Statusline(statusline) = parse_with_config(&args, &config) else {
            panic!("expected statusline");
        };
        assert_eq!(
            statusline.context_thresholds,
            ContextThresholds::new(85, 95).unwrap()
        );
        assert!(config.warnings().is_empty(), "{:?}", config.warnings());
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
    fn timezone(&self) -> Option<&str> {
        self.shared_timezone
    }

    fn apply_shared(&self, shared: &mut SharedArgs) {
        if let Some(json) = self.shared_json {
            shared.json = json;
        }
        if let Some(order) = self.shared_order {
            shared.order = Some(order);
        }
        if let Some(since) = self.shared_since {
            shared.since = Some(since.to_string());
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
            args.token_limit = Some(token_limit.parse().unwrap());
        }
        if let Some(session_length) = self.blocks_session_length {
            args.session_length = ccusage_cli::SessionLength::from_hours(session_length).unwrap();
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

    fn agent_options(&self, agent: &str) -> AgentOptions {
        match agent {
            "codex" => AgentOptions {
                codex_speed: self.codex_speed.unwrap_or_default(),
                ..AgentOptions::default()
            },
            "pi" => AgentOptions {
                pi_path: self.pi_path.map(str::to_string),
                ..AgentOptions::default()
            },
            "openclaw" => AgentOptions {
                open_claw_path: self.open_claw_path.map(str::to_string),
                ..AgentOptions::default()
            },
            _ => AgentOptions::default(),
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
        "order": format!("{:?}", shared.order.unwrap_or_default()),
        "breakdown": shared.breakdown,
        "offline": shared.offline,
        "color": format!("{:?}", shared.color),
        "timezone": shared.timezone.as_ref().map(|timezone| timezone.name()),
        "jq": shared.jq.as_deref(),
        "compact": shared.compact,
        "singleThread": shared.single_thread,
    })
}

fn cli_snapshot(cli: Command) -> Value {
    json!({
        "command": command_snapshot(cli),
    })
}

fn command_snapshot(command: Command) -> Value {
    match command {
        Command::All(args) => agent_command_snapshot("all", args),
        Command::Daily(args) => json!({
            "type": "daily",
            "shared": shared_snapshot(&args.shared),
            "instances": args.instances,
            "project": args.project,
            "projectAliases": args.project_aliases,
        }),
        Command::Monthly(shared) => json!({
            "type": "monthly",
            "shared": shared_snapshot(&shared),
        }),
        Command::Weekly(args) => json!({
            "type": "weekly",
            "shared": shared_snapshot(&args.shared),
            "startOfWeek": format!("{:?}", args.start_of_week),
        }),
        Command::Session(args) => json!({
            "type": "session",
            "shared": shared_snapshot(&args.shared),
            "id": args.id,
        }),
        Command::Blocks(args) => json!({
            "type": "blocks",
            "shared": shared_snapshot(&args.shared),
            "active": args.active,
            "recent": args.recent,
            "tokenLimit": args.token_limit.map(|value| value.to_string()),
            "sessionLength": args.session_length.as_millis() as f64 / 3_600_000.0,
        }),
        Command::Statusline(args) => json!({
            "type": "statusline",
            "offline": args.offline,
            "visualBurnRate": format!("{:?}", args.visual_burn_rate),
            "costSource": format!("{:?}", args.cost_source),
            "cache": args.cache,
            "refreshInterval": args.refresh_interval,
            "contextLowThreshold": args.context_thresholds.low(),
            "contextMediumThreshold": args.context_thresholds.medium(),
            "debug": args.debug,
        }),
        Command::Agent(_, AgentCommand::Statusline(args)) => json!({
            "type": "codex-statusline",
            "shared": shared_snapshot(&args.shared),
            "sessionId": args.session_id,
            "speed": format!("{:?}", args.codex_speed),
            "mcp": args.mcp,
        }),
        Command::Agent(agent, AgentCommand::Report(args)) => {
            agent_command_snapshot(agent.spec().name, args)
        }
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
        "piPath": args.options.pi_path,
        "openClawPath": args.options.open_claw_path,
        "codexSpeed": format!("{:?}", args.options.codex_speed),
    })
}

#[test]
fn parses_root_daily_as_all_agent_report() {
    let cli = parse(&["ccusage", "daily", "--json", "--since", "20260102"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
}

#[test]
fn commandless_invocation_resolves_the_unified_daily_report() {
    let cli = parse(&["ccusage"]);

    let Command::All(args) = cli else {
        panic!("expected the unified daily report");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert_eq!(args.start_of_week, WeekDay::Monday);
}

#[test]
fn source_reparse_keeps_the_unified_week_start() {
    let args = parse_report_args_with_config(
        ["ccusage", "weekly", "--start-of-week", "friday"]
            .into_iter()
            .map(OsString::from),
        &TestConfig {
            weekly_start: Some(WeekDay::Sunday),
            ..TestConfig::default()
        },
        "test",
        WeekDay::Wednesday,
        None,
    )
    .unwrap();
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert_eq!(args.start_of_week, WeekDay::Wednesday);
}

#[test]
fn report_reparse_carries_focused_agent_options() {
    let args = parse_report_args_with_config(
        ["ccusage", "opencode", "weekly", "--since", "20260102"]
            .into_iter()
            .map(OsString::from),
        &TestConfig::default(),
        "test",
        WeekDay::Monday,
        None,
    )
    .unwrap();
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
}

#[test]
fn offline_flags_override_config_in_both_directions_and_positions() {
    for (config_json, flag, expected) in [
        (r#"{"defaults":{"offline":true}}"#, "--no-offline", false),
        (r#"{"defaults":{"noOffline":true}}"#, "--offline", true),
        (r#"{"defaults":{"offline":false}}"#, "--offline", true),
    ] {
        let fixture = fs_fixture!({ "ccusage.json": config_json });
        let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
        for command in ["daily", "statusline"] {
            for argv in [
                vec!["ccusage", command, flag, "--config", &path],
                vec!["ccusage", flag, command, "--config", &path],
            ] {
                let config_args = argv[1..]
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect::<Vec<_>>();
                let config = ccusage_config::ConfigContext::from_args(&config_args);
                let command = parse_with_config(&argv, &config);
                let offline = match command {
                    Command::Statusline(args) => args.offline,
                    command => command.shared().unwrap().offline,
                };
                assert_eq!(offline, expected, "{argv:?}, {config_json}");
            }
        }
    }
}

#[test]
fn cache_flags_override_config_in_both_directions() {
    for (config_json, flag, expected) in [
        (r#"{"defaults":{"cache":true}}"#, "--no-cache", false),
        (r#"{"defaults":{"noCache":true}}"#, "--cache", true),
    ] {
        let fixture = fs_fixture!({ "ccusage.json": config_json });
        let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
        let argv = ["ccusage", "statusline", flag, "--config", &path];
        let config_args = argv[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>();
        let config = ccusage_config::ConfigContext::from_args(&config_args);
        let Command::Statusline(args) = parse_with_config(&argv, &config) else {
            panic!("expected statusline");
        };
        assert_eq!(args.cache, expected, "{config_json}");
    }
}

#[test]
fn color_flags_override_the_opposite_config_value() {
    for (config_json, flag, expected) in [
        (
            r#"{"defaults":{"color":true}}"#,
            "--no-color",
            ColorChoice::Never,
        ),
        (
            r#"{"defaults":{"noColor":true}}"#,
            "--color",
            ColorChoice::Always,
        ),
    ] {
        let fixture = fs_fixture!({ "ccusage.json": config_json });
        let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
        let argv = ["ccusage", "daily", flag, "--config", &path];
        let config_args = argv[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>();
        let config = ccusage_config::ConfigContext::from_args(&config_args);
        let command = parse_with_config(&argv, &config);
        let shared = command.shared().unwrap();
        assert_eq!(shared.color, expected, "{config_json}");
    }
}

#[test]
fn false_negation_and_color_keys_leave_lower_config_layers_in_force() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{
            "defaults": { "noColor": true, "offline": true, "cache": false },
            "commands": {
                "daily": { "color": false, "noOffline": false },
                "statusline": { "noOffline": false, "noCache": false }
            }
        }"#,
    });
    let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    for report in ["daily", "statusline"] {
        let argv = ["ccusage", report, "--config", &path];
        let config_args = argv[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>();
        let config = ccusage_config::ConfigContext::from_args(&config_args);
        match parse_with_config(&argv, &config) {
            Command::Statusline(args) => {
                assert!(args.offline);
                assert!(!args.cache);
            }
            command => {
                let shared = command.shared().unwrap();
                assert!(shared.offline);
                assert_eq!(shared.color, ColorChoice::Never);
            }
        }
    }
}

#[test]
fn the_last_offline_flag_wins() {
    for (flags, expected) in [
        (["--offline", "--no-offline"], false),
        (["--no-offline", "--offline"], true),
    ] {
        let command = parse(&["ccusage", "daily", flags[0], flags[1]]);
        assert_eq!(command.shared().unwrap().offline, expected);
        let Command::Statusline(args) = parse(&["ccusage", "statusline", flags[0], flags[1]])
        else {
            panic!("expected statusline");
        };
        assert_eq!(args.offline, expected);
    }
}

#[test]
fn higher_config_layers_override_negated_defaults() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{
            "defaults": { "noOffline": true, "noCache": true, "noColor": true },
            "commands": {
                "daily": { "offline": true, "color": true },
                "statusline": { "offline": true, "cache": true }
            }
        }"#,
    });
    let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    for report in ["daily", "statusline"] {
        let argv = ["ccusage", report, "--config", &path];
        let config_args = argv[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>();
        let config = ccusage_config::ConfigContext::from_args(&config_args);
        let command = parse_with_config(&argv, &config);
        match command {
            Command::Statusline(args) => {
                assert!(args.offline);
                assert!(args.cache);
            }
            command => {
                let shared = command.shared().unwrap();
                assert!(shared.offline);
                assert_eq!(shared.color, ColorChoice::Always);
            }
        }
    }
}

#[test]
fn rejects_date_bounds_that_are_not_real_calendar_dates() {
    for value in [
        "2026-02-30",
        "2026-13-01",
        "abc",
        "2026/07/10",
        "2026-07-10T00:00:00Z",
    ] {
        assert_eq!(
            parse_error(&["ccusage", "daily", "--since", value]),
            format!("Invalid value for --since '{value}'. Expected YYYY-MM-DD or YYYYMMDD.")
        );
        assert_eq!(
            parse_error(&["ccusage", "daily", "--until", value]),
            format!("Invalid value for --until '{value}'. Expected YYYY-MM-DD or YYYYMMDD.")
        );
    }
}

#[test]
fn accepts_both_documented_date_bound_formats() {
    for (value, normalized) in [("2026-07-10", "20260710"), ("20260710", "20260710")] {
        let cli = parse(&["ccusage", "daily", "--since", value, "--until", value]);
        let Command::All(args) = cli else {
            panic!("expected all-agent command");
        };
        assert_eq!(args.shared.since.as_deref(), Some(normalized));
        assert_eq!(args.shared.until.as_deref(), Some(normalized));
    }
}

#[test]
fn parses_last_periods_on_period_reports() {
    let cli = parse(&["ccusage", "daily", "--last", "1"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(1));

    let cli = parse(&["ccusage", "claude", "weekly", "--last", "2"]);
    let Command::Weekly(args) = cli else {
        panic!("expected weekly command");
    };
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(2));

    let cli = parse(&["ccusage", "codex", "monthly", "--last=3"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(3));
}

#[test]
fn parses_last_periods_before_the_command_name() {
    let cli = parse(&["ccusage", "--last", "1", "weekly"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(1));

    let cli = parse(&["ccusage", "--last", "1"]);
    let Command::All(args) = cli else {
        panic!("expected unified report");
    };
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(1));
}

#[test]
fn leaves_the_short_alias_of_the_removed_locale_option_unused() {
    assert_eq!(
        parse_error(&["ccusage", "daily", "-l", "1"]),
        "Unknown option '-l'"
    );
}

#[test]
fn rejects_last_periods_on_reports_without_a_period() {
    for args in [
        ["ccusage", "session", "--last", "1"],
        ["ccusage", "blocks", "--last", "1"],
        ["ccusage", "codex", "session", "--last=1"],
    ] {
        assert_eq!(
            parse_error(&args),
            "The --last option is only available for the daily, weekly, and monthly reports."
        );
    }
}

#[test]
fn rejects_a_since_date_later_than_the_until_date() {
    let cases: [&[&str]; 6] = [
        &["ccusage", "--since", "2026-09-14", "--until", "2026-09-01"],
        &[
            "ccusage", "daily", "--since", "20260914", "--until", "20260901",
        ],
        &[
            "ccusage",
            "session",
            "--since",
            "2026-09-14",
            "--until",
            "20260901",
        ],
        &[
            "ccusage",
            "claude",
            "session",
            "--since",
            "2026-09-14",
            "--until",
            "20260901",
        ],
        &[
            "ccusage",
            "blocks",
            "--since",
            "2026-09-14",
            "--until",
            "2026-09-01",
        ],
        &[
            "ccusage",
            "codex",
            "monthly",
            "--since",
            "2026-09-14",
            "--until",
            "2026-09-01",
        ],
    ];
    for args in cases {
        assert_eq!(
            parse_error(args),
            "The --since date '20260914' is later than the --until date '20260901'."
        );
    }
}

#[test]
fn accepts_a_since_date_equal_to_the_until_date() {
    let cli = parse(&[
        "ccusage",
        "claude",
        "daily",
        "--since",
        "2026-09-14",
        "--until",
        "20260914",
    ]);
    let Command::Daily(args) = cli else {
        panic!("expected claude daily command");
    };
    assert_eq!(args.shared.since.as_deref(), Some("20260914"));
    assert_eq!(args.shared.until.as_deref(), Some("20260914"));
}

#[test]
fn rejects_a_config_since_date_later_than_the_until_flag() {
    let config = TestConfig {
        shared_since: Some("20260914"),
        ..TestConfig::default()
    };
    let result = parse_from_with_config(
        ["ccusage", "daily", "--until", "2026-09-01"]
            .iter()
            .map(OsString::from),
        &config,
        env!("CARGO_PKG_VERSION"),
    );
    let Err(error) = result else {
        panic!("expected parse error");
    };
    assert_eq!(
        error,
        "The --since date '20260914' is later than the --until date '20260901'."
    );
}

#[test]
fn checks_the_date_window_after_config_and_flags_are_merged() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "defaults": { "since": "2026-09-14", "until": "2026-09-01" } }"#,
    });
    let config_path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    let parse_with_file = |extra: &[&str]| {
        let mut args = vec![
            "daily".to_string(),
            "--config".to_string(),
            config_path.clone(),
        ];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        let config = ccusage_config::ConfigContext::from_args(&args);
        parse_from_with_config(
            std::iter::once(OsString::from("ccusage"))
                .chain(args.iter().map(|arg| OsString::from(arg.as_str()))),
            &config,
            env!("CARGO_PKG_VERSION"),
        )
    };

    let Err(error) = parse_with_file(&[]) else {
        panic!("expected a reversed config window to be rejected");
    };
    assert_eq!(
        error,
        "The --since date '20260914' is later than the --until date '20260901'."
    );

    let Ok(cli) = parse_with_file(&["--until", "2026-09-30"]) else {
        panic!("expected a flag to repair the reversed config window");
    };
    let Command::All(args) = cli else {
        panic!("expected unified daily command");
    };
    assert_eq!(args.shared.since.as_deref(), Some("20260914"));
    assert_eq!(args.shared.until.as_deref(), Some("20260930"));
}

#[test]
fn statusline_ignores_reversed_default_date_window() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "defaults": { "since": "2026-09-14", "until": "2026-09-01" } }"#,
    });
    let config_path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    let args = ["statusline", "--config", config_path.as_str()]
        .map(str::to_string)
        .to_vec();
    let config = ccusage_config::ConfigContext::from_args(&args);

    let result = parse_from_with_config(
        std::iter::once(OsString::from("ccusage")).chain(args.iter().map(OsString::from)),
        &config,
        env!("CARGO_PKG_VERSION"),
    );

    let Ok(cli) = result else {
        panic!("expected statusline to ignore the report date window");
    };
    assert!(matches!(cli, Command::Statusline(_)));
}

#[test]
fn rejects_last_periods_alongside_an_explicit_date_window() {
    assert_eq!(
        parse_error(&["ccusage", "daily", "--last", "1", "--since", "20260101"]),
        "The --last option cannot be combined with --since or --until."
    );
    assert_eq!(
        parse_error(&["ccusage", "daily", "--last", "1", "--until", "20260101"]),
        "The --last option cannot be combined with --since or --until."
    );
}

#[test]
fn rejects_last_periods_with_multi_section_reports() {
    assert_eq!(
        parse_error(&["ccusage", "daily", "--last", "1", "--sections", "monthly"]),
        "The --last option cannot be used with --sections."
    );
}

#[test]
fn rejects_last_periods_that_are_not_positive_whole_numbers() {
    assert_eq!(
        parse_error(&["ccusage", "daily", "--last", "0"]),
        "Invalid value for --last '0'. Expected a whole number of periods, 1 or greater."
    );
    assert_eq!(
        parse_error(&["ccusage", "daily", "--last", "week"]),
        "Invalid value for --last 'week'. Expected a whole number of periods, 1 or greater."
    );
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
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(
        args.sections.as_deref(),
        Some(&[AgentReportKind::Daily, AgentReportKind::Monthly][..])
    );
}

#[test]
fn rejects_an_inline_value_on_a_switch() {
    for (args, flag) in [
        (&["ccusage", "--offline=false"][..], "--offline"),
        (&["ccusage", "daily", "--json=false"][..], "--json"),
        (
            &["ccusage", "daily", "--json=false", "--since", "20260101"][..],
            "--json",
        ),
        (&["ccusage", "--by-agent=no", "daily"][..], "--by-agent"),
        (&["ccusage", "daily", "--all=yes"][..], "--all"),
        (
            &["ccusage", "claude", "blocks", "--active=yes"][..],
            "--active",
        ),
        (&["ccusage", "statusline", "--cache=false"][..], "--cache"),
        (
            &["ccusage", "opencode", "--include-claude-agent=1"][..],
            "--include-claude-agent",
        ),
    ] {
        assert_eq!(
            parse_error(args),
            format!("{flag} does not take a value"),
            "{args:?}"
        );
    }
}

#[test]
fn ignores_empty_unified_section_tokens() {
    let cli = parse(&["ccusage", "daily", "--sections", "daily,,monthly,"]);
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
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
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert!(args.session_id.is_none());
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
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
    assert_eq!(args.shared.order, Some(SortOrder::Asc));
}

#[test]
fn applies_agent_namespace_config_to_codex_speed() {
    let config = TestConfig {
        codex_speed: Some(CodexSpeed::Fast),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "codex", "daily"], &config);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.options.codex_speed, CodexSpeed::Fast);
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
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.kind, AgentReportKind::Monthly);
    assert!(args.shared.json);
    assert_eq!(
        args.shared
            .timezone
            .as_ref()
            .map(|timezone| timezone.name()),
        Some("Asia/Tokyo")
    );
    assert_eq!(args.shared.since.as_deref(), Some("20260101"));
    assert_eq!(args.options.codex_speed, CodexSpeed::Standard);
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
    let Command::Weekly(args) = cli else {
        panic!("expected weekly command");
    };
    assert!(args.shared.json);
    assert!(args.shared.compact);
    assert_eq!(args.start_of_week, WeekDay::Monday);

    let cli = parse_with_config(&["ccusage", "claude", "blocks"], &config);
    let Command::Blocks(args) = cli else {
        panic!("expected blocks command");
    };
    assert!(args.active);
    assert_eq!(args.token_limit, Some("500000".parse().unwrap()));
    assert_eq!(args.session_length.as_millis(), 21_600_000);

    let cli = parse_with_config(&["ccusage", "claude", "statusline"], &config);
    let Command::Statusline(args) = cli else {
        panic!("expected statusline command");
    };
    assert_eq!(args.visual_burn_rate, VisualBurnRate::EmojiText);
    assert_eq!(args.cost_source, CostSource::Both);
    assert_eq!(args.refresh_interval, 3);

    let cli = parse_with_config(&["ccusage", "pi", "daily"], &config);
    let Command::Agent(StandardAgent::Pi, AgentCommand::Report(args)) = cli else {
        panic!("expected pi command");
    };
    assert_eq!(args.options.pi_path.as_deref(), Some("/tmp/pi-sessions"));

    let cli = parse_with_config(&["ccusage", "openclaw", "daily"], &config);
    let Command::Agent(StandardAgent::OpenClaw, AgentCommand::Report(args)) = cli else {
        panic!("expected openclaw command");
    };
    assert_eq!(
        args.options.open_claw_path.as_deref(),
        Some("/tmp/openclaw")
    );
}

#[test]
fn root_help_lists_agent_namespaces_without_nested_commands() {
    let help = help_text();
    let agents = [
        "claude", "codex", "opencode", "amp", "droid", "codebuff", "hermes", "pi", "goose", "kilo",
        "copilot", "gemini", "kimi", "qwen", "openclaw", "grok", "zcode", "cline", "dsh",
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
        "codex".to_string(),
        "daily".to_string(),
        "--help".to_string(),
    ]);

    assert!(help.contains("Show Codex token usage grouped by day"));
    assert!(help.contains("USAGE:\n  ccusage codex daily <OPTIONS>"));
    assert!(help.contains("choices: auto | standard | flex | fast"));
}

#[test]
fn contextual_codex_session_help_lists_id_option() {
    let help = help_text_for_args(&["codex".to_string(), "session".to_string()]);

    assert!(help.contains("-i, --id <id>"));
}

#[test]
fn contextual_agent_help_lists_agent_subcommands() {
    let help = help_text_for_args(&["claude".to_string()]);

    assert!(help.contains("USAGE:\n  ccusage claude <COMMANDS>"));
    assert!(help.contains("daily       Show usage report grouped by date"));
    assert!(help.contains("statusline  Display compact status line for Claude Code hooks"));
    assert!(help.contains("ccusage claude statusline --help"));
    assert!(!help.contains("ccusage claude daily <OPTIONS>"));
}

#[test]
fn contextual_all_agent_help_lists_color_options() {
    let help = help_text_for_args(&["daily".to_string()]);

    assert!(help.contains("--color"));
    assert!(help.contains("--no-color"));
}

#[test]
fn contextual_root_session_help_lists_id_option() {
    let help = help_text_for_args(&["session".to_string()]);

    assert!(help.contains("--id"));
}

#[test]
fn contextual_root_weekly_help_lists_start_of_week_option() {
    let help = help_text_for_args(&["weekly".to_string()]);

    assert!(help.contains("-w, --start-of-week [start-of-week]"));
    assert!(help.contains("default: monday"));

    let daily_help = help_text_for_args(&["daily".to_string()]);
    assert!(!daily_help.contains("--start-of-week"));
}

#[test]
fn parses_top_level_weekly_start_of_week() {
    let cli = parse(&[
        "ccusage",
        "weekly",
        "--json",
        "--offline",
        "--last",
        "1",
        "-w",
        "wednesday",
    ]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert_eq!(args.start_of_week, WeekDay::Wednesday);
    assert!(args.shared.json);
    assert!(args.shared.offline);
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(1));

    let cli = parse(&["ccusage", "weekly", "--start-of-week=friday"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.start_of_week, WeekDay::Friday);

    let cli = parse(&["ccusage", "weekly"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.start_of_week, WeekDay::Monday);
}

#[test]
fn applies_weekly_config_start_of_week_to_top_level_weekly() {
    let config = TestConfig {
        weekly_start: Some(WeekDay::Sunday),
        ..TestConfig::default()
    };

    let cli = parse_with_config(&["ccusage", "weekly"], &config);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.start_of_week, WeekDay::Sunday);

    let cli = parse_with_config(&["ccusage", "weekly", "-w", "tuesday"], &config);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.start_of_week, WeekDay::Tuesday);
}

#[test]
fn rejects_start_of_week_outside_weekly_reports() {
    assert_eq!(
        parse_error(&["ccusage", "daily", "-w", "monday"]),
        "Unknown option '-w'"
    );
    assert_eq!(
        parse_error(&["ccusage", "weekly", "-w", "someday"]),
        "Invalid week day 'someday'"
    );
}

#[test]
fn contextual_statusline_help_lists_choice_options() {
    let help = help_text_for_args(&["statusline".to_string()]);

    assert!(help.contains("choices: off | emoji | text | emoji-text"));
    assert!(help.contains("choices: auto | ccusage | cc | both"));
}

#[test]
fn grok_help_does_not_advertise_path_option() {
    let help = help_text_for_args(&["grok".to_string(), "daily".to_string()]);

    assert!(!help.contains("--grok-path"));
}

#[test]
fn snapshots_root_and_contextual_help_text() {
    insta::assert_snapshot!("root_help", help_text());
    insta::assert_snapshot!(
        "claude_agent_help",
        help_text_for_args(&["claude".to_string()])
    );
    insta::assert_snapshot!(
        "codex_daily_help",
        help_text_for_args(&["codex".to_string(), "daily".to_string(),])
    );
    insta::assert_snapshot!(
        "statusline_help",
        help_text_for_args(&["statusline".to_string()])
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
            "case": "grok daily",
            "cli": cli_snapshot(parse(&["ccusage", "grok", "daily", "--json"])),
        }),
        json!({
            "case": "antigravity session",
            "cli": cli_snapshot(parse(&["ccusage", "antigravity", "session", "--json"])),
        }),
        json!({
            "case": "zcode daily",
            "cli": cli_snapshot(parse(&["ccusage", "zcode", "daily", "--json"])),
        }),
        json!({
            "case": "cline weekly",
            "cli": cli_snapshot(parse(&["ccusage", "cline", "weekly", "--json"])),
        }),
        json!({
            "case": "dsh weekly",
            "cli": cli_snapshot(parse(&["ccusage", "dsh", "weekly", "--json"])),
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
    let Command::Daily(args) = cli else {
        panic!("expected daily command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.mode, CostMode::Display);
    assert!(args.instances);
    assert_eq!(args.project.as_deref(), Some("repo"));
}

#[test]
fn rejects_removed_locale_option() {
    let result = parse_from_with_config(
        ["ccusage", "--locale", "en-CA"]
            .into_iter()
            .map(OsString::from),
        &NoConfig,
        env!("CARGO_PKG_VERSION"),
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
    let Command::Blocks(args) = cli else {
        panic!("expected blocks command");
    };
    assert!(args.active);
    assert_eq!(args.token_limit, Some(TokenLimit::Max));
    assert_eq!(args.session_length.as_millis(), 21_600_000);
}

#[test]
fn parses_claude_blocks_short_active_option() {
    let cli = parse(&["ccusage", "claude", "blocks", "-a"]);
    let Command::Blocks(args) = cli else {
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
    let Command::Statusline(args) = cli else {
        panic!("expected statusline command");
    };
    assert!(args.offline);
    assert!(!args.cache);
    assert_eq!(
        args.timezone.as_ref().map(|timezone| timezone.name()),
        Some("Asia/Tokyo")
    );
    assert_eq!(args.visual_burn_rate, VisualBurnRate::EmojiText);
    assert_eq!(args.cost_source, CostSource::Both);
}

#[test]
fn statusline_keeps_configured_cost_mode_with_each_cost_source() {
    let fixture = fs_fixture!({"ccusage.json": r#"{"defaults":{"mode":"display"}}"#});
    let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    for (source, expected) in [
        ("auto", CostSource::Auto),
        ("ccusage", CostSource::Ccusage),
        ("cc", CostSource::Cc),
        ("both", CostSource::Both),
    ] {
        let args = ["statusline", "--config", &path, "--cost-source", source];
        let config = ccusage_config::ConfigContext::from_args(
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
        );
        let Command::Statusline(statusline) = parse_with_config(
            &[
                "ccusage",
                "statusline",
                "--config",
                &path,
                "--cost-source",
                source,
            ],
            &config,
        ) else {
            panic!("expected statusline");
        };
        assert_eq!(statusline.mode, CostMode::Display);
        assert_eq!(statusline.cost_source, expected);
    }
}

#[test]
fn parses_codex_default_daily_options() {
    let cli = parse(&["ccusage", "codex", "--json", "--since", "20260102"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
    assert_eq!(args.shared.since.as_deref(), Some("20260102"));
}

#[test]
fn parses_codex_speed_option() {
    let cli = parse(&["ccusage", "codex", "daily", "--speed", "fast"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.options.codex_speed, CodexSpeed::Fast);
}

#[test]
fn parses_codex_flex_speed_option() {
    let cli = parse(&["ccusage", "codex", "daily", "--speed", "flex"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex command");
    };
    assert_eq!(args.options.codex_speed, CodexSpeed::Flex);
}

#[test]
fn rejects_removed_codex_by_source_option() {
    assert_eq!(
        parse_error(&["ccusage", "codex", "daily", "--by-source"]),
        "Unknown codex option '--by-source'"
    );
    assert_eq!(
        parse_error(&["ccusage", "daily", "--by-source"]),
        "Unknown option '--by-source'"
    );
}

#[test]
fn parses_legacy_colon_agent_commands() {
    let cli = parse(&["ccusage", "codex:monthly", "--json"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
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
    let Command::Session(args) = cli else {
        panic!("expected claude session command");
    };
    assert!(args.shared.json);
    assert_eq!(args.id.as_deref(), Some("abc"));
}

#[test]
fn parses_top_level_session_id_lookup() {
    let cli = parse(&["ccusage", "session", "--json", "--id", "abc"]);
    let Command::Session(args) = cli else {
        panic!("expected session command");
    };
    assert!(args.shared.json);
    assert_eq!(args.id.as_deref(), Some("abc"));
}

#[test]
fn parses_codex_session_id_filter() {
    let cli = parse(&[
        "ccusage",
        "codex",
        "session",
        "--json",
        "--id",
        "01a0bb8c-c7c0-7630-9d82-860b875930f0",
    ]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Report(args)) = cli else {
        panic!("expected codex session command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(
        args.session_id.as_deref(),
        Some("01a0bb8c-c7c0-7630-9d82-860b875930f0")
    );
}

#[test]
fn rejects_codex_id_filter_outside_session_report() {
    let error = parse_error(&["ccusage", "codex", "daily", "--id", "abc"]);

    assert_eq!(error, "Unknown codex option '--id'");
}

#[test]
fn parses_opencode_weekly_options() {
    let cli = parse(&["ccusage", "opencode", "weekly", "--json"]);
    let Command::Agent(StandardAgent::OpenCode, AgentCommand::Report(args)) = cli else {
        panic!("expected opencode command");
    };
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert!(args.shared.json);
    assert!(!args.include_claude_agent);
}

#[test]
fn parses_opencode_include_claude_agent_on_every_report() {
    for report in [None, Some("daily"), Some("weekly"), Some("session")] {
        let mut argv = vec!["ccusage", "opencode"];
        argv.extend(report);
        argv.extend(["--include-claude-agent", "--json"]);
        let cli = parse(&argv);
        let Command::Agent(StandardAgent::OpenCode, AgentCommand::Report(args)) = cli else {
            panic!("expected opencode command");
        };
        assert!(args.include_claude_agent, "{report:?}");
        assert!(args.shared.json, "{report:?}");
    }
    assert_eq!(
        parse_error(&["ccusage", "amp", "--include-claude-agent"]),
        "Unknown option '--include-claude-agent'"
    );
}

#[test]
fn parses_cline_standard_reports() {
    for (report, kind) in [
        ("daily", AgentReportKind::Daily),
        ("weekly", AgentReportKind::Weekly),
        ("monthly", AgentReportKind::Monthly),
        ("session", AgentReportKind::Session),
    ] {
        let cli = parse(&["ccusage", "cline", report, "--json"]);
        let Command::Agent(StandardAgent::Cline, AgentCommand::Report(args)) = cli else {
            panic!("expected cline {report} command");
        };

        assert_eq!(args.kind, kind);
        assert!(args.shared.json);
    }
}

#[test]
fn parses_dsh_standard_reports() {
    for (report, kind) in [
        ("daily", AgentReportKind::Daily),
        ("weekly", AgentReportKind::Weekly),
        ("monthly", AgentReportKind::Monthly),
        ("session", AgentReportKind::Session),
    ] {
        let cli = parse(&["ccusage", "dsh", report, "--json"]);
        let Command::Agent(StandardAgent::Dsh, AgentCommand::Report(args)) = cli else {
            panic!("expected dsh {report} command");
        };

        assert_eq!(args.kind, kind);
        assert!(args.shared.json);
    }
}

#[test]
fn parses_amp_session_options() {
    let cli = parse(&["ccusage", "amp", "session", "--json"]);
    let Command::Agent(StandardAgent::Amp, AgentCommand::Report(args)) = cli else {
        panic!("expected amp command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_droid_session_options() {
    let cli = parse(&["ccusage", "droid", "session", "--json"]);
    let Command::Agent(StandardAgent::Droid, AgentCommand::Report(args)) = cli else {
        panic!("expected droid command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_codebuff_session_options() {
    let cli = parse(&["ccusage", "codebuff", "session", "--json"]);
    let Command::Agent(StandardAgent::Codebuff, AgentCommand::Report(args)) = cli else {
        panic!("expected codebuff command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_qwen_session_options() {
    let cli = parse(&["ccusage", "qwen", "session", "--json"]);
    let Command::Agent(StandardAgent::Qwen, AgentCommand::Report(args)) = cli else {
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
    let Command::Agent(StandardAgent::Pi, AgentCommand::Report(args)) = cli else {
        panic!("expected pi command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(args.options.pi_path.as_deref(), Some("/tmp/pi-sessions"));
}

#[test]
fn parses_kilo_session_options() {
    let cli = parse(&["ccusage", "kilo", "session", "--json"]);
    let Command::Agent(StandardAgent::Kilo, AgentCommand::Report(args)) = cli else {
        panic!("expected kilo command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_goose_session_options() {
    let cli = parse(&["ccusage", "goose", "session", "--json"]);
    let Command::Agent(StandardAgent::Goose, AgentCommand::Report(args)) = cli else {
        panic!("expected goose command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_copilot_session_options() {
    let cli = parse(&["ccusage", "copilot", "session", "--json"]);
    let Command::Agent(StandardAgent::Copilot, AgentCommand::Report(args)) = cli else {
        panic!("expected copilot command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_gemini_session_options() {
    let cli = parse(&["ccusage", "gemini", "session", "--json"]);
    let Command::Agent(StandardAgent::Gemini, AgentCommand::Report(args)) = cli else {
        panic!("expected gemini command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
}

#[test]
fn parses_kimi_session_options() {
    let cli = parse(&["ccusage", "kimi", "session", "--json"]);
    let Command::Agent(StandardAgent::Kimi, AgentCommand::Report(args)) = cli else {
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
    let Command::Agent(StandardAgent::OpenClaw, AgentCommand::Report(args)) = cli else {
        panic!("expected openclaw command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(
        args.options.open_claw_path.as_deref(),
        Some("/tmp/openclaw")
    );
}

#[test]
fn parses_grok_daily_options() {
    let cli = parse(&["ccusage", "grok", "daily", "--json"]);
    let Command::Agent(StandardAgent::Grok, AgentCommand::Report(args)) = cli else {
        panic!("expected grok command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
}

#[test]
fn parses_zcode_daily_options() {
    let cli = parse(&["ccusage", "zcode", "daily", "--json"]);
    let Command::Agent(StandardAgent::ZCode, AgentCommand::Report(args)) = cli else {
        panic!("expected zcode command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
}

#[test]
fn rejects_grok_path_option() {
    let error = parse_error(&["ccusage", "grok", "daily", "--grok-path", "/tmp/grok-home"]);

    assert_eq!(error, "Unknown option '--grok-path'");
}

#[test]
fn invalid_explicit_named_stores_fail_statusline() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "statusline".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let parsed = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
        env!("CARGO_PKG_VERSION"),
    );

    let Err(error) = parsed else {
        panic!("expected invalid explicit config to fail")
    };
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}

#[test]
fn named_pi_store_validation_does_not_break_statusline() {
    assert_invalid_discovered_stores_allow(&["statusline"]);
}

#[test]
fn named_pi_store_validation_does_not_break_agent_commands() {
    assert_invalid_discovered_stores_allow(&["codex", "daily"]);
}

fn assert_invalid_discovered_stores_allow(args: &[&str]) {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{"pi":{"stores":[{"name":"codex","path":"/tmp/omp"}]}}"#,
    });
    let _guard = ccusage_test_support::EnvGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let config = ccusage_config::ConfigContext::from_args(&args);
    let parsed = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage"))
            .chain(args.iter().map(std::ffi::OsString::from)),
        &config,
        env!("CARGO_PKG_VERSION"),
    );
    assert!(parsed.is_ok(), "{:?}", parsed.err());
}

#[test]
fn invalid_explicit_named_stores_fail_agent_commands() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "codex".to_string(),
        "daily".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let parsed = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
        env!("CARGO_PKG_VERSION"),
    );

    let Err(error) = parsed else {
        panic!("expected invalid explicit config to fail")
    };
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}

#[test]
fn reports_named_pi_store_validation_through_cli_config_error_path() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "daily".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let result = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
        env!("CARGO_PKG_VERSION"),
    );
    let Err(error) = result else {
        panic!("expected config error");
    };

    assert!(error.starts_with("Invalid ccusage config: "));
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}

#[test]
fn preserves_whether_report_order_was_supplied() {
    for (args, order, explicit) in [
        (vec!["ccusage", "session"], SortOrder::Asc, false),
        (vec!["ccusage", "daily"], SortOrder::Asc, false),
        (
            vec!["ccusage", "session", "--order", "asc"],
            SortOrder::Asc,
            true,
        ),
        (
            vec!["ccusage", "--order", "desc", "session"],
            SortOrder::Desc,
            true,
        ),
        (
            vec![
                "ccusage",
                "session",
                "-o",
                "asc",
                "--sections",
                "daily,weekly,monthly",
            ],
            SortOrder::Asc,
            true,
        ),
    ] {
        let Command::All(report) = parse(&args) else {
            panic!("expected unified report");
        };
        assert_eq!(report.shared.order, explicit.then_some(order));
    }
}

#[test]
fn preserves_configured_order_and_cli_precedence() {
    for (config_json, extra, expected, explicit) in [
        (r#"{}"#, vec![], SortOrder::Asc, false),
        (
            r#"{"defaults":{"order":"asc"}}"#,
            vec![],
            SortOrder::Asc,
            true,
        ),
        (
            r#"{"defaults":{"order":"desc"}}"#,
            vec![],
            SortOrder::Desc,
            true,
        ),
        (
            r#"{"defaults":{"order":"desc"},"commands":{"session":{"order":"asc"}}}"#,
            vec![],
            SortOrder::Asc,
            true,
        ),
        (
            r#"{"commands":{"session":{"order":"asc"}}}"#,
            vec!["--order", "desc"],
            SortOrder::Desc,
            true,
        ),
        (
            r#"{"defaults":{"order":"desc"}}"#,
            vec!["--order", "asc"],
            SortOrder::Asc,
            true,
        ),
    ] {
        let fixture = fs_fixture!({});
        fixture.write_file("ccusage.json", config_json);
        let config_path = fixture.path("ccusage.json").to_string_lossy().into_owned();
        let mut args = vec![
            "session",
            "--config",
            &config_path,
            "--sections",
            "daily,weekly,monthly",
        ];
        args.extend(extra);
        let config = ccusage_config::ConfigContext::from_args(
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
        );
        args.insert(0, "ccusage");
        let Command::All(report) = parse_with_config(&args, &config) else {
            panic!("expected unified report");
        };
        assert_eq!(
            report.shared.order,
            explicit.then_some(expected),
            "{config_json}"
        );
    }
}

#[test]
fn every_registered_weekly_agent_accepts_a_week_start() {
    for agent in AGENTS
        .iter()
        .filter(|agent| agent.reports.contains(&AgentReportKind::Weekly))
    {
        let command = parse(&["ccusage", agent.name, "weekly", "-w", "friday"]);
        assert_eq!(
            command.period(),
            Some((PeriodUnit::Week, WeekDay::Friday)),
            "{}",
            agent.name
        );
    }
}

#[test]
fn focused_week_starts_resolve_config_before_flags_for_every_weekly_agent() {
    for agent in AGENTS
        .iter()
        .filter(|agent| agent.reports.contains(&AgentReportKind::Weekly))
    {
        let config = TestConfig {
            weekly_start: Some(WeekDay::Tuesday),
            ..TestConfig::default()
        };
        let configured = parse_with_config(&["ccusage", agent.name, "weekly"], &config);
        assert_eq!(
            configured.period(),
            Some((PeriodUnit::Week, WeekDay::Tuesday)),
            "{}",
            agent.name
        );
        let flagged = parse_with_config(
            &[
                "ccusage",
                agent.name,
                "weekly",
                "--start-of-week",
                "saturday",
            ],
            &config,
        );
        assert_eq!(
            flagged.period(),
            Some((PeriodUnit::Week, WeekDay::Saturday)),
            "{}",
            agent.name
        );
    }
}

#[test]
fn statusline_accepts_valid_root_flags() {
    for flags in [
        vec!["--json"],
        vec!["--no-color"],
        vec!["--color"],
        vec!["--compact"],
        vec!["--mode", "calculate"],
        vec!["--order", "asc"],
        vec!["--last", "2"],
        vec!["--since", "20260101"],
        vec!["--until", "20260102"],
        vec!["--breakdown"],
        vec!["--no-cost"],
        vec!["--single-thread"],
        vec!["--sections", "daily,monthly"],
        vec!["--by-agent"],
    ] {
        for namespace in [vec!["statusline"], vec!["claude", "statusline"]] {
            let mut argv = vec!["ccusage", "--debug"];
            argv.extend(flags.clone());
            argv.extend(namespace);
            let Command::Statusline(args) = parse(&argv) else {
                panic!("expected statusline");
            };
            assert!(args.debug);
        }
    }
}

#[test]
fn option_metadata_matches_every_documented_flag_and_arity() {
    use ccusage_cli::OptionArity;
    use std::collections::{BTreeMap, BTreeSet};
    let help: Value = serde_json::from_str(include_str!("cli-help.json")).unwrap();
    let mut arities = BTreeMap::<&str, BTreeSet<bool>>::new();
    for group in help.as_object().unwrap().values() {
        for spec in group.as_array().unwrap() {
            let flags = spec["flags"].as_str().unwrap();
            let takes_value = flags.contains(['<', '[']);
            for alias in flags.split(',') {
                arities
                    .entry(alias.split_whitespace().next().unwrap())
                    .or_default()
                    .insert(takes_value);
            }
        }
    }
    for (flag, arities) in arities {
        let expected = match arities.into_iter().collect::<Vec<_>>().as_slice() {
            [false] => OptionArity::Switch,
            [true] => OptionArity::Value,
            [false, true] => OptionArity::CommandDependent,
            _ => panic!("missing arity"),
        };
        assert_eq!(ccusage_cli::option_arity(flag), Some(expected), "{flag}");
    }
}

#[test]
fn parses_codex_statusline_with_offline_pricing_by_default() {
    let cli = parse(&["ccusage", "codex", "statusline"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert!(args.shared.offline);
    assert!(args.session_id.is_none());

    let cli = parse(&[
        "ccusage",
        "codex",
        "statusline",
        "--id",
        "thread",
        "--speed",
        "fast",
        "--no-offline",
    ]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert_eq!(args.session_id.as_deref(), Some("thread"));
    assert_eq!(args.codex_speed, CodexSpeed::Fast);
    assert!(!args.shared.offline);
    assert!(
        parse_error(&["ccusage", "codex", "statusline", "--last", "1"])
            .contains("only available for the daily")
    );
}

#[test]
fn codex_statusline_config_overrides_defaults_and_flags_override_config() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "codex": { "commands": { "statusline": {
            "offline": false, "speed": "flex", "timezone": "Asia/Tokyo"
        } } } }"#,
    });
    let path = fixture.path("ccusage.json").to_string_lossy().into_owned();
    let args = ["codex", "statusline", "--config", &path];
    let config = ccusage_config::ConfigContext::from_args(&args.map(str::to_string));
    let cli = parse_with_config(&[&["ccusage"][..], &args].concat(), &config);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert!(!args.shared.offline);
    assert_eq!(args.codex_speed, CodexSpeed::Flex);
    assert_eq!(
        args.shared
            .timezone
            .as_ref()
            .map(|timezone| timezone.name()),
        Some("Asia/Tokyo")
    );

    let cli = parse_with_config(
        &[
            "ccusage",
            "codex",
            "statusline",
            "--offline",
            "--speed",
            "standard",
        ],
        &config,
    );
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert!(args.shared.offline);
    assert_eq!(args.codex_speed, CodexSpeed::Standard);
}

#[test]
fn codex_statusline_help_describes_standalone_output() {
    insta::assert_snapshot!(
        "codex_statusline_help",
        help_text_for_args(&["codex".into(), "statusline".into()])
    );
    let help = help_text_for_args(&["codex".into()]);
    assert!(help.contains("ccusage codex statusline --help"));
}

#[test]
fn codex_statusline_rejects_explicit_report_flags() {
    for (flag, value) in [("--last", "1"), ("--sections", "daily")] {
        for options in [
            ["ccusage", flag, value, "codex", "statusline"],
            ["ccusage", "codex", "statusline", flag, value],
        ] {
            let error = parse_error(&options);
            assert!(error.contains(flag), "{error}");
        }
    }
    for flag in ["--all", "--by-agent"] {
        let error = parse_error(&["ccusage", flag, "codex", "statusline"]);
        assert!(error.contains(flag), "{error}");
    }
}

#[test]
fn codex_statusline_rejects_explicit_reversed_windows_in_every_scope() {
    for scope in [&[][..], &["--id", "thread"], &["--mcp"]] {
        let args = [
            "ccusage",
            "-s=20260605",
            "codex",
            "statusline",
            "--until",
            "20260603",
        ];
        let error = parse_error(&[args.as_slice(), scope].concat());
        assert_eq!(
            error,
            "The --since date '20260605' is later than the --until date '20260603'."
        );
    }
}
