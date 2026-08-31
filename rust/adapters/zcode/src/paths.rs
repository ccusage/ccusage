use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::{Result, cli::SharedArgs, debug_log};

pub(crate) const ZCODE_HOME_ENV: &str = "ZCODE_HOME";
pub(crate) const ZCODE_DB_RELATIVE_PATH: &str = "cli/db/db.sqlite";

fn paths(shared: &SharedArgs) -> Result<Vec<PathBuf>> {
    if let Some(raw) = env::var_os(ZCODE_HOME_ENV) {
        let configured = configured_roots(raw);
        if !configured.is_empty() {
            return Ok(unique_roots(configured, shared, true));
        }
    }

    let Some(home) = crate::home::home_dir() else {
        debug_log(shared, "Unable to resolve the default ZCode home");
        return Ok(Vec::new());
    };
    Ok(unique_roots([home.join(".zcode")], shared, false))
}

pub(super) fn db_paths(shared: &SharedArgs) -> Result<Vec<PathBuf>> {
    let mut db_paths = Vec::new();
    let mut seen = HashSet::new();
    for root in paths(shared)? {
        let db_path = root.join(ZCODE_DB_RELATIVE_PATH);
        let canonical = match fs::canonicalize(&db_path) {
            Ok(path) if path.is_file() => path,
            Ok(path) => {
                debug_log(
                    shared,
                    format!("ZCode database is not a file: {}", path.display()),
                );
                continue;
            }
            Err(error) => {
                debug_log(
                    shared,
                    format!(
                        "Unable to access ZCode database {}: {error}",
                        db_path.display()
                    ),
                );
                continue;
            }
        };
        if seen.insert(canonical.clone()) {
            db_paths.push(canonical);
        }
    }
    Ok(db_paths)
}

fn configured_roots(raw: std::ffi::OsString) -> Vec<PathBuf> {
    match raw.into_string() {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(raw) => vec![PathBuf::from(raw)],
    }
}

fn unique_roots(
    roots: impl IntoIterator<Item = PathBuf>,
    shared: &SharedArgs,
    configured: bool,
) -> Vec<PathBuf> {
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let Some(root) = canonical_directory(&root, shared, configured) else {
            continue;
        };
        if seen.insert(root.clone()) {
            resolved.push(root);
        }
    }
    resolved
}

fn canonical_directory(path: &Path, shared: &SharedArgs, configured: bool) -> Option<PathBuf> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            if configured {
                debug_log(
                    shared,
                    format!(
                        "Ignoring configured ZCODE_HOME root {}: {error}",
                        path.display()
                    ),
                );
            }
            return None;
        }
    };
    if !metadata.is_dir() {
        if configured {
            debug_log(
                shared,
                format!(
                    "Ignoring configured ZCODE_HOME root {}: not a directory",
                    path.display()
                ),
            );
        }
        return None;
    }
    match fs::canonicalize(path) {
        Ok(path) => Some(path),
        Err(error) => {
            if configured {
                debug_log(
                    shared,
                    format!(
                        "Ignoring configured ZCODE_HOME root {}: {error}",
                        path.display()
                    ),
                );
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::*;

    fn shared() -> SharedArgs {
        SharedArgs {
            debug: true,
            ..SharedArgs::default()
        }
    }

    #[test]
    fn discovers_and_deduplicates_configured_roots() {
        let fixture = fs_fixture!({});
        let first = fixture.create_dir_all("first");
        let second = fixture.create_dir_all("second");
        let raw = format!(
            " {}, {}, {} ",
            first.display(),
            second.display(),
            first.display()
        );
        let _guard = EnvVarsGuard::set_many([
            (ZCODE_HOME_ENV, Some(OsString::from(raw))),
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ]);

        assert_eq!(
            paths(&shared()).unwrap(),
            vec![
                first.canonicalize().unwrap(),
                second.canonicalize().unwrap()
            ]
        );
    }

    #[test]
    fn invalid_configured_roots_do_not_fall_back_to_default() {
        let fixture = fs_fixture!({
            ".zcode/cli/db/db.sqlite": "not a database",
        });
        let _guard = EnvVarsGuard::set_many([
            (
                ZCODE_HOME_ENV,
                Some(fixture.path("missing").into_os_string()),
            ),
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ]);

        assert!(paths(&shared()).unwrap().is_empty());
    }

    #[test]
    fn empty_configured_value_uses_default_home() {
        let fixture = fs_fixture!({
            ".zcode/cli/db/db.sqlite": "not a database",
        });
        let _guard = EnvVarsGuard::set_many([
            (ZCODE_HOME_ENV, Some(OsString::new())),
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ]);

        assert_eq!(
            paths(&shared()).unwrap(),
            vec![fixture.path(".zcode").canonicalize().unwrap()]
        );
    }
}
