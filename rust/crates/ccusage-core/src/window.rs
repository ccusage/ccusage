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
            timezone: JiffTimeZone::system(),
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

    pub fn contains_date(&self, date: &str) -> bool {
        self.is_unbounded() || IsoDate::parse(date).is_some_and(|day| self.contains_day(day))
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
    use crate::{MILLIS_PER_HOUR, TimestampMs, cli::SharedArgs};

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
        assert!(!window.contains_date("2025-12-31"));
        assert!(window.contains_date("2026-01-01"));
        assert!(window.contains_date("2026-01-31"));
        assert!(!window.contains_date("2026-02-01"));
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
