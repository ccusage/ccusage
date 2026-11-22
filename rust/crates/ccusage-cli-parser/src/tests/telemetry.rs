use std::{ffi::OsString, ops::ControlFlow};

use ccusage_cli::{Command, NoConfig, TelemetryAction};

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
