use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use assert_fs::{
    TempDir,
    fixture::{ChildPath, FileWriteStr, PathChild, PathCreateDir},
};

pub mod mutation;
pub mod zcode;

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
        litellm_pricing: ccusage_pricing_snapshots::LITELLM_PRICING,
        models_dev_pricing: ccusage_pricing_snapshots::MODELS_DEV_PRICING,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
    });
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvVarGuard {
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let guard = env_lock();
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self {
            key,
            previous,
            _guard: guard,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

pub struct EnvVarsGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvVarsGuard {
    pub fn set_many(vars: impl IntoIterator<Item = (&'static str, Option<OsString>)>) -> Self {
        let guard = env_lock();
        let mut previous = Vec::new();
        for (key, value) in vars {
            previous.push((key, std::env::var_os(key)));
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for EnvVarsGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..).rev() {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
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

    #[must_use]
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

    #[must_use]
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
            let _ = fixture.write_file($path, $contents);
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
        let _ = fixture.write_file("projects/example/session/chat.jsonl", "{}\n");

        assert!(
            fixture
                .path("projects/example/session/chat.jsonl")
                .is_file()
        );
    }
}
