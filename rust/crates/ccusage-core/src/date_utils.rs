use ccusage_cli::{DateBound, IsoDate, ReportTimezone, parse_digits};

use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};

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
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
}

impl TimestampMs {
    pub const UNIX_EPOCH: Self = Self(0);

    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn from_instant_millis(millis: i64) -> Option<Self> {
        JiffTimestamp::from_millisecond(millis)
            .ok()
            .map(|_| Self(millis))
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
        let (year, month, day) = IsoDate::from_epoch_days(days).parts();
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

impl From<JiffTimestamp> for TimestampMs {
    fn from(instant: JiffTimestamp) -> Self {
        Self::from_millis(instant.as_millisecond())
    }
}

pub fn utc_now() -> TimestampMs {
    JiffTimestamp::now().into()
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
    let year = i32::try_from(parse_digits(&bytes[0..4])?).ok()?;
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

pub fn date_range_bounds_ms(
    since: Option<DateBound>,
    until: Option<DateBound>,
    timezone: &JiffTimeZone,
) -> (Option<i64>, Option<i64>) {
    let start = since.and_then(|date| start_of_day_ms(date.day(), timezone));
    let end = until
        .and_then(|date| date.day().checked_add_days(1))
        .and_then(|date| start_of_day_ms(date, timezone));
    (start, end)
}

fn start_of_day_ms(date: IsoDate, timezone: &JiffTimeZone) -> Option<i64> {
    Some(
        date.civil()?
            .to_zoned(timezone.clone())
            .ok()?
            .timestamp()
            .as_millisecond(),
    )
}

pub fn resolve_tz(timezone: Option<&ReportTimezone>) -> JiffTimeZone {
    timezone.map_or_else(ReportTimezone::system_zone, |timezone| {
        timezone.zone().clone()
    })
}

pub fn date_at(timestamp: TimestampMs, timezone: &JiffTimeZone) -> IsoDate {
    match JiffTimestamp::from_millisecond(timestamp.as_millis()) {
        Ok(timestamp) => timezone.to_datetime(timestamp).date().into(),
        Err(_) => IsoDate::from_epoch_days(timestamp.as_millis().div_euclid(MILLIS_PER_DAY)),
    }
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

#[cfg(test)]
mod tests {
    use super::{MILLIS_PER_HOUR, TimestampMs, date_range_bounds_ms, resolve_tz};

    const JAN_2_UTC: i64 = 1_767_312_000_000;

    #[test]
    fn accepts_only_millis_that_name_a_representable_instant() {
        assert_eq!(
            TimestampMs::from_instant_millis(JAN_2_UTC),
            Some(TimestampMs::from_millis(JAN_2_UTC))
        );
        for millis in [9_000_000_000_000_000_000, i64::MAX, i64::MIN] {
            assert_eq!(TimestampMs::from_instant_millis(millis), None, "{millis}");
        }
    }

    #[test]
    fn formats_local_dates_across_dst_boundaries() {
        let timezone = resolve_tz(Some(&"Europe/London".parse().unwrap()));
        for (timestamp, expected) in [
            ("2026-03-28T23:30:00Z", "2026-03-28"),
            ("2026-03-29T23:30:00Z", "2026-03-30"),
            ("2026-10-24T23:30:00Z", "2026-10-25"),
            ("2026-10-25T23:30:00Z", "2026-10-25"),
        ] {
            let timestamp = super::parse_ts_timestamp(timestamp).unwrap();
            assert_eq!(super::date_at(timestamp, &timezone).to_string(), expected);
        }
    }

    #[test]
    fn handles_documented_timezone_aliases_without_a_zoneinfo_lookup() {
        assert_eq!(
            resolve_tz(Some(&ccusage_cli::ReportTimezone::UTC)).iana_name(),
            Some("UTC")
        );
        let system = ccusage_cli::ReportTimezone::system_zone();
        for timezone in [Some(&"local".parse().unwrap()), None] {
            assert_eq!(
                resolve_tz(timezone).iana_name(),
                system.iana_name(),
                "{timezone:?}"
            );
        }
    }

    #[test]
    fn resolves_bounds_to_local_midnight() {
        let utc = resolve_tz(Some(&ccusage_cli::ReportTimezone::UTC));
        let (start, end) = date_range_bounds_ms(
            Some("20260102".parse().unwrap()),
            Some("20260102".parse().unwrap()),
            &utc,
        );
        assert_eq!(start, Some(JAN_2_UTC));
        assert_eq!(
            end,
            Some(JAN_2_UTC + 24 * MILLIS_PER_HOUR),
            "until is inclusive, so the bound is the start of the next day"
        );

        let kiritimati = resolve_tz(Some(&"Pacific/Kiritimati".parse().unwrap()));
        let (start, _) = date_range_bounds_ms(Some("20260102".parse().unwrap()), None, &kiritimati);
        assert_eq!(start, Some(JAN_2_UTC - 14 * MILLIS_PER_HOUR));
    }

    #[test]
    fn spans_twenty_three_hours_across_a_spring_forward_day() {
        let new_york = resolve_tz(Some(&"America/New_York".parse().unwrap()));
        let (start, end) = date_range_bounds_ms(
            Some("20260308".parse().unwrap()),
            Some("20260308".parse().unwrap()),
            &new_york,
        );
        assert_eq!(
            end.unwrap() - start.unwrap(),
            23 * MILLIS_PER_HOUR,
            "the day the clocks move forward is an hour short"
        );
    }

    #[test]
    fn leaves_bounds_open_when_they_have_no_instant() {
        let utc = resolve_tz(Some(&ccusage_cli::ReportTimezone::UTC));
        assert_eq!(date_range_bounds_ms(None, None, &utc), (None, None));
    }
}
