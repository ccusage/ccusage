use ccusage_core::{TimestampMs, UtcParts, format_date_tz, parse_tz};
use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};

/// `--timezone` and a config `timezone` are checked with this before use, so an
/// unknown name is rejected up front instead of silently falling back to the
/// local zone, which can group usage under the wrong date. `local` is the
/// documented way to ask for the system timezone and keeps that fallback.
pub fn is_valid_timezone(value: &str) -> bool {
    matches!(value, "local" | "UTC") || JiffTimeZone::get(value).is_ok()
}

pub fn format_date(timestamp: TimestampMs, timezone: Option<&str>) -> String {
    format_date_tz(timestamp, parse_tz(timezone).as_ref())
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
    use super::is_valid_timezone;

    #[test]
    fn handles_documented_timezone_aliases_without_a_zoneinfo_lookup() {
        assert!(is_valid_timezone("UTC"));
        assert!(is_valid_timezone("local"));
        assert!(!is_valid_timezone("Not/AZone"));
    }
}
