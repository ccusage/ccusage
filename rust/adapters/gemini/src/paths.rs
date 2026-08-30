use std::{collections::HashSet, env, path::PathBuf};

use crate::{Result, collect_files_with_extension};

pub(super) const GEMINI_DATA_DIR_ENV: &str = "GEMINI_DATA_DIR";
pub(super) const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";

/// Returns all discovery candidate directories for Gemini and Antigravity logs.
fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(GEMINI_DATA_DIR_ENV) {
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

    if let Ok(env_paths) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
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
    }

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".gemini").join("tmp");
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
        let antigravity_dirs = [
            home.join(".gemini")
                .join("antigravity")
                .join("conversations"),
            home.join(".gemini")
                .join("antigravity-cli")
                .join("conversations"),
            home.join(".gemini")
                .join("antigravity-ide")
                .join("conversations"),
            home.join(".gemini")
                .join("antigravity-backup")
                .join("conversations"),
        ];
        for dir in antigravity_dirs {
            if dir.is_dir() && seen.insert(dir.clone()) {
                paths.push(dir);
            }
        }
    }
    Ok(paths)
}

/// Discovers all `.json`, `.jsonl`, and SQLite `.db` log files across known directories.
pub(super) fn discover_log_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths()? {
        collect_files_with_extension(&path, "json", &mut files);
        collect_files_with_extension(&path, "jsonl", &mut files);
        collect_files_with_extension(&path, "db", &mut files);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn discovers_json_and_jsonl_logs() {
        let fixture = fs_fixture!({
            "chats/a.json": "{}",
            "chats/b.jsonl": "{}\n",
            "chats/ignore.txt": "no",
        });
        let _env_guard = super::super::GeminiDataDirEnvGuard::set(fixture.root());
        let files = discover_log_files().unwrap();

        assert_eq!(
            files,
            vec![fixture.path("chats/a.json"), fixture.path("chats/b.jsonl")]
        );
    }
}
