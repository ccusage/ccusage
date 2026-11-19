use ccusage_cli::{Command, PeriodUnit, SharedArgs, WeekDay};
use ccusage_core::{date_at, resolve_tz, utc_now};
use ccusage_report::last_periods_since;

pub(crate) fn resolve(command: &mut Command) -> Result<(), String> {
    let Some(period) = command.period() else {
        return Ok(());
    };
    let Some(shared) = command.shared_mut() else {
        return Ok(());
    };
    resolve_shared(shared, period)
}

pub(crate) fn resolve_shared(
    shared: &mut SharedArgs,
    (unit, start_of_week): (PeriodUnit, WeekDay),
) -> Result<(), String> {
    let Some(last) = shared.last else {
        return Ok(());
    };
    let today = date_at(utc_now(), &resolve_tz(shared.timezone.as_ref()));
    let Some(since) = last_periods_since(unit, last, today, start_of_week) else {
        return Err(format!("Could not resolve --last {last} from today's date"));
    };
    shared.since = Some(since);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ccusage_cli::{
        AgentCommandArgs, AgentReportKind, SharedArgs, SortOrder, WeekDay, WeeklyArgs,
    };

    use super::*;

    fn periods(count: u32) -> std::num::NonZeroU32 {
        std::num::NonZeroU32::new(count).unwrap()
    }

    fn shared_with_last(last: u32) -> SharedArgs {
        SharedArgs {
            last: Some(periods(last)),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::with_defaults()
        }
    }

    fn agent_command(kind: AgentReportKind, last: u32) -> AgentCommandArgs {
        AgentCommandArgs {
            options: Default::default(),
            shared: shared_with_last(last),
            kind,
            session_id: None,
            sections: None,
            by_agent: false,

            include_claude_agent: false,
            start_of_week: WeekDay::Monday,
            sources: BTreeMap::new(),
        }
    }

    fn resolved_since(command: Command) -> Option<ccusage_cli::DateBound> {
        let mut command = command;
        resolve(&mut command).unwrap();
        command.shared().and_then(|shared| shared.since)
    }

    #[test]
    fn resolves_the_unified_daily_window_to_today() {
        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());

        let since = resolved_since(Command::All(agent_command(AgentReportKind::Daily, 1)));

        assert_eq!(since, Some(today.into()));
    }

    #[test]
    fn resolves_agent_report_kinds_to_their_own_period() {
        let weekly = resolved_since(Command::agent_report(
            ccusage_cli::StandardAgent::Codex,
            agent_command(AgentReportKind::Weekly, 1),
        ));
        let cline_weekly = resolved_since(Command::agent_report(
            ccusage_cli::StandardAgent::Cline,
            agent_command(AgentReportKind::Weekly, 1),
        ));
        let monthly = resolved_since(Command::agent_report(
            ccusage_cli::StandardAgent::Codex,
            agent_command(AgentReportKind::Monthly, 1),
        ));

        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());
        assert_eq!(
            weekly,
            last_periods_since(PeriodUnit::Week, periods(1), today, WeekDay::Monday)
        );
        assert_eq!(
            cline_weekly,
            last_periods_since(PeriodUnit::Week, periods(1), today, WeekDay::Monday)
        );
        assert_eq!(
            monthly,
            last_periods_since(PeriodUnit::Month, periods(1), today, WeekDay::Monday)
        );
    }

    #[test]
    fn keeps_the_claude_weekly_start_of_week() {
        let since = resolved_since(Command::Weekly(WeeklyArgs {
            shared: shared_with_last(1),
            start_of_week: WeekDay::Sunday,
        }));

        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());
        assert_eq!(
            since,
            last_periods_since(PeriodUnit::Week, periods(1), today, WeekDay::Sunday)
        );
    }

    #[test]
    fn keeps_the_unified_weekly_start_of_week() {
        let since = resolved_since(Command::All(AgentCommandArgs {
            start_of_week: WeekDay::Wednesday,
            ..agent_command(AgentReportKind::Weekly, 1)
        }));

        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());
        assert_eq!(
            since,
            last_periods_since(PeriodUnit::Week, periods(1), today, WeekDay::Wednesday)
        );
    }

    #[test]
    fn leaves_commands_without_last_untouched() {
        let mut command = Command::Monthly(SharedArgs {
            order: Some(SortOrder::Desc),
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
            "test",
        )
        .unwrap();
        resolve(&mut command).unwrap();
        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());
        assert_eq!(
            command.shared().unwrap().since,
            last_periods_since(PeriodUnit::Day, periods(2), today, WeekDay::Monday)
        );
    }
}
