use std::{
    collections::HashSet,
    env,
    path::{Component, Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

pub(super) const KIMI_DATA_DIR_ENV: &str = "KIMI_DATA_DIR";
pub(super) const KIMI_SESSIONS_DIR_NAME: &str = "sessions";
pub(super) const KIMI_WIRE_FILE_NAME: &str = "wire.jsonl";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(KIMI_DATA_DIR_ENV) {
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
        for dir in [".kimi", ".kimi-code"] {
            let path = home.join(dir);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

pub(super) fn discover_wire_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for kimi_path in paths()? {
        let sessions_path = kimi_path.join(KIMI_SESSIONS_DIR_NAME);
        let mut candidates = Vec::new();
        collect_files_with_extension(&sessions_path, "jsonl", &mut candidates);
        files.extend(
            candidates
                .into_iter()
                .filter(|file| is_kimi_wire_file(&sessions_path, file)),
        );
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn is_kimi_wire_file(sessions_path: &Path, file_path: &Path) -> bool {
    if file_path.file_name().and_then(|name| name.to_str()) != Some(KIMI_WIRE_FILE_NAME) {
        return false;
    }
    let Ok(relative) = file_path.strip_prefix(sessions_path) else {
        return false;
    };
    // Old format: sessions/<group>/<session>/wire.jsonl             → 3 components
    // New format: sessions/<workspace>/<session>/agents/<agent>/wire.jsonl → 5 components
    matches!(
        relative
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count(),
        3 | 5
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_wire_jsonl_files_under_sessions_group_session() {
        let fixture = fs_fixture!({
            "sessions/group/session/wire.jsonl": "{}\n",
            "sessions/group/session/other.jsonl": "{}\n",
            "sessions/nested/path/session/wire.jsonl": "{}\n",
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let files = discover_wire_files().unwrap();

        assert_eq!(
            files,
            vec![fixture.path("sessions/group/session/wire.jsonl")]
        );
    }

    #[test]
    fn discovers_both_old_and_new_layouts_and_skips_non_wire_files() {
        let fixture = fs_fixture!({
            "sessions/ws/session-c/agents/agent-1/wire.jsonl": "{}\n",
            "sessions/ws/session-c/agents/agent-1/other.jsonl": "{}\n",
            "sessions/group/session-d/wire.jsonl": "{}\n",
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let files = discover_wire_files().unwrap();

        assert_eq!(
            files,
            vec![
                fixture.path("sessions/group/session-d/wire.jsonl"),
                fixture.path("sessions/ws/session-c/agents/agent-1/wire.jsonl"),
            ]
        );
    }
}
