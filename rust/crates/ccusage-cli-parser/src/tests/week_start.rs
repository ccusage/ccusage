use super::*;

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
fn every_agent_accepts_a_week_start() {
    for agent in AGENTS {
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
    for agent in AGENTS {
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
