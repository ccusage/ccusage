use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

pub(super) const VIBE_DATA_DIR_ENV: &str = "VIBE_DATA_DIR";

/// Returns the default paths where Mistral Vibe stores session data
pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    // Check for custom environment variable
    if let Ok(env_paths) = env::var(VIBE_DATA_DIR_ENV) {
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
        if !paths.is_empty() {
            return Ok(paths);
        }
    }

    // Default path: ~/.vibe/logs/session
    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".vibe").join("logs").join("session");
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }

    Ok(paths)
}

/// Discover all session directories containing meta.json files
pub(super) fn discover_session_dirs() -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for path in paths()? {
        if !path.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if entry_path.is_dir() {
                // Check if this directory contains a meta.json file
                let meta_path = entry_path.join("meta.json");
                if meta_path.is_file() {
                    dirs.push(entry_path);
                }
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Extract session ID from session directory name
/// Session directories are named like: session_20260615_172447_ea8f636f
/// The session ID is the last component (ea8f636f)
pub(super) fn extract_session_id_from_path(path: &std::path::Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            if let Some(last_underscore) = name.rfind('_') {
                let id = &name[last_underscore + 1..];
                if !id.is_empty() {
                    Some(id.to_string())
                } else {
                    None
                }
            } else {
                Some(name.to_string())
            }
        })
}

/// Get the timestamp from session directory name
/// Session directories are named like: session_20260615_172447_ea8f636f
/// The timestamp is: 20260615_172447 (date and time)
pub(super) fn extract_timestamp_from_path(path: &std::path::Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            // Remove "session_" prefix if present
            let trimmed = name.strip_prefix("session_").unwrap_or(name);
            // Find the last underscore before the session ID
            if let Some(last_underscore) = trimmed.rfind('_') {
                let timestamp_part = &trimmed[..last_underscore];
                // Format: YYYYMMDD_HHMMSS
                if timestamp_part.len() >= 15 {
                    // Try to parse as 20260615_172447
                    Some(format!("{}T{}:00Z", &timestamp_part[..8], &timestamp_part[9..15]))
                } else {
                    None
                }
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn discovers_session_directories() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": "{}",
            "session_20260615_172500_def456/meta.json": "{}",
            "ignore_dir/": {},
        });
        let _env_guard = super::super::VibeDataDirEnvGuard::set(fixture.root());
        let dirs = discover_session_dirs().unwrap();

        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].to_string_lossy().contains("session_20260615_172447_abc123"));
        assert!(dirs[1].to_string_lossy().contains("session_20260615_172500_def456"));
    }

    #[test]
    fn extracts_session_id_from_directory_name() {
        let path = std::path::Path::new("session_20260615_172447_abc123");
        assert_eq!(extract_session_id_from_path(path), Some("abc123".to_string()));
    }

    #[test]
    fn extracts_timestamp_from_directory_name() {
        let path = std::path::Path::new("session_20260615_172447_abc123");
        assert_eq!(
            extract_timestamp_from_path(path),
            Some("20260615T17:24:47Z".to_string())
        );
    }
}
