use ccusage_core::{
    Result, TimestampMs, cli::ReportTimezone, cli_error, format_date_tz, resolve_tz,
};
use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};

pub fn format_date(timestamp: TimestampMs, timezone: Option<&ReportTimezone>) -> String {
    format_date_tz(timestamp, &resolve_tz(timezone))
}

pub struct DateTimeParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

pub fn zoned_parts(timestamp: TimestampMs, zone: &JiffTimeZone) -> Result<DateTimeParts> {
    let timestamp = JiffTimestamp::from_millisecond(timestamp.as_millis())
        .map_err(|error| cli_error(format!("Invalid block timestamp: {error}")))?;
    let parts = zone.to_datetime(timestamp);
    Ok(DateTimeParts {
        year: i32::from(parts.year()),
        month: u32::from(parts.month().unsigned_abs()),
        day: u32::from(parts.day().unsigned_abs()),
        hour: u32::from(parts.hour().unsigned_abs()),
        minute: u32::from(parts.minute().unsigned_abs()),
        second: u32::from(parts.second().unsigned_abs()),
    })
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
    use ccusage_core::{format_rfc3339_millis, parse_ts_timestamp};

    use super::format_date;

    #[test]
    fn formats_dates_with_timezone() {
        let timestamp = parse_ts_timestamp("2024-08-04T23:30:00.000Z").unwrap();

        assert_eq!(
            format_date(timestamp, Some(&ccusage_core::cli::ReportTimezone::UTC)),
            "2024-08-04"
        );
        assert_eq!(
            format_date(timestamp, Some(&"Asia/Tokyo".parse().unwrap())),
            "2024-08-05"
        );
        assert_eq!(format_rfc3339_millis(timestamp), "2024-08-04T23:30:00.000Z");
    }
}
