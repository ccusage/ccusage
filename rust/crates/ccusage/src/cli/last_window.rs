use ccusage_cli::{AgentCommandArgs, AgentReportKind, Command, SharedArgs, WeekDay};
use ccusage_core::{PeriodUnit, format_date, last_periods_since, utc_now};

use super::Cli;

/// Turns `--last N` into the `--since` bound every loader already understands.
///
/// The count means "the most recent N periods of the report's own unit", so it
/// can only be resolved once the command is known: `daily --last 1` is today,
/// `weekly --last 1` is the current week, `monthly --last 1` is this month.
pub(crate) fn resolve(cli: &mut Cli) -> Result<(), String> {
    let Some((shared, unit, start_of_week)) = window_target(cli) else {
        return Ok(());
    };
    let Some(last) = shared.last else {
        return Ok(());
    };
    let today = format_date(utc_now(), shared.timezone.as_deref());
    let Some(since) = last_periods_since(unit, last, &today, start_of_week) else {
        return Err(format!("Could not resolve --last {last} from today's date"));
    };
    shared.since = Some(since);
    Ok(())
}

fn window_target(cli: &mut Cli) -> Option<(&mut SharedArgs, PeriodUnit, WeekDay)> {
    match cli.command.as_mut() {
        // A bare `ccusage` is the unified daily report.
        None => Some((&mut cli.shared, PeriodUnit::Day, UNIFIED_WEEK_START)),
        Some(Command::Daily(args)) => Some((&mut args.shared, PeriodUnit::Day, UNIFIED_WEEK_START)),
        Some(Command::Monthly(shared)) => Some((shared, PeriodUnit::Month, UNIFIED_WEEK_START)),
        Some(Command::Weekly(args)) => {
            let start_of_week = args.start_of_week;
            Some((&mut args.shared, PeriodUnit::Week, start_of_week))
        }
        Some(
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
            | Command::Kimi(args)
            | Command::Qwen(args)
            | Command::OpenClaw(args)
            | Command::Grok(args),
        ) => agent_window_target(args),
        Some(Command::Session(_) | Command::Blocks(_) | Command::Statusline(_)) => None,
    }
}

/// Every report that buckets by week outside of `ccusage claude weekly` starts
/// the week on Monday, so the window has to as well.
const UNIFIED_WEEK_START: WeekDay = WeekDay::Monday;

fn agent_window_target(
    args: &mut AgentCommandArgs,
) -> Option<(&mut SharedArgs, PeriodUnit, WeekDay)> {
    let unit = match args.kind {
        AgentReportKind::Daily => PeriodUnit::Day,
        AgentReportKind::Weekly => PeriodUnit::Week,
        AgentReportKind::Monthly => PeriodUnit::Month,
        AgentReportKind::Session => return None,
    };
    Some((&mut args.shared, unit, UNIFIED_WEEK_START))
}

#[cfg(test)]
mod tests {
    use ccusage_cli::{CodexSpeed, SortOrder, WeeklyArgs};

    use super::*;

    fn shared_with_last(last: u32) -> SharedArgs {
        SharedArgs {
            last: Some(last),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::with_defaults()
        }
    }

    fn agent_command(kind: AgentReportKind, last: u32) -> AgentCommandArgs {
        AgentCommandArgs {
            shared: shared_with_last(last),
            kind,
            sections: None,
            by_agent: false,
            by_source: false,
            pi_path: None,
            open_claw_path: None,
            codex_speed: CodexSpeed::Auto,
        }
    }

    fn resolved_since(command: Command) -> Option<String> {
        let mut cli = Cli {
            command: Some(command),
            shared: SharedArgs::with_defaults(),
        };
        resolve(&mut cli).unwrap();
        match cli.command {
            Some(Command::All(args)) | Some(Command::Codex(args)) => args.shared.since,
            Some(Command::Weekly(args)) => args.shared.since,
            Some(Command::Monthly(shared)) => shared.since,
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn resolves_the_unified_daily_window_to_today() {
        let today = format_date(utc_now(), Some("UTC")).replace('-', "");

        let since = resolved_since(Command::All(agent_command(AgentReportKind::Daily, 1)));

        assert_eq!(since.as_deref(), Some(today.as_str()));
    }

    #[test]
    fn resolves_agent_report_kinds_to_their_own_period() {
        let weekly = resolved_since(Command::Codex(agent_command(AgentReportKind::Weekly, 1)));
        let monthly = resolved_since(Command::Codex(agent_command(AgentReportKind::Monthly, 1)));

        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            weekly,
            last_periods_since(PeriodUnit::Week, 1, &today, WeekDay::Monday)
        );
        assert_eq!(
            monthly,
            last_periods_since(PeriodUnit::Month, 1, &today, WeekDay::Monday)
        );
    }

    #[test]
    fn keeps_the_claude_weekly_start_of_week() {
        let since = resolved_since(Command::Weekly(WeeklyArgs {
            shared: shared_with_last(1),
            start_of_week: WeekDay::Sunday,
        }));

        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            since,
            last_periods_since(PeriodUnit::Week, 1, &today, WeekDay::Sunday)
        );
    }

    #[test]
    fn leaves_commands_without_last_untouched() {
        let mut cli = Cli {
            command: Some(Command::Monthly(SharedArgs {
                order: SortOrder::Desc,
                ..SharedArgs::with_defaults()
            })),
            shared: SharedArgs::with_defaults(),
        };

        resolve(&mut cli).unwrap();

        let Some(Command::Monthly(shared)) = cli.command else {
            panic!("expected monthly command");
        };
        assert!(shared.since.is_none());
    }

    #[test]
    fn resolves_the_bare_command_as_a_daily_window() {
        let mut cli = Cli {
            command: None,
            shared: shared_with_last(2),
        };

        resolve(&mut cli).unwrap();

        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            cli.shared.since,
            last_periods_since(PeriodUnit::Day, 2, &today, WeekDay::Monday)
        );
    }
}
