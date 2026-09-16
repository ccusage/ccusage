use ccusage_cli::{Command, SharedArgs};
use ccusage_core::is_valid_timezone;

use super::Cli;

/// A timezone can arrive from `--timezone` or from a config file, and both
/// land on the parsed args before this runs. Checking here, once, rejects an
/// unknown name instead of letting `parse_tz` fall back to the local zone,
/// which can group usage under the wrong date.
pub(crate) fn validate(cli: &Cli) -> Result<(), String> {
    match effective_timezone(cli) {
        Some(timezone) if !is_valid_timezone(timezone) => Err(format!(
            "Invalid value for --timezone '{timezone}'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
        )),
        _ => Ok(()),
    }
}

/// The parser clones the root options into the command before command flags
/// apply, so only the command's copy is current once a command is present.
fn effective_timezone(cli: &Cli) -> Option<&str> {
    match cli.command.as_ref() {
        None => cli.shared.timezone.as_deref(),
        Some(Command::Statusline(args)) => args.timezone.as_deref(),
        Some(command) => command_shared(command).timezone.as_deref(),
    }
}

fn command_shared(command: &Command) -> &SharedArgs {
    match command {
        Command::Daily(args) => &args.shared,
        Command::Monthly(shared) => shared,
        Command::Weekly(args) => &args.shared,
        Command::Session(args) => &args.shared,
        Command::Blocks(args) => &args.shared,
        Command::All(args)
        | Command::Codex(args)
        | Command::OpenCode(args)
        | Command::Amp(args)
        | Command::Droid(args)
        | Command::Codebuff(args)
        | Command::Hermes(args)
        | Command::Pi(args)
        | Command::Goose(args)
        | Command::Kilo(args)
        | Command::Copilot(args)
        | Command::Gemini(args)
        | Command::Antigravity(args)
        | Command::Kimi(args)
        | Command::Qwen(args)
        | Command::OpenClaw(args)
        | Command::Grok(args)
        | Command::ZCode(args) => &args.shared,
        Command::Statusline(_) => unreachable!("statusline keeps its own timezone field"),
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
        let cli = Cli::parse_from_with_config(
            std::iter::once(OsString::from("ccusage"))
                .chain(args.iter().map(|arg| OsString::from(arg.as_str()))),
            &config,
            5.0,
            "test",
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
