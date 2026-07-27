use std::{collections::HashSet, env, path::PathBuf};

use crate::{Result, collect_files_with_extension};

pub(super) const DROID_SESSIONS_DIR_ENV: &str = "DROID_SESSIONS_DIR";

pub(super) fn discover_settings_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in droid_session_paths()? {
        collect_files_with_extension(&root, "json", &mut files);
    }
    Ok(files
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".settings.json"))
        })
        .collect())
}

fn droid_session_paths() -> Result<Vec<PathBuf>> {
    let raw_paths = if let Ok(paths) = env::var(DROID_SESSIONS_DIR_ENV) {
        paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    } else {
        let home =
            crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
        vec![home.join(".factory").join("sessions")]
    };
    let mut seen = HashSet::new();
    Ok(raw_paths
        .into_iter()
        .filter(|path| path.is_dir())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}
