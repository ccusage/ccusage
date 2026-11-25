use super::*;

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
fn statusline_accepts_the_short_debug_flag() {
    for namespace in [&["statusline"][..], &["claude", "statusline"][..]] {
        let argv = [&["ccusage"][..], namespace, &["-d"]].concat();
        let Command::Statusline(args) = parse(&argv) else {
            panic!("expected statusline");
        };
        assert!(args.debug);
    }
}

#[test]
fn parses_codex_statusline_with_offline_pricing_by_default() {
    let cli = parse(&["ccusage", "codex", "statusline"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert!(args.shared.offline);
    assert!(!args.shared.offline_selected);
    assert!(args.session_id.is_none());

    let cli = parse(&["ccusage", "codex", "statusline", "--offline"]);
    let Command::Agent(StandardAgent::Codex, AgentCommand::Statusline(args)) = cli else {
        panic!("expected codex statusline command");
    };
    assert!(args.shared.offline_selected);

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

#[test]
fn statusline_offline_default_is_not_a_user_selection() {
    let Command::Statusline(args) = parse(&["ccusage", "statusline"]) else {
        panic!("expected statusline");
    };
    assert!(args.offline);
    assert!(!args.offline_selected);

    for argv in [
        &["ccusage", "statusline", "--offline"][..],
        &["ccusage", "--offline", "statusline"][..],
    ] {
        let Command::Statusline(args) = parse(argv) else {
            panic!("expected statusline");
        };
        assert!(args.offline_selected, "{argv:?}");
    }
}
