use super::*;

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
        assert_eq!(
            args.shared.since.map(|date| date.to_string()).as_deref(),
            Some(normalized)
        );
        assert_eq!(
            args.shared.until.map(|date| date.to_string()).as_deref(),
            Some(normalized)
        );
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
    let Command::Claude {
        report: ClaudeReport::Weekly(args),
        ..
    } = cli
    else {
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
    let Command::Claude {
        report: ClaudeReport::Daily(args),
        ..
    } = cli
    else {
        panic!("expected claude daily command");
    };
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260914")
    );
    assert_eq!(
        args.shared.until.map(|date| date.to_string()).as_deref(),
        Some("20260914")
    );
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
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260914")
    );
    assert_eq!(
        args.shared.until.map(|date| date.to_string()).as_deref(),
        Some("20260930")
    );
}

#[test]
fn invalid_config_timezone_names_the_file_and_effective_field() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "defaults": { "timezone": "UTC" }, "commands": { "daily": { "timezone": "Not/AZone" } } }"#,
    });
    let path = fixture.path("ccusage.json");
    let args = ["daily", "--config", path.to_str().unwrap()].map(str::to_owned);
    let config = ccusage_config::ConfigContext::from_args(&args);
    let result = parse_from_with_config(
        std::iter::once(OsString::from("ccusage")).chain(args.iter().map(OsString::from)),
        &config,
    );

    let Err(error) = result else {
        panic!("expected the configured timezone to be rejected");
    };
    assert_eq!(
        error,
        format!(
            "Invalid value for ccusage config field {}: commands.daily.timezone 'Not/AZone'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone.",
            path.display()
        )
    );
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
fn rejects_last_periods_alongside_every_spelling_of_the_date_flags() {
    let cases: [&[&str]; 4] = [
        &["ccusage", "daily", "--last", "1", "-s", "20260101"],
        &["ccusage", "daily", "--last", "1", "-u", "20260101"],
        &["ccusage", "daily", "--last", "1", "--until=20260101"],
        &["ccusage", "--since", "20260101", "daily", "--last", "1"],
    ];
    for args in cases {
        assert_eq!(
            parse_error(args),
            "The --last option cannot be combined with --since or --until.",
            "{args:?}"
        );
    }
}

#[test]
fn last_periods_replace_a_configured_date_window() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "defaults": { "since": "20260101", "until": "20260531" } }"#,
    });
    let path = fixture.path("ccusage.json");
    let args = ["daily", "--last", "1", "--config", path.to_str().unwrap()];
    let config = ccusage_config::ConfigContext::from_args(&args.map(str::to_string));

    let cli = parse_from_with_config(
        std::iter::once(OsString::from("ccusage")).chain(args.map(OsString::from)),
        &config,
    )
    .unwrap();

    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.shared.last.map(NonZeroU32::get), Some(1));
    assert_eq!(args.shared.since, None);
    assert_eq!(args.shared.until, None);
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
