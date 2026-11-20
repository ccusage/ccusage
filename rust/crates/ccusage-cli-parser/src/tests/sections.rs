use super::*;

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
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260102")
    );
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
