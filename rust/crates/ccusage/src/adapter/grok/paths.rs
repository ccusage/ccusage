use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::Result;

pub(super) const GROK_HOME_ENV: &str = "GROK_HOME";
const UPDATES_FILE_NAME: &str = "updates.jsonl";
const SUMMARY_FILE_NAME: &str = "summary.json";

/// Resolve Grok home roots: custom path, else `GROK_HOME`, else `~/.grok`.
/// Only existing directories are returned; comma-separated lists are supported.
pub(super) fn paths(custom_path: Option<&str>) -> Vec<PathBuf> {
    if let Some(custom_path) = custom_path.filter(|path| !path.trim().is_empty()) {
        return existing_path_list(custom_path);
    }
    if let Ok(env_paths) = env::var(GROK_HOME_ENV)
        && !env_paths.trim().is_empty()
    {
        return existing_path_list(&env_paths);
    }
    let Some(home) = crate::home::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".grok");
    path.is_dir().then_some(path).into_iter().collect()
}

fn existing_path_list(raw: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter_map(|path| {
            let canonical = path.canonicalize().unwrap_or(path);
            (canonical.is_dir() && seen.insert(canonical.clone())).then_some(canonical)
        })
        .collect()
}

/// Session file pair: required `updates.jsonl` plus optional sibling `summary.json`.
#[derive(Debug, Clone)]
pub(super) struct GrokSessionFiles {
    pub(super) updates: PathBuf,
    pub(super) summary: Option<PathBuf>,
}

/// Discover `sessions/**/updates.jsonl` under each root (skipping symlinks).
pub(super) fn collect_session_files(root: &Path) -> Result<Vec<GrokSessionFiles>> {
    let mut files = Vec::new();
    let sessions = root.join("sessions");
    if sessions.is_dir() {
        collect_session_files_inner(&sessions, &mut files)?;
    } else {
        // Allow custom path that already points at a sessions tree or a single session dir.
        collect_session_files_inner(root, &mut files)?;
    }
    files.sort_by(|left, right| left.updates.cmp(&right.updates));
    Ok(files)
}

fn collect_session_files_inner(path: &Path, files: &mut Vec<GrokSessionFiles>) -> Result<()> {
    let Ok(entries) = fs::read_dir(path) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_session_files_inner(&path, files)?;
            continue;
        }
        if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == UPDATES_FILE_NAME)
        {
            let summary = path
                .parent()
                .map(|parent| parent.join(SUMMARY_FILE_NAME))
                .filter(|candidate| candidate.is_file());
            files.push(GrokSessionFiles {
                updates: path,
                summary,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    static GROK_HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn discovers_nested_updates_under_grok_home() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": "",
            "sessions/proj/sess-1/summary.json": "{}",
        });
        let _cleanup = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());

        let roots = paths(None);
        assert_eq!(roots.len(), 1);
        let files = collect_session_files(&roots[0]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].updates.ends_with("updates.jsonl"));
        assert!(files[0].summary.is_some());
    }

    #[test]
    fn missing_home_or_empty_sessions_returns_empty() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "other/file.txt": "x",
        });
        let _cleanup = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());

        let roots = paths(None);
        assert_eq!(roots.len(), 1);
        let files = collect_session_files(&roots[0]).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn custom_path_override_ignores_env() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let env_fixture = fs_fixture!({
            "sessions/a/s1/updates.jsonl": "",
        });
        let custom_fixture = fs_fixture!({
            "sessions/b/s2/updates.jsonl": "",
        });
        let _cleanup = EnvVarGuard::set(GROK_HOME_ENV, env_fixture.root());

        let roots = paths(custom_fixture.root().to_str());
        assert_eq!(roots.len(), 1);
        let files = collect_session_files(&roots[0]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].updates.to_string_lossy().contains("s2"));
    }

    #[test]
    fn comma_separated_roots_are_scanned_and_deduped() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let fixture_a = fs_fixture!({
            "sessions/p/s/updates.jsonl": "",
        });
        let fixture_b = fs_fixture!({
            "sessions/q/t/updates.jsonl": "",
        });
        let a = fixture_a.root().display().to_string();
        let b = fixture_b.root().display().to_string();
        // Same path twice should yield a single root after canonical dedupe.
        let list = format!("{a},{b},{a}");
        let roots = paths(Some(&list));
        assert_eq!(roots.len(), 2);
        let mut total = 0;
        for root in &roots {
            total += collect_session_files(root).unwrap().len();
        }
        assert_eq!(total, 2);
    }

    #[test]
    fn empty_custom_path_falls_back_to_env() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "sessions/p/s/updates.jsonl": "x",
        });
        let _cleanup = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());
        let roots = paths(Some("   "));
        assert_eq!(roots.len(), 1);
    }
}
