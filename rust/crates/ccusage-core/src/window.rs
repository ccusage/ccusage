use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    TimestampMs,
    cli::{DateBound, IsoDate, SharedArgs},
    date_at, date_range_bounds_ms, resolve_tz,
};

#[derive(Clone, Debug)]
pub struct ReportWindow {
    since: Option<DateBound>,
    until: Option<DateBound>,
    timezone: JiffTimeZone,
}

impl ReportWindow {
    pub fn from_shared(shared: &SharedArgs) -> Self {
        Self {
            since: shared.since,
            until: shared.until,
            timezone: resolve_tz(shared.timezone.as_ref()),
        }
    }

    pub fn unbounded() -> Self {
        Self {
            since: None,
            until: None,
            timezone: resolve_tz(None),
        }
    }

    pub fn is_unbounded(&self) -> bool {
        self.since.is_none() && self.until.is_none()
    }

    pub fn timezone(&self) -> &JiffTimeZone {
        &self.timezone
    }

    pub fn next_day_until(&self) -> Option<DateBound> {
        let day = self.until?.day();
        let next = day.checked_add_days(1)?;
        next.civil().map(|_| next.into())
    }

    pub fn contains_day(&self, day: IsoDate) -> bool {
        self.since.is_none_or(|since| day >= since.day())
            && self.until.is_none_or(|until| day <= until.day())
    }

    pub fn contains_entry_date(&self, day: IsoDate) -> bool {
        // NOTE(2026-11-19, #2247): Bounded entry reports previously rejected years outside 0000–9999 when parsing the formatted date. Preserve that rule even for a one-sided window.
        self.is_unbounded() || ((0..=9999).contains(&day.parts().0) && self.contains_day(day))
    }

    pub fn contains_timestamp(&self, timestamp: TimestampMs) -> bool {
        self.is_unbounded() || self.contains_day(date_at(timestamp, &self.timezone))
    }

    pub fn bounds_ms(&self) -> (Option<i64>, Option<i64>) {
        date_range_bounds_ms(self.since, self.until, &self.timezone)
    }
}

#[cfg(test)]
mod tests {
    use super::ReportWindow;
    use crate::{
        MILLIS_PER_HOUR, TimestampMs,
        cli::{IsoDate, SharedArgs},
    };

    fn window(since: Option<&str>, until: Option<&str>, timezone: Option<&str>) -> ReportWindow {
        ReportWindow::from_shared(&SharedArgs {
            since: since.map(|date| date.parse().unwrap()),
            until: until.map(|date| date.parse().unwrap()),
            timezone: timezone.map(|value| value.parse().unwrap()),
            ..SharedArgs::default()
        })
    }

    #[test]
    fn contains_both_date_bounds_inclusively() {
        let window = window(Some("20260101"), Some("20260131"), None);
        assert!(!window.contains_entry_date(IsoDate::from_ymd(2025, 12, 31).unwrap()));
        assert!(window.contains_entry_date(IsoDate::from_ymd(2026, 1, 1).unwrap()));
        assert!(window.contains_entry_date(IsoDate::from_ymd(2026, 1, 31).unwrap()));
        assert!(!window.contains_entry_date(IsoDate::from_ymd(2026, 2, 1).unwrap()));
    }

    #[test]
    fn only_unbounded_entry_reports_accept_extended_years() {
        let before = IsoDate::from_ymd(-1, 12, 31).unwrap();
        let after = IsoDate::from_ymd(10000, 1, 1).unwrap();
        let unbounded = ReportWindow::unbounded();
        assert!(unbounded.contains_entry_date(before));
        assert!(unbounded.contains_entry_date(after));
        assert!(!window(None, Some("20260131"), None).contains_entry_date(before));
        assert!(!window(Some("20260101"), None, None).contains_entry_date(after));
    }

    #[test]
    fn is_unbounded_only_without_either_bound() {
        assert!(window(None, None, Some("UTC")).is_unbounded());
        assert!(ReportWindow::unbounded().is_unbounded());
        assert!(!window(Some("20260101"), None, None).is_unbounded());
        assert!(!window(None, Some("20260101"), None).is_unbounded());
    }

    #[test]
    fn contains_timestamp_takes_the_day_in_the_report_timezone() {
        let timestamp = TimestampMs::from_millis(1_767_312_000_000 - 4 * MILLIS_PER_HOUR);

        assert!(!window(Some("20260102"), None, Some("UTC")).contains_timestamp(timestamp));
        assert!(window(Some("20260102"), None, Some("Asia/Tokyo")).contains_timestamp(timestamp));
        assert!(ReportWindow::unbounded().contains_timestamp(timestamp));
    }
}
