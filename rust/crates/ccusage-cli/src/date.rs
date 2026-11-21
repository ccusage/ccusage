use std::{cmp::Ordering, fmt};

use jiff::civil::Date as JiffDate;

use crate::WeekDay;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoDate {
    year: i32,
    month: u32,
    day: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateBound(IsoDate);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayLabel(IsoDate);

impl DayLabel {
    pub fn day(self) -> IsoDate {
        self.0
    }
}

impl From<IsoDate> for DayLabel {
    fn from(day: IsoDate) -> Self {
        Self(day)
    }
}

impl Ord for DayLabel {
    fn cmp(&self, other: &Self) -> Ordering {
        // NOTE(2026-11-20, #2247): Years outside 0000-9999 print with a sign or a fifth digit, so their labels do not sort by date. Report rows keep label order.
        let four_digit = |day: IsoDate| (0..=9999).contains(&day.year);
        if four_digit(self.0) && four_digit(other.0) {
            self.0.cmp(&other.0)
        } else {
            self.0.to_string().cmp(&other.0.to_string())
        }
    }
}

impl PartialOrd for DayLabel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl DateBound {
    pub fn parse(value: &str) -> Option<Self> {
        parse_date(value, false).map(Self)
    }

    pub fn day(self) -> IsoDate {
        self.0
    }
}

impl From<IsoDate> for DateBound {
    fn from(day: IsoDate) -> Self {
        Self(day)
    }
}

impl From<JiffDate> for IsoDate {
    fn from(day: JiffDate) -> Self {
        Self {
            year: i32::from(day.year()),
            month: u32::from(day.month().unsigned_abs()),
            day: u32::from(day.day().unsigned_abs()),
        }
    }
}

impl std::str::FromStr for DateBound {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(crate::DATE_BOUND_FORMATS)
    }
}

impl fmt::Display for DateBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let IsoDate { year, month, day } = self.0;
        write!(formatter, "{year:04}{month:02}{day:02}")
    }
}

fn parse_date(value: &str, dashed_only: bool) -> Option<IsoDate> {
    let (year, month, day) = match value.as_bytes() {
        [year @ .., b'-', month_a, month_b, b'-', day_a, day_b] if year.len() == 4 => (
            parse_digits(year)?,
            parse_digits(&[*month_a, *month_b])?,
            parse_digits(&[*day_a, *day_b])?,
        ),
        bytes if !dashed_only && bytes.len() == 8 => (
            parse_digits(&bytes[..4])?,
            parse_digits(&bytes[4..6])?,
            parse_digits(&bytes[6..])?,
        ),
        _ => return None,
    };
    IsoDate::from_ymd(i32::try_from(year).ok()?, month, day)
}

impl IsoDate {
    pub fn parse(value: &str) -> Option<Self> {
        parse_date(value, true)
    }

    pub fn month_start(self) -> Self {
        Self { day: 1, ..self }
    }

    pub fn format_month(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }

    pub fn week_start(self, start: WeekDay) -> Self {
        let start = match start {
            WeekDay::Sunday => 0,
            WeekDay::Monday => 1,
            WeekDay::Tuesday => 2,
            WeekDay::Wednesday => 3,
            WeekDay::Thursday => 4,
            WeekDay::Friday => 5,
            WeekDay::Saturday => 6,
        };
        let shift = (i64::from(self.weekday_from_sunday()) - start).rem_euclid(7);
        Self::from_epoch_days(self.days_since_epoch() - shift)
    }

    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    pub fn days_since_epoch(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn from_epoch_days(days: i64) -> Self {
        let (year, month, day) = civil_from_days(days);
        Self { year, month, day }
    }

    pub fn parts(self) -> (i32, u32, u32) {
        (self.year, self.month, self.day)
    }

    pub fn civil(self) -> Option<JiffDate> {
        JiffDate::new(
            i16::try_from(self.year).ok()?,
            i8::try_from(self.month).ok()?,
            i8::try_from(self.day).ok()?,
        )
        .ok()
    }

    fn weekday_from_sunday(self) -> u32 {
        weekday_from_epoch_days(self.days_since_epoch())
    }

    pub fn checked_add_days(self, days: i64) -> Option<Self> {
        let days = self.days_since_epoch().checked_add(days)?;
        Some(Self::from_epoch_days(days))
    }
}

pub fn weekday_from_epoch_days(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

impl fmt::Display for IsoDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
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
