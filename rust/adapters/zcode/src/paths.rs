use std::{collections::HashSet, fs, path::PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::{Result, cli::SharedArgs, debug_log};

pub(crate) const ZCODE_HOME_ENV: &str = "ZCODE_HOME";
pub(crate) const ZCODE_DB_RELATIVE_PATH: &str = "cli/db/db.sqlite";

fn paths(shared: &SharedArgs) -> Result<Vec<PathBuf>> {
    let configured_roots = RootsEnv::List(ZCODE_HOME_ENV).configured();
    let configured = configured_roots.is_some();
    let candidates = configured_roots.unwrap_or_else(|| {
        roots::default_candidates(shared.debug, || {
            crate::cli::home::home_dir()
                .map(|home| home.join(".zcode"))
                .into_iter()
                .collect()
        })
    });
    Ok(roots::directories_with(candidates, |path, error| {
        if configured {
            debug_log(
                shared,
                format!(
                    "Ignoring configured ZCODE_HOME root {}: {error}",
                    path.display()
                ),
            );
        }
    }))
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

#[cfg(test)]
mod tests {
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
        Environment::with_vars([
            (ZCODE_HOME_ENV, Some(raw.into())),
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert_eq!(paths(&shared()).unwrap(), vec![first, second]);
        })
    }

    use std::ffi::OsString;

    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;

    fn shared() -> SharedArgs {
        SharedArgs {
            debug: true,
            ..SharedArgs::default()
        }
    }

    #[test]
    fn configured_homes_preserve_list_order_and_deduplicate() {
        let fixture = fs_fixture!({});
        let first = fixture.create_dir_all("first");
        let second = fixture.create_dir_all("second");
        Environment::with_vars([
            (
                ZCODE_HOME_ENV,
                Some(
                    format!(
                        " {}, {}, {} ",
                        first.display(),
                        second.display(),
                        first.join(".").display()
                    )
                    .into(),
                ),
            ),
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert_eq!(paths(&shared()).unwrap(), vec![first, second]);
        })
    }

    #[test]
    fn invalid_configured_roots_do_not_fall_back_to_default() {
        let fixture = fs_fixture!({
            ".zcode/cli/db/db.sqlite": "not a database",
        });
        Environment::with_vars([
            (
                ZCODE_HOME_ENV,
                Some(fixture.path("missing").into_os_string()),
            ),
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert!(paths(&shared()).unwrap().is_empty());
        })
    }

    #[test]
    fn empty_configured_value_uses_default_home() {
        let fixture = fs_fixture!({
            ".zcode/cli/db/db.sqlite": "not a database",
        });
        Environment::with_vars([
            (ZCODE_HOME_ENV, Some(OsString::new())),
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert_eq!(paths(&shared()).unwrap(), vec![fixture.path(".zcode")]);
        })
    }
}
