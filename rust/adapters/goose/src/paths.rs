use std::{collections::HashSet, path::PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const GOOSE_PATH_ROOT_ENV: &str = "GOOSE_PATH_ROOT";
pub(super) const GOOSE_DB_FILE_NAME: &str = "sessions.db";

pub(super) fn goose_db_paths(debug: bool) -> Result<Vec<PathBuf>> {
    let roots = RootsEnv::Single(GOOSE_PATH_ROOT_ENV)
        .configured()
        .map(|paths| {
            paths
                .into_iter()
                .map(|root| root.join("data/sessions"))
                .collect()
        })
        .unwrap_or_else(|| roots::default_candidates(debug, default_goose_db_candidates));
    let mut seen = HashSet::new();
    Ok(roots::directories(roots)
        .into_iter()
        .map(|root| root.join(GOOSE_DB_FILE_NAME))
        .map(|path| crate::cli::home::path_identity(&path))
        .filter(|path| path.is_file())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

fn default_goose_db_candidates() -> Vec<PathBuf> {
    crate::cli::home::home_dir()
        .into_iter()
        .flat_map(|home| {
            [
                ".local/share/goose/sessions",
                "Library/Application Support/goose/sessions",
                ".local/share/Block/goose/sessions",
            ]
            .map(|path| home.join(path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn database_identity_keeps_the_given_path_when_it_cannot_be_resolved() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("unresolved/sessions.db");
        assert!(path.canonicalize().is_err());
        assert_eq!(crate::cli::home::path_identity(&path), path);
    }

    #[cfg(unix)]
    #[test]
    fn default_roots_read_a_shared_database_once() {
        let fixture = ccusage_test_support::Fixture::new();
        let database = fixture.write_file(".local/share/goose/sessions/sessions.db", "");
        let alias_root = fixture.create_dir_all("Library/Application Support/goose/sessions");
        std::os::unix::fs::symlink(&database, alias_root.join("sessions.db")).unwrap();
        ccusage_test_support::Environment::with_vars([
            (GOOSE_PATH_ROOT_ENV, None),
            ("HOME", Some(fixture.root().into())),
        ])
        .run(|| {
            assert_eq!(
                goose_db_paths(false).unwrap(),
                vec![database.canonicalize().unwrap()]
            );
        })
    }
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn discovers_goose_path_root_database() {
        let fixture = fs_fixture!({
            "data/sessions/sessions.db": "",
        });
        Environment::with_var(GOOSE_PATH_ROOT_ENV, fixture.root()).run(|| {
            let paths = goose_db_paths(false).unwrap();

            assert_eq!(paths.len(), 1);
            assert!(paths[0].ends_with(Path::new("data/sessions/sessions.db")));
        })
    }

    #[test]
    fn configured_home_is_one_root_even_with_commas() {
        let fixture = ccusage_test_support::fs_fixture!({
            "first,second/data/sessions/sessions.db": "",
        });
        ccusage_test_support::Environment::with_var(
            GOOSE_PATH_ROOT_ENV,
            format!(" {} ", fixture.path("first,second").display()),
        )
        .run(|| {
            assert_eq!(
                goose_db_paths(false).unwrap(),
                vec![
                    fixture
                        .path("first,second/data/sessions/sessions.db")
                        .canonicalize()
                        .unwrap()
                ]
            );
        })
    }
}
