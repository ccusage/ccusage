use ccusage_cli::{SharedArgs, WeekDay};
use ccusage_core::utc_now;
use ccusage_report::{PeriodUnit, format_date, last_periods_since};

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
        None => Some((&mut cli.shared, PeriodUnit::Day, WeekDay::Monday)),
        Some(command) => {
            let (unit, start_of_week) = command.period()?;
            Some((command.shared_mut()?, unit, start_of_week))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ccusage_cli::{
        AgentCommandArgs, AgentReportKind, CodexSpeed, Command, SortOrder, WeeklyArgs,
    };

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
            session_id: None,
            sections: None,
            by_agent: false,
            pi_path: None,
            open_claw_path: None,
            codex_speed: CodexSpeed::Auto,
            include_claude_agent: false,
            start_of_week: WeekDay::Monday,
            sources: BTreeMap::new(),
        }
    }

    fn resolved_since(command: Command) -> Option<String> {
        let mut cli = Cli {
            command: Some(command),
            shared: SharedArgs::with_defaults(),
        };
        resolve(&mut cli).unwrap();
        match cli.command {
            Some(Command::All(args))
            | Some(Command::Agent(ccusage_cli::AgentId::Codex, args))
            | Some(Command::Agent(ccusage_cli::AgentId::Cline, args)) => args.shared.since,
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
        let weekly = resolved_since(Command::Agent(
            ccusage_cli::AgentId::Codex,
            agent_command(AgentReportKind::Weekly, 1),
        ));
        let cline_weekly = resolved_since(Command::Agent(
            ccusage_cli::AgentId::Cline,
            agent_command(AgentReportKind::Weekly, 1),
        ));
        let monthly = resolved_since(Command::Agent(
            ccusage_cli::AgentId::Codex,
            agent_command(AgentReportKind::Monthly, 1),
        ));

        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            weekly,
            last_periods_since(PeriodUnit::Week, 1, &today, WeekDay::Monday)
        );
        assert_eq!(
            cline_weekly,
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
    fn keeps_the_unified_weekly_start_of_week() {
        let since = resolved_since(Command::All(AgentCommandArgs {
            start_of_week: WeekDay::Wednesday,
            ..agent_command(AgentReportKind::Weekly, 1)
        }));

        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            since,
            last_periods_since(PeriodUnit::Week, 1, &today, WeekDay::Wednesday)
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
