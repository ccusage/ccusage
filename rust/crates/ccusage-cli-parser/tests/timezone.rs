use std::ffi::OsString;

use ccusage_config::ConfigContext;
use ccusage_test_support::fs_fixture;

fn validated(args: &[&str]) -> Result<(), String> {
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let config = ConfigContext::from_args(&args);
    ccusage_cli_parser::parse_from_with_config(
        std::iter::once(OsString::from("ccusage"))
            .chain(args.iter().map(|arg| OsString::from(arg.as_str()))),
        &config,
    )
    .map(|_| ())
}

fn rejection(timezone: &str) -> Result<(), String> {
    Err(format!(
        "Invalid value for --timezone '{timezone}'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
    ))
}

#[test]
fn rejects_unknown_timezones_wherever_they_are_given() {
    for args in [
        &["--timezone", "Not/AZone"][..],
        &["daily", "-z", "Not/AZone"],
        &["claude", "daily", "--timezone", "Not/AZone"],
        &["claude", "session", "--timezone", "Not/AZone"],
        &["codex", "monthly", "--timezone", "Not/AZone"],
        &["codex", "statusline", "--timezone", "Not/AZone"],
        &["cline", "weekly", "--timezone", "Not/AZone"],
        &["blocks", "--timezone", "Not/AZone"],
        &["--timezone", "Not/AZone", "statusline"],
        &["statusline", "--timezone", "Not/AZone"],
    ] {
        assert_eq!(validated(args), rejection("Not/AZone"), "{args:?}");
    }
}

#[test]
fn accepts_known_timezones_local_and_no_timezone() {
    for args in [
        &[][..],
        &["--timezone", "UTC"],
        &["daily", "-z", "Asia/Tokyo"],
        &["daily", "-z", "local"],
        &["--timezone", "UTC", "statusline"],
        &["statusline", "--timezone", "America/New_York"],
    ] {
        assert_eq!(validated(args), Ok(()), "{args:?}");
    }
}

#[test]
fn checks_only_the_timezone_the_command_will_use() {
    assert_eq!(
        validated(&["--timezone", "Not/AZone", "daily", "-z", "UTC"]),
        Ok(())
    );
    assert_eq!(
        validated(&["--timezone", "UTC", "daily", "-z", "Not/AZone"]),
        rejection("Not/AZone")
    );
}

#[test]
fn checks_config_timezones_after_flags_override_them() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "defaults": { "timezone": "Not/AZone" } }"#,
    });
    let config = fixture.path("ccusage.json").to_string_lossy().into_owned();
    let configured_rejection = Err(format!(
        "Invalid value for ccusage config field {config}: defaults.timezone 'Not/AZone'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
    ));

    for args in [
        &["daily", "--config", &config][..],
        &["claude", "weekly", "--config", &config],
        &["statusline", "--config", &config],
    ] {
        assert_eq!(validated(args), configured_rejection, "{args:?}");
    }
    for args in [
        &["daily", "--config", &config, "--timezone", "UTC"][..],
        &["--timezone", "UTC", "daily", "--config", &config],
        &["statusline", "--config", &config, "-z", "UTC"],
    ] {
        assert_eq!(validated(args), Ok(()), "{args:?}");
    }
}
