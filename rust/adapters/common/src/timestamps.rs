use std::{
    fs::{self, Metadata},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use ccusage_core::TimestampMs;

const SECONDS_BELOW: i64 = 10_000_000_000;

pub fn file_mtime(path: &Path) -> Option<TimestampMs> {
    epoch_millis(fs::metadata(path).ok()?.modified().ok()?).map(TimestampMs::from_millis)
}

fn epoch_millis(time: SystemTime) -> Option<i64> {
    i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

// NOTE(2026-11-23, #2531): On Unix every write, copy, restore or touch resets a file's ctime, while mtime follows writes alone; elsewhere creation time stands in for ctime. The later of the two therefore comes after every record a log holds, even when a restore kept an older mtime.
pub(crate) fn changed_before(path: &Path, cutoff: i64) -> bool {
    fs::metadata(path)
        .ok()
        .as_ref()
        .and_then(changed_millis)
        .is_some_and(|changed| changed < cutoff)
}

fn changed_millis(metadata: &Metadata) -> Option<i64> {
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        let millis = metadata.ctime().checked_mul(1_000)?;
        millis.checked_add(metadata.ctime_nsec() / 1_000_000)?
    };
    #[cfg(not(unix))]
    let changed = epoch_millis(metadata.created().ok()?)?;
    Some(changed.max(epoch_millis(metadata.modified().ok()?)?))
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
    use std::time::SystemTime;

    use ccusage_core::TimestampMs;
    use ccusage_test_support::{Fixture, instant, set_modified};

    use super::{changed_before, epoch_millis, epoch_secs_or_millis, file_mtime};

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

    #[test]
    fn counts_a_file_as_changed_at_the_later_of_its_ctime_and_mtime() {
        let fixture = Fixture::new();
        let restored = fixture.write_file("restored.jsonl", "{}\n");
        let touched = fixture.write_file("touched.jsonl", "{}\n");
        set_modified(&restored, instant("2026-01-01T00:00:00Z"));
        set_modified(&touched, instant("2099-01-01T00:00:00Z"));
        let now = epoch_millis(SystemTime::now()).unwrap();
        let hour = 3_600_000;

        assert!(!changed_before(&restored, now - hour));
        assert!(changed_before(&restored, now + hour));
        assert!(!changed_before(&touched, now + hour));
        assert!(!changed_before(&fixture.path("missing.jsonl"), now + hour));
    }
}
