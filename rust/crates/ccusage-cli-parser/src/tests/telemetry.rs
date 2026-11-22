use std::{ffi::OsString, ops::ControlFlow};

use ccusage_cli::{CliConfig, Command, NoConfig, TelemetryAction};

use crate::{parse_cli, parse_from_with_config};

fn parse(args: &[&str]) -> Command {
    parse_from_with_config(args.iter().map(OsString::from), &NoConfig).unwrap()
}

fn parse_error(args: &[&str]) -> String {
    match parse_from_with_config(args.iter().map(OsString::from), &NoConfig) {
        Ok(_) => panic!("expected parse error"),
        Err(error) => error,
    }
}

#[test]
fn parses_telemetry_actions_with_status_as_the_default() {
    for (args, expected) in [
        (&["ccusage", "telemetry"][..], TelemetryAction::Status),
        (
            &["ccusage", "telemetry", "status"][..],
            TelemetryAction::Status,
        ),
        (
            &["ccusage", "telemetry", "enable"][..],
            TelemetryAction::Enable,
        ),
        (
            &["ccusage", "telemetry", "disable"][..],
            TelemetryAction::Disable,
        ),
    ] {
        assert!(matches!(parse(args), Command::Telemetry(action) if action == expected));
    }
}

#[test]
fn rejects_unknown_telemetry_actions_and_extra_arguments() {
    assert_eq!(
        parse_error(&["ccusage", "telemetry", "off"]),
        "Unknown telemetry action 'off'. Expected status, enable, or disable."
    );
    assert_eq!(
        parse_error(&["ccusage", "telemetry", "enable", "now"]),
        "Unexpected argument 'now'"
    );
}

#[test]
fn telemetry_preferences_bypass_unusable_explicit_config_files() {
    let fixture = ccusage_test_support::fs_fixture!({
        "cwd/.keep": "",
        "invalid.json": "not json",
    });
    let missing_path = fixture.path("missing.json");
    let unreadable_path = fixture.path("unreadable.json");
    let invalid_path = fixture.path("invalid.json");
    std::fs::create_dir(&unreadable_path).unwrap();

    for path in [missing_path, unreadable_path, invalid_path] {
        let config_path = path.to_str().unwrap();
        let args = ["ccusage", "--config", config_path, "telemetry", "disable"];
        let config = ccusage_config::ConfigContext::from_args(
            &args[1..]
                .iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(config.config_error().is_none());

        let report_args = ["ccusage", "--config", config_path, "daily"];
        let report_config = ccusage_config::ConfigContext::from_args(
            &report_args[1..]
                .iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(report_config.config_error().is_some());
        assert!(matches!(
            parse_from_with_config(args.iter().map(OsString::from), &config),
            Ok(Command::Telemetry(TelemetryAction::Disable))
        ));
        assert!(matches!(
            parse_from_with_config(args.iter().map(OsString::from), &report_config),
            Ok(Command::Telemetry(TelemetryAction::Disable))
        ));
        assert!(matches!(
            parse_from_with_config(report_args.iter().map(OsString::from), &report_config),
            Err(error) if error.starts_with("Invalid ccusage config: ")
        ));
    }
}

#[test]
fn telemetry_preferences_bypass_invalid_report_date_defaults() {
    let fixture = ccusage_test_support::fs_fixture!({
        "ccusage.json": r#"{"defaults":{"since":"2026-02-30"}}"#,
    });
    let config_file = fixture.path("ccusage.json");
    let config_path = config_file.to_str().unwrap();
    let args = ["ccusage", "--config", config_path, "telemetry", "disable"];
    let config = ccusage_config::ConfigContext::from_args(
        &args[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>(),
    );

    assert!(config.config_error().is_none());

    let report_args = ["ccusage", "--config", config_path, "daily"];
    let report_config = ccusage_config::ConfigContext::from_args(
        &report_args[1..]
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>(),
    );
    assert!(report_config.config_error().is_some());
    assert!(matches!(
        parse_from_with_config(args.iter().map(OsString::from), &config),
        Ok(Command::Telemetry(TelemetryAction::Disable))
    ));
    assert!(matches!(
        parse_from_with_config(args.iter().map(OsString::from), &report_config),
        Ok(Command::Telemetry(TelemetryAction::Disable))
    ));
    assert!(matches!(
        parse_from_with_config(report_args.iter().map(OsString::from), &report_config),
        Err(error) if error.contains("defaults.since: not a valid date")
    ));
}

#[test]
fn renders_telemetry_help_from_the_command_spec() {
    let output = parse_cli(
        ["ccusage", "telemetry", "--help"].map(OsString::from),
        &NoConfig,
        "test",
    )
    .unwrap();
    let ControlFlow::Break(help) = output else {
        panic!("expected help output");
    };

    assert!(help.contains("ccusage telemetry <ACTION>"));
    assert!(help.contains("disable"));
}
