use jiff::tz::TimeZone as JiffTimeZone;

use crate::{TimestampMs, cli::SharedArgs, date_range_bounds_ms, format_date_tz, resolve_tz};

/// The `--since`/`--until` window of one report in its report timezone.
///
/// Build it once per source from the options that source loads with, after the
/// binary has rewritten `--last` into `since`, and ask it instead of comparing
/// bounds by hand: every place that decides whether a day is in the report then
/// uses the same resolved bounds.
#[derive(Clone, Debug)]
pub struct ReportWindow {
    since: Option<Box<str>>,
    until: Option<Box<str>>,
    timezone: JiffTimeZone,
}

impl ReportWindow {
    pub fn from_shared(shared: &SharedArgs) -> Self {
        Self {
            since: shared.since.as_deref().map(Box::from),
            until: shared.until.as_deref().map(Box::from),
            timezone: resolve_tz(shared.timezone.as_deref()),
        }
    }

    /// A window that keeps every day.
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

    /// The timezone resolved once for this report.
    pub fn timezone(&self) -> &JiffTimeZone {
        &self.timezone
    }

    /// Reports whether a `YYYY-MM-DD` day is inside the window.
    ///
    /// The bounds stay compact text and are compared byte by byte against the
    /// day without its dashes, so a partial bound keeps its meaning: `--since
    /// 2026` includes all of 2026 and `--until 2026` excludes it.
    pub fn contains_date(&self, date: &str) -> bool {
        let compact = || date.bytes().filter(|byte| *byte != b'-');
        self.since
            .as_deref()
            .is_none_or(|since| compact().ge(since.bytes()))
            && self
                .until
                .as_deref()
                .is_none_or(|until| compact().le(until.bytes()))
    }

    /// Reports whether the day `timestamp` falls on in the report timezone is
    /// inside the window.
    pub fn contains_timestamp(&self, timestamp: TimestampMs) -> bool {
        self.is_unbounded() || self.contains_date(&format_date_tz(timestamp, &self.timezone))
    }

    /// The window as half-open Unix millisecond bounds, for a loader that can
    /// skip input early. A `None` side must not be narrowed, and
    /// [`contains_date`](Self::contains_date) still makes the per-day decision;
    /// see [`date_range_bounds_ms`].
    pub fn bounds_ms(&self) -> (Option<i64>, Option<i64>) {
        date_range_bounds_ms(self.since.as_deref(), self.until.as_deref(), &self.timezone)
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
            timezone: timezone.map(str::to_string),
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
        // 2026-01-01T20:00:00Z is already 2026-01-02 in Asia/Tokyo.
        let timestamp = TimestampMs::from_millis(1_767_312_000_000 - 4 * MILLIS_PER_HOUR);

        assert!(!window(Some("20260102"), None, Some("UTC")).contains_timestamp(timestamp));
        assert!(window(Some("20260102"), None, Some("Asia/Tokyo")).contains_timestamp(timestamp));
        assert!(ReportWindow::unbounded().contains_timestamp(timestamp));
    }

    #[test]
    fn bounds_ms_match_the_shared_helper() {
        for timezone in [Some("UTC"), Some("Asia/Tokyo"), Some("America/New_York")] {
            let parsed = resolve_tz(timezone);
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
