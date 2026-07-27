use std::time::{SystemTime, UNIX_EPOCH};

use jiff::{Timestamp as JiffTimestamp, civil::Date as JiffDate, tz::TimeZone as JiffTimeZone};

const MILLIS_PER_SECOND: i64 = 1_000;
pub const MILLIS_PER_MINUTE: i64 = 60 * MILLIS_PER_SECOND;
pub const MILLIS_PER_HOUR: i64 = 60 * MILLIS_PER_MINUTE;
pub const MILLIS_PER_DAY: i64 = 24 * MILLIS_PER_HOUR;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimestampMs(i64);

#[derive(Debug, Clone, Copy)]
pub struct UtcParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    millisecond: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct IsoDate {
    year: i32,
    month: u32,
    day: u32,
}

impl TimestampMs {
    pub const UNIX_EPOCH: Self = Self(0);

    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn from_unix_seconds(seconds: i64) -> Option<Self> {
        seconds.checked_mul(MILLIS_PER_SECOND).map(Self)
    }

    pub fn as_millis(self) -> i64 {
        self.0
    }

    pub fn checked_add_millis(self, millis: i64) -> Option<Self> {
        self.0.checked_add(millis).map(Self)
    }

    pub fn checked_sub_millis(self, millis: i64) -> Option<Self> {
        self.0.checked_sub(millis).map(Self)
    }

    pub fn duration_since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }

    pub fn floor_to_hour(self) -> Self {
        Self(self.0.div_euclid(MILLIS_PER_HOUR) * MILLIS_PER_HOUR)
    }

    fn utc_parts(self) -> UtcParts {
        let seconds = self.0.div_euclid(MILLIS_PER_SECOND);
        let millisecond = self.0.rem_euclid(MILLIS_PER_SECOND) as u32;
        let days = seconds.div_euclid(86_400);
        let second_of_day = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        UtcParts {
            year,
            month,
            day,
            hour: (second_of_day / 3_600) as u32,
            minute: ((second_of_day % 3_600) / 60) as u32,
            second: (second_of_day % 60) as u32,
            millisecond,
        }
    }
}

