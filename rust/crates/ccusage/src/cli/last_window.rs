use ccusage_cli::Command;
use ccusage_core::utc_now;
use ccusage_report::{PeriodUnit, format_date, last_periods_since};

/// Turns `--last N` into the `--since` bound every loader already understands.
///
/// The count means "the most recent N periods of the report's own unit", so it
/// can only be resolved once the command is known: `daily --last 1` is today,
/// `weekly --last 1` is the current week, `monthly --last 1` is this month.
pub(crate) fn resolve(command: &mut Command) -> Result<(), String> {
    let Some((unit, start_of_week)) = command.period() else {
        return Ok(());
    };
    let Some(shared) = command.shared_mut() else {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ccusage_cli::{
        AgentCommandArgs, AgentReportKind, CodexSpeed, SharedArgs, SortOrder, WeekDay, WeeklyArgs,
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
        let mut command = command;
        resolve(&mut command).unwrap();
        command.shared().and_then(|shared| shared.since.clone())
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
        let mut command = Command::Monthly(SharedArgs {
            order: SortOrder::Desc,
            ..SharedArgs::with_defaults()
        });
        resolve(&mut command).unwrap();
        assert!(command.shared().unwrap().since.is_none());
    }

    #[test]
    fn resolves_the_bare_command_as_a_daily_window() {
        let mut command = ccusage_cli_parser::parse_from_with_config(
            ["ccusage", "--last", "2", "--timezone", "UTC"]
                .into_iter()
                .map(std::ffi::OsString::from),
            &ccusage_cli::NoConfig,
            5.0,
            "test",
            None,
        )
        .unwrap();
        resolve(&mut command).unwrap();
        let today = format_date(utc_now(), Some("UTC"));
        assert_eq!(
            command.shared().unwrap().since,
            last_periods_since(PeriodUnit::Day, 2, &today, WeekDay::Monday)
        );
    }
}
