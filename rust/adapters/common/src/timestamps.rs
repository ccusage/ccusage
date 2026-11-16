use std::{fs, path::Path, time::UNIX_EPOCH};

use ccusage_core::TimestampMs;

const SECONDS_BELOW: i64 = 10_000_000_000;

/// When `path` was last modified, or `None` when the file system cannot say.
pub fn file_mtime(path: &Path) -> Option<TimestampMs> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(millis).ok().map(TimestampMs::from_millis)
}

/// A positive epoch number whose unit the source does not fix: seconds when
/// that is before the year 2286, milliseconds otherwise. A source that always
/// writes one unit converts with that unit instead.
pub fn epoch_secs_or_millis(raw: i64) -> Option<TimestampMs> {
    if raw <= 0 {
        return None;
    }
    if raw < SECONDS_BELOW {
        return TimestampMs::from_unix_seconds(raw);
    }
    Some(TimestampMs::from_millis(raw))
}

#[cfg(test)]
mod tests {
    use ccusage_core::TimestampMs;
    use ccusage_test_support::Fixture;

    use super::{epoch_secs_or_millis, file_mtime};

    #[test]
    fn reads_an_epoch_number_by_its_magnitude() {
        assert_eq!(
            epoch_secs_or_millis(1_767_225_600),
            Some(TimestampMs::from_millis(1_767_225_600_000))
        );
        assert_eq!(
            epoch_secs_or_millis(1_767_225_600_000),
            Some(TimestampMs::from_millis(1_767_225_600_000))
        );
        assert_eq!(
            epoch_secs_or_millis(9_999_999_999),
            Some(TimestampMs::from_millis(9_999_999_999_000))
        );
        assert_eq!(
            epoch_secs_or_millis(10_000_000_000),
            Some(TimestampMs::from_millis(10_000_000_000))
        );
    }

    #[test]
    fn a_non_positive_epoch_number_is_no_timestamp() {
        assert_eq!(epoch_secs_or_millis(0), None);
        assert_eq!(epoch_secs_or_millis(-1), None);
    }

    #[test]
    fn a_missing_file_has_no_mtime() {
        let fixture = Fixture::new();
        let file = fixture.write_file("present.json", "{}");

        assert!(file_mtime(&file).is_some_and(|mtime| mtime > TimestampMs::UNIX_EPOCH));
        assert_eq!(file_mtime(&fixture.path("missing.json")), None);
    }
}
