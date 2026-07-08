use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

/// Direct path override: points at the archive.db file itself.
pub(super) const BUZZ_ARCHIVE_PATH_ENV: &str = "BUZZ_ARCHIVE_PATH";

/// Root directory override: ccusage joins `archive/archive.db` to this.
/// Mirrors the `GOOSE_PATH_ROOT` pattern so test fixtures can set a temp root.
pub(super) const BUZZ_PATH_ROOT_ENV: &str = "BUZZ_PATH_ROOT";

pub(super) const BUZZ_ARCHIVE_FILE_NAME: &str = "archive.db";

pub(super) fn buzz_db_paths() -> Result<Vec<PathBuf>> {
    let candidates = if let Ok(direct) = env::var(BUZZ_ARCHIVE_PATH_ENV) {
        let direct = direct.trim();
        if direct.is_empty() {
            default_buzz_db_candidates()?
        } else {
            vec![PathBuf::from(direct)]
        }
    } else if let Ok(root) = env::var(BUZZ_PATH_ROOT_ENV) {
        let root = root.trim();
        if root.is_empty() {
            default_buzz_db_candidates()?
        } else {
            vec![
                PathBuf::from(root)
                    .join("archive")
                    .join(BUZZ_ARCHIVE_FILE_NAME),
            ]
        }
    } else {
        default_buzz_db_candidates()?
    };

    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for path in candidates {
        let path = path.canonicalize().unwrap_or(path);
        if path.is_file() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn default_buzz_db_candidates() -> Result<Vec<PathBuf>> {
    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(vec![
        home.join(".buzz")
            .join("archive")
            .join(BUZZ_ARCHIVE_FILE_NAME),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_buzz_archive_path_env_directly() {
        let fixture = fs_fixture!({
            "archive.db": "",
        });
        let _cleanup = EnvVarGuard::set(BUZZ_ARCHIVE_PATH_ENV, fixture.path("archive.db"));

        let paths = buzz_db_paths().unwrap();

        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with(Path::new("archive.db")));
    }

    #[test]
    fn discovers_buzz_path_root_database() {
        let fixture = fs_fixture!({
            "archive/archive.db": "",
        });
        let _cleanup = EnvVarGuard::set(BUZZ_PATH_ROOT_ENV, fixture.root());

        let paths = buzz_db_paths().unwrap();

        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with(Path::new("archive/archive.db")));
    }

    #[test]
    fn returns_empty_when_no_db_exists() {
        let fixture = fs_fixture!({});
        let _cleanup = EnvVarGuard::set(BUZZ_PATH_ROOT_ENV, fixture.root());

        let paths = buzz_db_paths().unwrap();

        assert!(paths.is_empty());
    }
}
