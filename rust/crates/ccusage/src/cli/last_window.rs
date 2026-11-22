use std::num::NonZeroU32;

use ccusage_cli::{Command, DateBound, IsoDate, PeriodUnit, SharedArgs, WeekDay};
use ccusage_core::{date_at, resolve_tz};
use jiff::Span;

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
    let today = date_at(
        shared.clock.now().into(),
        &resolve_tz(shared.timezone.as_ref()),
    );
    let (Some(since), Some(until)) = (
        last_periods_since(unit, last, today, start_of_week),
        last_periods_until(unit, today, start_of_week),
    ) else {
        return Err(format!("Could not resolve --last {last} from today's date"));
    };
    shared.since = Some(since);
    shared.until = Some(until);
    Ok(())
}

pub(super) fn last_periods_since(
    unit: PeriodUnit,
    count: NonZeroU32,
    today: IsoDate,
    start_of_week: WeekDay,
) -> Option<DateBound> {
    let earlier = i64::from(count.get() - 1);
    // NOTE(2026-11-20, #2369): Jiff's span setters panic on counts a u32 can express. Use the fallible setters so an oversized --last stays a window error.
    let (base, span) = match unit {
        PeriodUnit::Day => (today.civil()?, Span::new().try_days(earlier).ok()?),
        PeriodUnit::Week => (
            today.week_start(start_of_week).civil()?,
            Span::new().try_weeks(earlier).ok()?,
        ),
        PeriodUnit::Month => (
            today.civil()?.first_of_month(),
            Span::new().try_months(earlier).ok()?,
        ),
    };
    Some(IsoDate::from(base.checked_sub(span).ok()?).into())
}

pub(super) fn last_periods_until(
    unit: PeriodUnit,
    today: IsoDate,
    start_of_week: WeekDay,
) -> Option<DateBound> {
    let last = match unit {
        PeriodUnit::Day => today,
        PeriodUnit::Week => today.week_start(start_of_week).checked_add_days(6)?,
        PeriodUnit::Month => {
            let first = today.civil()?.first_of_month();
            IsoDate::from(first.with().day(first.days_in_month()).build().ok()?)
        }
    };
    Some(last.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ccusage_cli::{
        AgentCommandArgs, AgentReportKind, ClaudeReport, SharedArgs, SortOrder, WeekDay, WeeklyArgs,
    };

    use super::*;
    use ccusage_core::utc_now;

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
        resolved_bounds(command).0
    }

    fn resolved_bounds(
        command: Command,
    ) -> (
        Option<ccusage_cli::DateBound>,
        Option<ccusage_cli::DateBound>,
    ) {
        let mut command = command;
        resolve(&mut command).unwrap();
        let shared = command.shared().unwrap();
        (shared.since, shared.until)
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
        let since = resolved_since(Command::claude_report(ClaudeReport::Weekly(WeeklyArgs {
            shared: shared_with_last(1),
            start_of_week: WeekDay::Sunday,
        })));

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
    fn caps_the_unified_window_at_the_end_of_the_current_period() {
        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());

        assert_eq!(
            resolved_bounds(Command::All(agent_command(AgentReportKind::Daily, 1))),
            (Some(today.into()), Some(today.into()))
        );
        for (kind, unit) in [
            (AgentReportKind::Weekly, PeriodUnit::Week),
            (AgentReportKind::Monthly, PeriodUnit::Month),
        ] {
            assert_eq!(
                resolved_bounds(Command::All(agent_command(kind, 1))).1,
                last_periods_until(unit, today, WeekDay::Monday)
            );
        }
    }

    #[test]
    fn ends_the_window_with_the_last_day_of_its_period() {
        let day = |date: &str, unit, start_of_week| {
            last_periods_until(unit, IsoDate::parse(date).unwrap(), start_of_week)
                .map(|day| day.to_string())
        };

        assert_eq!(
            day("2026-07-29", PeriodUnit::Day, WeekDay::Monday).as_deref(),
            Some("20260729")
        );
        assert_eq!(
            day("2026-07-29", PeriodUnit::Week, WeekDay::Monday).as_deref(),
            Some("20260802")
        );
        assert_eq!(
            day("2026-07-29", PeriodUnit::Week, WeekDay::Sunday).as_deref(),
            Some("20260801")
        );
        assert_eq!(
            day("2026-02-10", PeriodUnit::Month, WeekDay::Monday).as_deref(),
            Some("20260228")
        );
        assert_eq!(
            day("2026-12-31", PeriodUnit::Month, WeekDay::Monday).as_deref(),
            Some("20261231")
        );
        assert_eq!(
            day("9999-12-15", PeriodUnit::Month, WeekDay::Monday).as_deref(),
            Some("99991231")
        );
    }

    #[test]
    fn leaves_commands_without_last_untouched() {
        let mut command = Command::claude_report(ClaudeReport::Monthly(SharedArgs {
            order: Some(SortOrder::Desc),
            ..SharedArgs::with_defaults()
        }));
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
        )
        .unwrap();
        resolve(&mut command).unwrap();
        let today = date_at(utc_now(), ccusage_core::cli::ReportTimezone::UTC.zone());
        assert_eq!(
            command.shared().unwrap().since,
            last_periods_since(PeriodUnit::Day, periods(2), today, WeekDay::Monday)
        );
    }

    #[test]
    fn covers_only_today_for_a_single_day() {
        assert_eq!(
            last_periods_since(
                PeriodUnit::Day,
                periods(1),
                IsoDate::parse("2026-07-27").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260727")
        );
    }

    #[test]
    fn counts_days_inclusively_across_a_month_boundary() {
        assert_eq!(
            last_periods_since(
                PeriodUnit::Day,
                periods(3),
                IsoDate::parse("2026-08-01").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260730")
        );
    }

    #[test]
    fn starts_a_single_week_at_the_configured_week_start() {
        assert_eq!(
            last_periods_since(
                PeriodUnit::Week,
                periods(1),
                IsoDate::parse("2026-07-29").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260727")
        );
        assert_eq!(
            last_periods_since(
                PeriodUnit::Week,
                periods(1),
                IsoDate::parse("2026-07-29").unwrap(),
                WeekDay::Sunday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260726")
        );
    }

    #[test]
    fn counts_whole_weeks_back_from_the_current_one() {
        assert_eq!(
            last_periods_since(
                PeriodUnit::Week,
                periods(2),
                IsoDate::parse("2026-07-29").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260720")
        );
    }

    #[test]
    fn rejects_counts_beyond_the_span_limits() {
        let today = IsoDate::parse("2026-07-27").unwrap();

        for unit in [PeriodUnit::Day, PeriodUnit::Week, PeriodUnit::Month] {
            assert!(
                last_periods_since(unit, periods(u32::MAX), today, WeekDay::Monday).is_none(),
                "{unit:?} should not resolve a window beyond the representable range"
            );
        }
    }

    #[test]
    fn starts_months_on_the_first_and_wraps_the_year() {
        assert_eq!(
            last_periods_since(
                PeriodUnit::Month,
                periods(1),
                IsoDate::parse("2026-07-27").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20260701")
        );
        assert_eq!(
            last_periods_since(
                PeriodUnit::Month,
                periods(3),
                IsoDate::parse("2026-01-15").unwrap(),
                WeekDay::Monday
            )
            .map(|day| day.to_string())
            .as_deref(),
            Some("20251101")
        );
    }
}