impl IsoDate {
    fn from_ymd(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    fn days_since_epoch(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub(crate) fn weekday_from_sunday(self) -> u32 {
        (self.days_since_epoch() + 4).rem_euclid(7) as u32
    }

    pub(crate) fn checked_add_days(self, days: i64) -> Option<Self> {
        let days = self.days_since_epoch().checked_add(days)?;
        let (year, month, day) = civil_from_days(days);
        Some(Self { year, month, day })
    }
}

pub fn utc_now() -> TimestampMs {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    TimestampMs::from_millis(millis)
}

pub fn parse_ts_timestamp(value: &str) -> Option<TimestampMs> {
    let bytes = value.as_bytes();
    let (millis, timezone_start) = match bytes.len() {
        20 | 25 if bytes[19] == b'Z' || bytes[19] == b'+' || bytes[19] == b'-' => (0, 19),
        24 | 29 if bytes[19] == b'.' => (parse_digits(&bytes[20..23])?, 23),
        _ => return None,
    };
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i32;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let timezone_offset = parse_timezone_offset(&bytes[timezone_start..])?;
    let date = IsoDate::from_ymd(year, month, day)?;
    let timestamp = date
        .days_since_epoch()
        .checked_mul(MILLIS_PER_DAY)?
        .checked_add(i64::from(hour) * MILLIS_PER_HOUR)?
        .checked_add(i64::from(minute) * MILLIS_PER_MINUTE)?
        .checked_add(i64::from(second) * MILLIS_PER_SECOND)?
        .checked_add(i64::from(millis))?;
    TimestampMs::from_millis(timestamp).checked_sub_millis(timezone_offset * MILLIS_PER_MINUTE)
}

fn parse_timezone_offset(bytes: &[u8]) -> Option<i64> {
    if bytes == b"Z" {
        return Some(0);
    }
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    let hours = parse_digits(&bytes[1..3])?;
    let minutes = parse_digits(&bytes[4..6])?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let offset = i64::from(hours * 60 + minutes);
    Some(if bytes[0] == b'+' { offset } else { -offset })
}

pub(crate) fn parse_iso_date(value: &str) -> Option<IsoDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i32;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    IsoDate::from_ymd(year, month, day)
}

/// Parses a compact `YYYYMMDD` date, the normalized form of the `--since` and
/// `--until` bounds.
///
/// Returns `None` for anything that is not a full valid date, including partial
/// bounds such as `202601` that callers still compare lexicographically.
fn parse_compact_date(value: &str) -> Option<IsoDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 8 {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i32;
    let month = parse_digits(&bytes[4..6])?;
    let day = parse_digits(&bytes[6..8])?;
    IsoDate::from_ymd(year, month, day)
}

/// Reports whether a `YYYY-MM-DD` date falls inside the `--since`/`--until`
/// window.
///
/// Both bounds are inclusive and compared as compact `YYYYMMDD` text, so
/// partial bounds such as `2026` keep working. This is the authoritative window
/// check shared by every report and loader.
pub fn date_within_range(date: &str, since: Option<&str>, until: Option<&str>) -> bool {
    if since.is_none() && until.is_none() {
        return true;
    }
    let compact = date.replace('-', "");
    since.is_none_or(|since| compact.as_str() >= since)
        && until.is_none_or(|until| compact.as_str() <= until)
}

/// Resolves the `--since`/`--until` window to half-open Unix millisecond bounds
/// `[since 00:00, the day after until 00:00)` in `timezone`.
///
/// A `None` bound means that side must not be narrowed: either the option was
/// absent, or it is not a full `YYYYMMDD` date and therefore has no instant to
/// compare against. Callers pair these bounds with [`date_within_range`], which
/// stays authoritative for the exact per-entry decision.
pub fn date_range_bounds_ms(
    since: Option<&str>,
    until: Option<&str>,
    timezone: Option<&JiffTimeZone>,
) -> (Option<i64>, Option<i64>) {
    let start = since
        .and_then(parse_compact_date)
        .and_then(|date| start_of_day_ms(date, timezone));
    let end = until
        .and_then(parse_compact_date)
        .and_then(|date| date.checked_add_days(1))
        .and_then(|date| start_of_day_ms(date, timezone));
    (start, end)
}

/// First instant of `date` in `timezone`, as Unix milliseconds.
///
/// Days whose midnight does not exist because of a spring-forward transition
/// resolve to the first instant that does, so a bound never lands in the
/// neighboring day.
fn start_of_day_ms(date: IsoDate, timezone: Option<&JiffTimeZone>) -> Option<i64> {
    let civil = JiffDate::new(
        i16::try_from(date.year).ok()?,
        i8::try_from(date.month).ok()?,
        i8::try_from(date.day).ok()?,
    )
    .ok()?;
    let timezone = timezone.cloned().unwrap_or_else(JiffTimeZone::system);
    Some(civil.to_zoned(timezone).ok()?.timestamp().as_millisecond())
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    let mut value: u32 = 0;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    Some(value)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

pub fn parse_tz(timezone: Option<&str>) -> Option<JiffTimeZone> {
    timezone.and_then(|value| JiffTimeZone::get(value).ok())
}

pub fn format_date(timestamp: TimestampMs, timezone: Option<&str>) -> String {
    format_date_tz(timestamp, parse_tz(timezone).as_ref())
}

pub fn format_date_tz(timestamp: TimestampMs, timezone: Option<&JiffTimeZone>) -> String {
    let Ok(timestamp) = JiffTimestamp::from_millisecond(timestamp.as_millis()) else {
        return format_utc_date(timestamp);
    };
    let timezone = timezone.cloned().unwrap_or_else(JiffTimeZone::system);
    let zoned = timestamp.to_zoned(timezone);
    format_date_parts(
        i32::from(zoned.year()),
        u32::from(zoned.month() as u8),
        u32::from(zoned.day() as u8),
    )
}

fn format_utc_date(timestamp: TimestampMs) -> String {
    let parts = timestamp.utc_parts();
    format_date_parts(parts.year, parts.month, parts.day)
}

pub(crate) fn format_naive_date(date: IsoDate) -> String {
    format_date_parts(date.year, date.month, date.day)
}

fn format_date_parts(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

#[doc(hidden)]
pub fn format_utc_minute(timestamp: TimestampMs) -> String {
    let parts = timestamp.utc_parts();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute
    )
}

pub fn format_utc_second(timestamp: TimestampMs) -> String {
    let parts = timestamp.utc_parts();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    )
}

pub fn format_rfc3339_millis(timestamp: TimestampMs) -> String {
    let parts = timestamp.utc_parts();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.year,
        parts.month,
        parts.day,
        parts.hour,
        parts.minute,
        parts.second,
        parts.millisecond
    )
}

