use super::*;

#[test]
fn parses_root_daily_as_all_agent_report() {
    let cli = parse(&["ccusage", "daily", "--json", "--since", "20260102"]);
    let Command::All(args) = cli else {
        panic!("expected all-agent command");
    };
    assert_eq!(args.kind, AgentReportKind::Daily);
    assert!(args.shared.json);
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260102")
    );
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
        WeekDay::Monday,
        None,
    )
    .unwrap();
    assert_eq!(args.kind, AgentReportKind::Weekly);
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260102")
    );
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
                let (offline, selected) = if let Command::Statusline(args) = &command {
                    (args.offline, args.offline_selected)
                } else {
                    let shared = command.shared().unwrap();
                    (shared.offline, shared.offline_selected)
                };
                assert_eq!(offline, expected, "{argv:?}, {config_json}");
                assert_eq!(selected, expected, "{argv:?}, {config_json}");
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
        let command = parse_with_config(&argv, &config);
        if let Command::Statusline(args) = &command {
            assert!(args.offline);
            assert!(!args.cache);
        } else {
            let shared = command.shared().unwrap();
            assert!(shared.offline);
            assert_eq!(shared.color, ColorChoice::Never);
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
        if let Command::Statusline(args) = &command {
            assert!(args.offline);
            assert!(args.cache);
        } else {
            let shared = command.shared().unwrap();
            assert!(shared.offline);
            assert_eq!(shared.color, ColorChoice::Always);
        }
    }
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
