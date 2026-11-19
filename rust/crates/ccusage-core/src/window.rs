use jiff::tz::TimeZone as JiffTimeZone;
use jiff::{ToSpan, civil::Date};

use crate::{TimestampMs, cli::SharedArgs, date_range_bounds_ms, format_date_tz, resolve_tz};

#[derive(Clone, Debug)]
pub struct ReportWindow {
    since: Option<Box<str>>,
    until: Option<ReportBound>,
    timezone: JiffTimeZone,
}

#[derive(Clone, Debug)]
struct ReportBound {
    text: Box<str>,
    day: Option<Date>,
}

impl ReportBound {
    fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            day: Date::strptime("%Y%m%d", text).ok(),
        }
    }
}

impl ReportWindow {
    pub fn from_shared(shared: &SharedArgs) -> Self {
        Self {
            since: shared.since.as_deref().map(Box::from),
            until: shared.until.as_deref().map(ReportBound::new),
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

    pub fn next_day_until(&self) -> crate::Result<Option<String>> {
        let Some(bound) = &self.until else {
            return Ok(None);
        };
        let day = bound
            .day
            .ok_or_else(|| crate::cli_error("Claim windows require a complete calendar date"))?;
        if day == Date::MAX {
            return Ok(None);
        }
        let next = day
            .checked_add(1.days())
            .map_err(|error| crate::cli_error(error.to_string()))?;
        Ok(Some(next.strftime("%Y%m%d").to_string()))
    }

    /// Tests a `YYYY-MM-DD` day against the report window.
    /// Partial bounds compare lexically: `--since 2026` includes 2026 and `--until 2026` excludes it.
    pub fn contains_date(&self, date: &str) -> bool {
        let compact = || date.bytes().filter(|byte| *byte != b'-');
        self.since
            .as_deref()
            .is_none_or(|since| compact().ge(since.bytes()))
            && self
                .until
                .as_ref()
                .is_none_or(|until| compact().le(until.text.bytes()))
    }

    pub fn contains_timestamp(&self, timestamp: TimestampMs) -> bool {
        self.is_unbounded() || self.contains_date(&format_date_tz(timestamp, &self.timezone))
    }

    /// Half-open Unix millisecond bounds for an early loader filter.
    /// A `None` side stays open until [`Self::contains_date`] checks the record's day.
    pub fn bounds_ms(&self) -> (Option<i64>, Option<i64>) {
        date_range_bounds_ms(
            self.since.as_deref(),
            self.until.as_ref().map(|bound| bound.text.as_ref()),
            &self.timezone,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ReportWindow;
    use crate::{MILLIS_PER_HOUR, TimestampMs, cli::SharedArgs, date_range_bounds_ms, resolve_tz};

    const BOUNDS: [Option<&str>; 12] = [
        None,
        Some(""),
        Some("2"),
        Some("2025"),
        Some("2026"),
        Some("202601"),
        Some("202602"),
        Some("20260101"),
        Some("20260102"),
        Some("20260131"),
        Some("20270101"),
        Some("abあcde"),
    ];

    const DATES: [&str; 13] = [
        "",
        "2025-12-31",
        "2026-01-01",
        "2026-01-02",
        "2026-01-31",
        "2026-02-01",
        "2026-12-31",
        "2027-01-01",
        "20260102",
        "2026-01",
        "2026",
        "-2026--01-02-",
        "あ",
    ];

    fn window(since: Option<&str>, until: Option<&str>, timezone: Option<&str>) -> ReportWindow {
        ReportWindow::from_shared(&SharedArgs {
            since: since.map(str::to_string),
            until: until.map(str::to_string),
            timezone: timezone.map(|value| value.parse().unwrap()),
            ..SharedArgs::default()
        })
    }

    #[test]
    fn contains_date_preserves_compact_lexicographic_bounds() {
        for since in BOUNDS {
            for until in BOUNDS {
                let window = window(since, until, None);
                for date in DATES {
                    assert_eq!(
                        window.contains_date(date),
                        {
                            let compact = date.replace('-', "");
                            since.is_none_or(|bound| compact.as_str() >= bound)
                                && until.is_none_or(|bound| compact.as_str() <= bound)
                        },
                        "date {date:?} since {since:?} until {until:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn partial_bounds_keep_their_lexicographic_meaning() {
        assert!(window(Some("2026"), None, None).contains_date("2026-01-01"));
        assert!(!window(None, Some("2026"), None).contains_date("2026-01-01"));
        assert!(window(None, Some("2026"), None).contains_date("2025-12-31"));
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

    #[test]
    fn bounds_ms_match_the_shared_helper() {
        for timezone in [Some("UTC"), Some("Asia/Tokyo"), Some("America/New_York")] {
            let parsed = resolve_tz(timezone.map(|value| value.parse().unwrap()).as_ref());
            for since in BOUNDS {
                for until in BOUNDS {
                    assert_eq!(
                        window(since, until, timezone).bounds_ms(),
                        date_range_bounds_ms(since, until, &parsed),
                        "since {since:?} until {until:?} timezone {timezone:?}"
                    );
                }
            }
        }
    }
}