pub fn local_parts(timestamp: TimestampMs) -> UtcParts {
    let Ok(timestamp) = JiffTimestamp::from_millisecond(timestamp.as_millis()) else {
        return timestamp.utc_parts();
    };
    let zoned = timestamp.to_zoned(JiffTimeZone::system());
    UtcParts {
        year: i32::from(zoned.year()),
        month: u32::from(zoned.month() as u8),
        day: u32::from(zoned.day() as u8),
        hour: u32::from(zoned.hour() as u8),
        minute: u32::from(zoned.minute() as u8),
        second: u32::from(zoned.second() as u8),
        millisecond: 0,
    }
}

pub fn hour_12(hour: u32) -> u32 {
    let hour = hour % 12;
    if hour == 0 { 12 } else { hour }
}

pub fn am_pm(hour: u32) -> &'static str {
    if hour < 12 { "AM" } else { "PM" }
}

#[cfg(test)]
mod tests {
    use super::{
        MILLIS_PER_HOUR, date_range_bounds_ms, date_within_range, parse_compact_date, parse_tz,
    };

    // 2026-01-02 00:00:00 UTC
    const JAN_2_UTC: i64 = 1_767_312_000_000;

    #[test]
    fn parses_full_compact_dates_only() {
        let date = parse_compact_date("20260102").unwrap();
        assert_eq!((date.year, date.month, date.day), (2026, 1, 2));
        assert!(parse_compact_date("202601").is_none());
        assert!(parse_compact_date("2026010200").is_none());
        assert!(parse_compact_date("20260231").is_none(), "invalid day");
        assert!(parse_compact_date("2026-01-02").is_none());
    }

    #[test]
    fn parses_multi_byte_bounds_without_panicking() {
        // Eight bytes, but the character boundaries do not line up with the
        // year/month/day fields.
        assert!(parse_compact_date("abあcde").is_none());
        assert!(parse_compact_date("ああ").is_none());
        assert!(parse_compact_date("２０２６０１０２").is_none());
    }

    #[test]
    fn resolves_bounds_to_local_midnight() {
        let utc = parse_tz(Some("UTC"));
        let (start, end) = date_range_bounds_ms(Some("20260102"), Some("20260102"), utc.as_ref());
        assert_eq!(start, Some(JAN_2_UTC));
        assert_eq!(
            end,
            Some(JAN_2_UTC + 24 * MILLIS_PER_HOUR),
            "until is inclusive, so the bound is the start of the next day"
        );

        // UTC+14 reaches a given local date fourteen hours before UTC does.
        let kiritimati = parse_tz(Some("Pacific/Kiritimati"));
        let (start, _) = date_range_bounds_ms(Some("20260102"), None, kiritimati.as_ref());
        assert_eq!(start, Some(JAN_2_UTC - 14 * MILLIS_PER_HOUR));
    }

    #[test]
    fn spans_twenty_three_hours_across_a_spring_forward_day() {
        let new_york = parse_tz(Some("America/New_York"));
        let (start, end) =
            date_range_bounds_ms(Some("20260308"), Some("20260308"), new_york.as_ref());
        assert_eq!(
            end.unwrap() - start.unwrap(),
            23 * MILLIS_PER_HOUR,
            "the day the clocks move forward is an hour short"
        );
    }

    #[test]
    fn leaves_bounds_open_when_they_have_no_instant() {
        let utc = parse_tz(Some("UTC"));
        assert_eq!(
            date_range_bounds_ms(Some("202601"), Some("abあcde"), utc.as_ref()),
            (None, None)
        );
        assert_eq!(date_range_bounds_ms(None, None, utc.as_ref()), (None, None));
    }

    #[test]
    fn treats_both_window_ends_as_inclusive() {
        assert!(date_within_range("2026-01-02", Some("20260102"), None));
        assert!(date_within_range("2026-01-02", None, Some("20260102")));
        assert!(date_within_range(
            "2026-01-02",
            Some("20260101"),
            Some("20260103")
        ));
        assert!(!date_within_range("2026-01-02", Some("20260103"), None));
        assert!(!date_within_range("2026-01-02", None, Some("20260101")));
        assert!(date_within_range("2026-01-02", None, None));
    }

    #[test]
    fn compares_partial_bounds_lexicographically() {
        assert!(date_within_range(
            "2026-01-02",
            Some("2026"),
            Some("202602")
        ));
        assert!(!date_within_range("2025-12-31", Some("2026"), None));
    }
}
