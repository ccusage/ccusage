use std::time::{SystemTime, UNIX_EPOCH};

use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};

pub const MILLIS_PER_SECOND: i64 = 1_000;
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
    pub millisecond: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct IsoDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
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

    pub fn utc_parts(self) -> UtcParts {
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
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    pub fn days_since_epoch(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn weekday_from_sunday(self) -> u32 {
        (self.days_since_epoch() + 4).rem_euclid(7) as u32
    }

    pub fn checked_add_days(self, days: i64) -> Option<Self> {
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

pub fn parse_timezone_offset(bytes: &[u8]) -> Option<i64> {
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

pub fn parse_iso_date(value: &str) -> Option<IsoDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i32;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    IsoDate::from_ymd(year, month, day)
}

pub fn parse_digits(bytes: &[u8]) -> Option<u32> {
    let mut value: u32 = 0;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    Some(value)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
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

pub fn format_utc_date(timestamp: TimestampMs) -> String {
    let parts = timestamp.utc_parts();
    format_date_parts(parts.year, parts.month, parts.day)
}

pub fn format_naive_date(date: IsoDate) -> String {
    format_date_parts(date.year, date.month, date.day)
}

pub fn format_date_parts(year: i32, month: u32, day: u32) -> String {
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
