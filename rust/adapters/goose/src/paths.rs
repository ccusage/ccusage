use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

pub(super) const GOOSE_PATH_ROOT_ENV: &str = "GOOSE_PATH_ROOT";
pub(super) const GOOSE_DB_FILE_NAME: &str = "sessions.db";

pub(super) fn goose_db_paths() -> Result<Vec<PathBuf>> {
    let candidates = if let Ok(root) = env::var(GOOSE_PATH_ROOT_ENV) {
        let root = root.trim();
        if root.is_empty() {
            default_goose_db_candidates()?
        } else {
            vec![
                PathBuf::from(root)
                    .join("data")
                    .join("sessions")
                    .join(GOOSE_DB_FILE_NAME),
            ]
        }
    } else {
        default_goose_db_candidates()?
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

fn default_goose_db_candidates() -> Result<Vec<PathBuf>> {
    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(vec![
        home.join(".local/share/goose/sessions")
            .join(GOOSE_DB_FILE_NAME),
        home.join("Library/Application Support/goose/sessions")
            .join(GOOSE_DB_FILE_NAME),
        home.join(".local/share/Block/goose/sessions")
            .join(GOOSE_DB_FILE_NAME),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_goose_path_root_database() {
        let fixture = fs_fixture!({
            "data/sessions/sessions.db": "",
        });
        let _cleanup = EnvVarGuard::set(GOOSE_PATH_ROOT_ENV, fixture.root());

        let paths = goose_db_paths().unwrap();

        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with(Path::new("data/sessions/sessions.db")));
    }
}
