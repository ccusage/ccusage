use ccusage_cli::Command;
use ccusage_report::is_valid_timezone;

/// A timezone can arrive from `--timezone` or from a config file, and both
/// land on the parsed args before this runs. Checking here, once, rejects an
/// unknown name instead of letting `resolve_tz` fall back to the local zone,
/// which can group usage under the wrong date.
pub(crate) fn validate(command: &Command) -> Result<(), String> {
    match effective_timezone(command) {
        Some(timezone) if !is_valid_timezone(timezone) => Err(format!(
            "Invalid value for --timezone '{timezone}'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
        )),
        _ => Ok(()),
    }
}

fn effective_timezone(command: &Command) -> Option<&str> {
    match command {
        Command::Statusline(args) => args.timezone.as_deref(),
        command => command
            .shared()
            .and_then(|shared| shared.timezone.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_config::ConfigContext;
    use ccusage_test_support::fs_fixture;

    use super::*;

    fn validated(args: &[&str]) -> Result<(), String> {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let config = ConfigContext::from_args(&args);
        let cli = ccusage_cli_parser::parse_from_with_config(
            std::iter::once(OsString::from("ccusage"))
                .chain(args.iter().map(|arg| OsString::from(arg.as_str()))),
            &config,
            5.0,
            "test",
            None,
        )
        .unwrap();
        validate(&cli)
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

        for args in [
            &["daily", "--config", &config][..],
            &["claude", "weekly", "--config", &config],
            &["statusline", "--config", &config],
        ] {
            assert_eq!(validated(args), rejection("Not/AZone"), "{args:?}");
        }
        for args in [
            &["daily", "--config", &config, "--timezone", "UTC"][..],
            &["--timezone", "UTC", "daily", "--config", &config],
            &["statusline", "--config", &config, "-z", "UTC"],
        ] {
            assert_eq!(validated(args), Ok(()), "{args:?}");
        }
    }
}
