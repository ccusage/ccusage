use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use assert_fs::{
    TempDir,
    fixture::{ChildPath, FileWriteStr, PathChild, PathCreateDir},
};

pub mod mutation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instant(jiff::Timestamp);

pub fn instant(timestamp: &str) -> Instant {
    Instant(
        timestamp
            .parse()
            .expect("fixture instant is valid RFC 3339"),
    )
}

impl Instant {
    pub fn secs(self) -> i64 {
        self.0.as_second()
    }

    pub fn millis(self) -> i64 {
        self.0.as_millisecond()
    }

    pub fn secs_f64(self) -> f64 {
        self.0
            .duration_since(jiff::Timestamp::UNIX_EPOCH)
            .as_secs_f64()
    }

    pub fn millis_f64(self) -> f64 {
        self.0
            .duration_since(jiff::Timestamp::UNIX_EPOCH)
            .as_millis_f64()
    }

    pub fn rfc3339_millis(self) -> String {
        format!("{:.3}", self.0)
    }

    pub fn plus(self, duration: Duration) -> Self {
        Instant(
            self.0
                .checked_add(duration)
                .expect("fixture instant stays in range"),
        )
    }

    pub fn minus(self, duration: Duration) -> Self {
        Instant(
            self.0
                .checked_sub(duration)
                .expect("fixture instant stays in range"),
        )
    }
}

impl From<Instant> for jiff::Timestamp {
    fn from(instant: Instant) -> Self {
        instant.0
    }
}

pub fn set_modified(path: &Path, modified: Instant) {
    fs::File::options()
        .write(true)
        .open(path)
        .expect("failed to open fixture file")
        .set_modified(std::time::SystemTime::from(modified.0))
        .expect("failed to set fixture file mtime");
}

pub fn install_pricing_snapshots() {
    ccusage_core::pricing::set_embedded_snapshots(ccusage_core::pricing::EmbeddedSnapshots {
        litellm_sources: ccusage_pricing_snapshots::LITELLM_SOURCES,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
        pricing_table: ccusage_pricing_snapshots::PRICING_TABLE,
        models_dev_table: ccusage_pricing_snapshots::MODELS_DEV_TABLE,
    });
}

pub use ccusage_core::cli::environment::Environment;

pub fn read_fixture_dir(dir: &Path) -> std::io::Result<fs::ReadDir> {
    fs::read_dir(dir)
}

pub struct Fixture {
    dir: TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("failed to create temporary fixture directory"),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    #[must_use]
    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.dir.path().join(path)
    }

    fn child(&self, path: impl AsRef<Path>) -> ChildPath {
        self.dir.child(path)
    }

    pub fn write_file(&self, path: impl AsRef<Path>, contents: impl AsRef<str>) -> PathBuf {
        let child = self.child(path);
        if let Some(parent) = child.path().parent() {
            fs::create_dir_all(parent).expect("failed to create fixture file parent directory");
        }
        child
            .write_str(contents.as_ref())
            .expect("failed to write fixture file");
        child.path().to_path_buf()
    }

    pub fn create_dir_all(&self, path: impl AsRef<Path>) -> PathBuf {
        let child = self.child(path);
        child
            .create_dir_all()
            .expect("failed to create fixture directory");
        child.path().to_path_buf()
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

#[macro_export]
macro_rules! fs_fixture {
    ({ $($path:literal : $contents:expr_2021),* $(,)? }) => {{
        let fixture = $crate::Fixture::new();
        $(
            fixture.write_file($path, $contents);
        )*
        fixture
    }};
}

#[cfg(test)]
mod tests {
    use super::{Fixture, instant, set_modified};

    #[test]
    fn fixture_instant_preserves_epoch_units_and_fractional_seconds() {
        let timestamp = instant("2026-01-02T08:31:00.125Z");

        assert_eq!(timestamp.secs(), 1_767_342_660);
        assert_eq!(timestamp.millis(), 1_767_342_660_125);
        assert_eq!(timestamp.secs_f64(), 1_767_342_660.125);
        assert_eq!(timestamp.millis_f64(), 1_767_342_660_125.0);
        assert_eq!(timestamp, instant("2026-01-02T17:31:00.125+09:00"));
    }

    #[test]
    fn fixture_instant_steps_by_durations_and_prints_milliseconds() {
        let timestamp = instant("2026-01-02T08:31:00Z");
        let hour = std::time::Duration::from_secs(60 * 60);

        assert_eq!(timestamp.plus(hour), instant("2026-01-02T09:31:00Z"));
        assert_eq!(timestamp.minus(hour * 9), instant("2026-01-01T23:31:00Z"));
        assert_eq!(timestamp.rfc3339_millis(), "2026-01-02T08:31:00.000Z");
        assert_eq!(
            instant("2026-01-02T08:31:00.125Z").rfc3339_millis(),
            "2026-01-02T08:31:00.125Z"
        );
    }

    #[test]
    fn fixture_file_mtime_preserves_the_instant() {
        let fixture = Fixture::new();
        let path = fixture.write_file("fixture.jsonl", "{}");
        let timestamp = instant("2026-01-02T08:31:00.125Z");

        set_modified(&path, timestamp);

        let modified = std::fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(
            modified.duration_since(std::time::UNIX_EPOCH).unwrap(),
            std::time::Duration::from_millis(1_767_342_660_125)
        );
    }

    #[test]
    fn creates_inline_fixture_tree() {
        let fixture = fs_fixture!({
            "projects/example/session.jsonl": "{}\n",
        });

        assert_eq!(
            std::fs::read_to_string(fixture.path("projects/example/session.jsonl")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn creates_incremental_fixture_tree() {
        let fixture = fs_fixture!({});
        fixture.write_file("projects/example/session/chat.jsonl", "{}\n");

        assert!(
            fixture
                .path("projects/example/session/chat.jsonl")
                .is_file()
        );
    }
}
