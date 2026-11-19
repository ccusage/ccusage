use std::num::NonZeroU32;

use jiff::{Span, civil::Date as JiffDate};

use ccusage_core::{
    cli::{PeriodUnit, WeekDay},
    week_start,
};

/// Compact `YYYYMMDD` start of the window covering the most recent `count`
/// periods, the last of which is the one `today` falls in.
///
/// `today` is a `YYYY-MM-DD` date already resolved in the report timezone, so
/// the caller decides what "now" means. Weeks start on `start_of_week` to match
/// the bucketing the report itself uses; months always start on the first.
pub fn last_periods_since(
    unit: PeriodUnit,
    count: NonZeroU32,
    today: &str,
    start_of_week: WeekDay,
) -> Option<String> {
    let earlier = i64::from(count.get() - 1);
    let start = match unit {
        PeriodUnit::Day => parse_date(today)?
            .checked_sub(Span::new().days(earlier))
            .ok()?,
        PeriodUnit::Week => parse_date(&week_start(today, start_of_week)?)?
            .checked_sub(Span::new().weeks(earlier))
            .ok()?,
        PeriodUnit::Month => parse_date(today)?
            .first_of_month()
            .checked_sub(Span::new().months(earlier))
            .ok()?,
    };
    Some(format!(
        "{:04}{:02}{:02}",
        start.year(),
        start.month(),
        start.day()
    ))
}

fn parse_date(date: &str) -> Option<JiffDate> {
    date.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn periods(count: u32) -> NonZeroU32 {
        NonZeroU32::new(count).unwrap()
    }

    #[test]
    fn covers_only_today_for_a_single_day() {
        assert_eq!(
            last_periods_since(PeriodUnit::Day, periods(1), "2026-07-27", WeekDay::Monday)
                .as_deref(),
            Some("20260727")
        );
    }

    #[test]
    fn counts_days_inclusively_across_a_month_boundary() {
        assert_eq!(
            last_periods_since(PeriodUnit::Day, periods(3), "2026-08-01", WeekDay::Monday)
                .as_deref(),
            Some("20260730")
        );
    }

    #[test]
    fn starts_a_single_week_at_the_configured_week_start() {
        assert_eq!(
            last_periods_since(PeriodUnit::Week, periods(1), "2026-07-29", WeekDay::Monday)
                .as_deref(),
            Some("20260727")
        );
        assert_eq!(
            last_periods_since(PeriodUnit::Week, periods(1), "2026-07-29", WeekDay::Sunday)
                .as_deref(),
            Some("20260726")
        );
    }

    #[test]
    fn counts_whole_weeks_back_from_the_current_one() {
        assert_eq!(
            last_periods_since(PeriodUnit::Week, periods(2), "2026-07-29", WeekDay::Monday)
                .as_deref(),
            Some("20260720")
        );
    }

    #[test]
    fn starts_months_on_the_first_and_wraps_the_year() {
        assert_eq!(
            last_periods_since(PeriodUnit::Month, periods(1), "2026-07-27", WeekDay::Monday)
                .as_deref(),
            Some("20260701")
        );
        assert_eq!(
            last_periods_since(PeriodUnit::Month, periods(3), "2026-01-15", WeekDay::Monday)
                .as_deref(),
            Some("20251101")
        );
    }

    #[test]
    fn rejects_dates_that_are_not_iso_days() {
        assert!(
            last_periods_since(PeriodUnit::Day, periods(1), "not-a-date", WeekDay::Monday)
                .is_none()
        );
    }
}
