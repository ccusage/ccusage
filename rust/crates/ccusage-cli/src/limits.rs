use std::{num::NonZeroU64, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenLimit {
    Max,
    Tokens(NonZeroU64),
}

impl FromStr for TokenLimit {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "max" {
            return Ok(Self::Max);
        }
        value
            .parse()
            .map(Self::Tokens)
            .map_err(|_| "Expected max or a positive whole number".to_string())
    }
}

impl std::fmt::Display for TokenLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Max => formatter.write_str("max"),
            Self::Tokens(value) => value.fmt(formatter),
        }
    }
}

impl TokenLimit {
    pub fn resolve(self, max_tokens: u64) -> Option<NonZeroU64> {
        match self {
            Self::Max => NonZeroU64::new(max_tokens),
            Self::Tokens(tokens) => Some(tokens),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionLength(i64);

impl SessionLength {
    pub const DEFAULT: Self = Self(5 * 3_600_000);

    pub fn from_hours(hours: f64) -> Result<Self, String> {
        let error =
            || "Session length must be a positive, representable number of hours".to_string();
        if !hours.is_finite() || hours <= 0.0 {
            return Err(error());
        }
        let duration =
            std::time::Duration::try_from_secs_f64(hours * 3600.0).map_err(|_| error())?;
        let millis = i64::try_from(duration.as_millis()).map_err(|_| error())?;
        if millis == 0 {
            return Err(error());
        }
        Ok(Self(millis))
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

impl FromStr for SessionLength {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hours(
            value
                .parse()
                .map_err(|_| "Expected a number of hours".to_string())?,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextThresholds {
    low: u8,
    medium: u8,
}

impl Default for ContextThresholds {
    fn default() -> Self {
        Self {
            low: 50,
            medium: 80,
        }
    }
}

impl ContextThresholds {
    pub fn new(low: u8, medium: u8) -> Result<Self, String> {
        if low >= medium {
            return Err(format!(
                "Context low threshold ({low}) must be less than medium threshold ({medium})"
            ));
        }
        Ok(Self { low, medium })
    }

    pub const fn low(self) -> u8 {
        self.low
    }

    pub const fn medium(self) -> u8 {
        self.medium
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_token_limits_require_a_completed_nonempty_block() {
        assert_eq!(TokenLimit::Max.resolve(0), None);
        assert_eq!(TokenLimit::Max.resolve(42), NonZeroU64::new(42));
        assert_eq!(
            "17".parse::<TokenLimit>().unwrap().resolve(0),
            NonZeroU64::new(17)
        );
    }

    #[test]
    fn session_length_converts_hours_to_positive_milliseconds() {
        assert_eq!(
            SessionLength::from_hours(6.5).unwrap().as_millis(),
            23_400_000
        );
        assert_eq!(
            SessionLength::from_hours(5.0).unwrap(),
            SessionLength::DEFAULT
        );
        for (hours, millis) in [(4.35, 15_660_000), (0.7, 2_520_000)] {
            assert_eq!(
                SessionLength::from_hours(hours).unwrap().as_millis(),
                millis
            );
        }
        for hours in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e300, 1e-10] {
            assert!(SessionLength::from_hours(hours).is_err());
        }
    }
}
