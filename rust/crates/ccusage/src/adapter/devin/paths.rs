use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

const DEVIN_DATA_DIR_ENV: &str = "DEVIN_DATA_DIR";

pub(super) fn paths(custom_path: Option<&str>) -> Result<Vec<PathBuf>> {
    if let Some(custom_path) = custom_path.filter(|path| !path.trim().is_empty()) {
        return Ok(existing_path_list(custom_path));
    }
    if let Ok(env_paths) = env::var(DEVIN_DATA_DIR_ENV)
        && !env_paths.trim().is_empty()
    {
        return Ok(existing_path_list(&env_paths));
    }

    if let Some(data_dir) = default_data_dir() {
        return Ok(data_dir.is_dir().then_some(data_dir).into_iter().collect());
    }

    Ok(Vec::new())
}

fn default_data_dir() -> Option<PathBuf> {
    if let Some(app_data) = env::var_os("APPDATA").and_then(|path| {
        let path = PathBuf::from(path);
        (!path.as_os_str().is_empty()).then_some(path)
    }) {
        return Some(app_data.join("devin").join("cli"));
    }

    let home = crate::home::home_dir()?;
    Some(home.join(".local").join("share").join("devin").join("cli"))
}

fn existing_path_list(raw: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir() && seen.insert(path.clone()))
        .collect()
}
