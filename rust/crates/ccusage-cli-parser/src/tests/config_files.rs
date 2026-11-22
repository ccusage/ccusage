use super::*;

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
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260102")
    );
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
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260101")
    );
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
    let Command::Claude {
        report: ClaudeReport::Weekly(args),
        ..
    } = cli
    else {
        panic!("expected weekly command");
    };
    assert!(args.shared.json);
    assert!(args.shared.compact);
    assert_eq!(args.start_of_week, WeekDay::Monday);

    let cli = parse_with_config(&["ccusage", "claude", "blocks"], &config);
    let Command::Claude {
        report: ClaudeReport::Blocks(args),
        ..
    } = cli
    else {
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
