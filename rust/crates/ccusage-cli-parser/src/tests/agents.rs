use super::*;

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
    let Command::Claude {
        report: ClaudeReport::Daily(args),
        project,
    } = cli
    else {
        panic!("expected daily command");
    };
    assert!(args.shared.json);
    assert_eq!(args.shared.mode, CostMode::Display);
    assert!(args.instances);
    assert_eq!(project.as_deref(), Some("repo"));
}

#[test]
fn rejects_removed_locale_option() {
    let result = parse_from_with_config(
        ["ccusage", "--locale", "en-CA"]
            .into_iter()
            .map(OsString::from),
        &NoConfig,
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
    let Command::Claude {
        report: ClaudeReport::Blocks(args),
        ..
    } = cli
    else {
        panic!("expected blocks command");
    };
    assert!(args.active);
    assert_eq!(args.token_limit, Some(TokenLimit::Max));
    assert_eq!(args.session_length.as_millis(), 21_600_000);
}

#[test]
fn parses_claude_blocks_short_active_option() {
    let cli = parse(&["ccusage", "claude", "blocks", "-a"]);
    let Command::Claude {
        report: ClaudeReport::Blocks(args),
        ..
    } = cli
    else {
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
    assert_eq!(
        args.shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260102")
    );
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
    let Command::Claude {
        report: ClaudeReport::Session(args),
        ..
    } = cli
    else {
        panic!("expected claude session command");
    };
    assert!(args.shared.json);
    assert_eq!(args.id.as_deref(), Some("abc"));
}

#[test]
fn parses_top_level_session_id_lookup() {
    let cli = parse(&["ccusage", "session", "--json", "--id", "abc"]);
    let Command::Claude {
        report: ClaudeReport::Session(args),
        ..
    } = cli
    else {
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
        "--pi-agent-dir",
        "/tmp/pi-agent",
    ]);
    let Command::Agent(StandardAgent::Pi, AgentCommand::Report(args)) = cli else {
        panic!("expected pi command");
    };
    assert_eq!(args.kind, AgentReportKind::Session);
    assert!(args.shared.json);
    assert_eq!(args.options.pi_path.as_deref(), Some("/tmp/pi-sessions"));
    assert_eq!(args.options.pi_agent_dir.as_deref(), Some("/tmp/pi-agent"));
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
