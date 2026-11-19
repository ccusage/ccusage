use ccusage_core::{Result, TimestampMs, cli_error};
use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};

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
