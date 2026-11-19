use std::{fs, path::Path, time::UNIX_EPOCH};

use ccusage_core::TimestampMs;

const SECONDS_BELOW: i64 = 10_000_000_000;

pub fn file_mtime(path: &Path) -> Option<TimestampMs> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(millis).ok().map(TimestampMs::from_millis)
}

pub fn epoch_secs_or_millis(raw: i64) -> Option<TimestampMs> {
    if raw <= 0 {
        return None;
    }
    let millis = if raw < SECONDS_BELOW {
        raw.checked_mul(1_000)?
    } else {
        raw
    };
    TimestampMs::from_instant_millis(millis)
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
    fn an_epoch_number_that_names_no_instant_is_no_timestamp() {
        assert_eq!(epoch_secs_or_millis(0), None);
        assert_eq!(epoch_secs_or_millis(-1), None);
        assert_eq!(epoch_secs_or_millis(i64::MAX), None);
    }

    #[test]
    fn a_missing_file_has_no_mtime() {
        let fixture = Fixture::new();
        let file = fixture.write_file("present.json", "{}");

        assert!(file_mtime(&file).is_some_and(|mtime| mtime > TimestampMs::UNIX_EPOCH));
        assert_eq!(file_mtime(&fixture.path("missing.json")), None);
    }
}
