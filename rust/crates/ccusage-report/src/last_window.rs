use std::num::NonZeroU32;

use jiff::Span;

use ccusage_core::cli::{DateBound, IsoDate, PeriodUnit, WeekDay};

pub fn last_periods_since(
    unit: PeriodUnit,
    count: NonZeroU32,
    today: IsoDate,
    start_of_week: WeekDay,
) -> Option<DateBound> {
    let earlier = i64::from(count.get() - 1);
    let start = match unit {
        PeriodUnit::Day => today.civil()?.checked_sub(Span::new().days(earlier)).ok()?,
        PeriodUnit::Week => today
            .week_start(start_of_week)
            .civil()?
            .checked_sub(Span::new().weeks(earlier))
            .ok()?,
        PeriodUnit::Month => today
            .civil()?
            .first_of_month()
            .checked_sub(Span::new().months(earlier))
            .ok()?,
    };
    Some(IsoDate::from(start).into())
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
