use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

const HERMES_HOME_ENV: &str = "HERMES_HOME";

pub(super) fn hermes_state_db_paths() -> Result<Vec<PathBuf>> {
    let homes = if let Ok(paths) = env::var(HERMES_HOME_ENV) {
        paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    } else {
        let home =
            crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
        vec![home.join(".hermes")]
    };
    let mut seen = HashSet::new();
    Ok(homes
        .into_iter()
        .map(|home| home.join("state.db"))
        .filter(|path| path.is_file())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}
