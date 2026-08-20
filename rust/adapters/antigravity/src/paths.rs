use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::Result;

pub const ANTIGRAVITY_HOME_ENV: &str = "ANTIGRAVITY_HOME";
pub const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";
pub const AGY_HOME_ENV: &str = "AGY_HOME";

pub(super) fn antigravity_db_paths() -> Result<Vec<PathBuf>> {
    let mut candidate_dirs = Vec::new();

    for var in [ANTIGRAVITY_HOME_ENV, ANTIGRAVITY_DATA_DIR_ENV, AGY_HOME_ENV] {
        if let Ok(val) = env::var(var) {
            for part in val.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let path = PathBuf::from(part);
                if path.join("conversations").is_dir() {
                    candidate_dirs.push(path.join("conversations"));
                } else if path.is_dir() {
                    candidate_dirs.push(path);
                }
            }
        }
    }

    if candidate_dirs.is_empty()
        && let Some(home) = crate::home::home_dir()
    {
        let primary = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("conversations");
        if primary.is_dir() {
            candidate_dirs.push(primary);
        }
        let config_alt = home
            .join(".config")
            .join("antigravity")
            .join("conversations");
        if config_alt.is_dir() {
            candidate_dirs.push(config_alt);
        }
    }

    let mut seen = HashSet::new();
    let mut db_paths = Vec::new();

    for dir in candidate_dirs {
        collect_db_files(&dir, &mut db_paths, &mut seen);
    }

    Ok(db_paths)
}

fn collect_db_files(dir: &Path, db_paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_file()
            && path.extension().is_some_and(|ext| ext == "db")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.contains("summary") && !name.contains("summaries"))
            && seen.insert(path.clone())
        {
            db_paths.push(path);
        }
    }
}
