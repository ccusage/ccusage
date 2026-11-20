use super::*;

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
    use ccusage_test_support::Environment;
    let fixture = fs_fixture!({
        "home/.claude/ccusage.json": r#"{"commands":{"statusline":{"contextLowThreshold":85}}}"#,
        "explicit.json": r#"{"commands":{"statusline":{"contextLowThreshold":85}}}"#,
    });
    Environment::with_vars([
        ("HOME", Some(fixture.path("home").into_os_string())),
        (
            "CLAUDE_CONFIG_DIR",
            Some(fixture.path("home/.claude").into_os_string()),
        ),
        (
            "XDG_CONFIG_HOME",
            Some(fixture.path("home/.config").into_os_string()),
        ),
    ])
    .run(|| {
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
    })
}
