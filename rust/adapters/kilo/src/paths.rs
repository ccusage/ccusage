use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use crate::Result;

pub(super) const KILO_DATA_DIR_ENV: &str = "KILO_DATA_DIR";
pub(super) const KILO_DB_FILE_NAME: &str = "kilo.db";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(KILO_DATA_DIR_ENV) {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(raw);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        return Ok(paths);
    }

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".local").join("share").join("kilo");
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub(super) fn db_path(kilo_dir: &Path) -> Option<PathBuf> {
    let path = kilo_dir.join(KILO_DB_FILE_NAME);
    path.is_file().then_some(path)
}
