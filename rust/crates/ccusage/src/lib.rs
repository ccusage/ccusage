use ccusage_cli::ReportClock;

pub fn report_clock() -> Result<ReportClock, String> {
    #[cfg(feature = "test-clock")]
    if let Some(now) = ccusage_cli::environment::Environment::current().var_os("CCUSAGE_TEST_NOW") {
        return parse_test_now(&now.to_string_lossy());
    }
    Ok(ReportClock::System)
}

#[cfg(any(test, feature = "test-clock"))]
fn parse_test_now(value: &str) -> Result<ReportClock, String> {
    value
        .parse::<jiff::Timestamp>()
        .map(ReportClock::Fixed)
        .map_err(|error| format!("CCUSAGE_TEST_NOW={value} is not an RFC 3339 instant: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOON: i64 = 1_767_355_200_000;

    #[test]
    fn pins_the_test_clock_to_any_rfc3339_precision() {
        for value in [
            "2026-01-02T12:00:00Z",
            "2026-01-02T12:00:00.0Z",
            "2026-01-02T12:00:00.000000Z",
            "2026-01-02T12:00:00.000000000Z",
            "2026-01-02T21:00:00+09:00",
        ] {
            assert_eq!(
                parse_test_now(value).map(|now| now.now().as_millisecond()),
                Ok(NOON),
                "{value}"
            );
        }
        assert_eq!(
            parse_test_now("2026-01-02T12:00:00.123456Z").map(|now| now.now().as_millisecond()),
            Ok(NOON + 123)
        );
    }

    #[test]
    fn explains_a_test_clock_that_is_not_an_instant() {
        let error = parse_test_now("2026-01-02").unwrap_err();
        assert!(
            error.starts_with("CCUSAGE_TEST_NOW=2026-01-02 is not an RFC 3339 instant: "),
            "{error}"
        );
    }
}
