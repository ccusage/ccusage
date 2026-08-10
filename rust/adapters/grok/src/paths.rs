use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

/// Official Grok CLI home override (single root).
pub(crate) const GROK_HOME_ENV: &str = "GROK_HOME";

#[derive(Debug, Clone)]
pub(super) struct GrokSessionFiles {
    pub updates: PathBuf,
    pub summary: Option<PathBuf>,
}

/// Resolve the Grok data root from `GROK_HOME`, then `~/.grok`.
fn resolve_root() -> Option<PathBuf> {
    if let Ok(home) = env::var(GROK_HOME_ENV)
        && !home.trim().is_empty()
    {
        let path = PathBuf::from(home.trim());
        return path.is_dir().then_some(path);
    }

    let home = crate::home::home_dir()?;
    let path = home.join(".grok");
    path.is_dir().then_some(path)
}

/// Discover every `sessions/**/updates.jsonl` under the resolved root.
pub(super) fn discover_session_files() -> Result<Vec<GrokSessionFiles>> {
    let mut files = Vec::new();
    if let Some(root) = resolve_root() {
        let sessions = root.join("sessions");
        if sessions.is_dir() {
            let mut updates = Vec::new();
            collect_files_with_extension(&sessions, "jsonl", &mut updates);
            updates.retain(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some("updates.jsonl")
            });
            for updates_path in updates {
                let summary = sibling_summary(&updates_path);
                files.push(GrokSessionFiles {
                    updates: updates_path,
                    summary,
                });
            }
        }
    }
    files.sort_by(|a, b| a.updates.cmp(&b.updates));
    Ok(files)
}

fn sibling_summary(updates: &Path) -> Option<PathBuf> {
    let summary = updates.with_file_name("summary.json");
    summary.is_file().then_some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
    use std::ffi::OsString;

    #[test]
    fn discovers_updates_jsonl_under_sessions() {
        let fixture = fs_fixture!({
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/updates.jsonl": "{}\n",
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/summary.json": "{}",
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/events.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);
        let files = discover_session_files().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].updates.ends_with("updates.jsonl"));
        assert!(files[0].summary.is_some());
        assert!(
            files[0]
                .summary
                .as_ref()
                .is_some_and(|path| path.ends_with("summary.json"))
        );
    }

    #[test]
    fn ignores_jsonl_files_that_are_not_named_updates() {
        let fixture = fs_fixture!({
            "sessions/proj/sess-a/updates.jsonl": "{}\n",
            "sessions/proj/sess-a/events.jsonl": "{}\n",
            "sessions/proj/sess-a/notes.txt": "nope\n",
            "logs/unified.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);

        let files = discover_session_files().unwrap();
        let names: Vec<_> = files
            .iter()
            .map(|file| {
                file.updates
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["updates.jsonl".to_string()]);
    }

    #[test]
    fn discovers_nested_session_trees_under_multiple_projects() {
        let fixture = fs_fixture!({
            "sessions/proj-a/sess-1/updates.jsonl": "{}\n",
            "sessions/proj-b/sess-2/updates.jsonl": "{}\n",
            "sessions/proj-b/sess-2/summary.json": "{}",
        });
        let _guard = EnvVarsGuard::set_many([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);

        let files = discover_session_files().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|file| file.summary.is_some()));
        assert!(files.iter().any(|file| file.summary.is_none()));
        // Discovery sorts by path so load order is stable across runs.
        assert!(files[0].updates < files[1].updates);
    }

    #[test]
    fn grok_home_is_used() {
        let fixture = fs_fixture!({
            "sessions/proj/session-a/updates.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);

        let files = discover_session_files().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].updates.ends_with("updates.jsonl"));
    }

    #[test]
    fn empty_grok_home_falls_back_to_default_home() {
        let fixture = fs_fixture!({
            ".grok/sessions/home/session-a/updates.jsonl": "{}\n",
        });
        let home = OsString::from(fixture.root().as_os_str());
        // Pin HOME/USERPROFILE alongside the blank override so the fallback root
        // is the fixture rather than whatever home the process inherits. Both go
        // through one guard because each guard holds the same global env lock.
        let _guard = EnvVarsGuard::set_many([
            (GROK_HOME_ENV, Some(OsString::from("  "))),
            ("HOME", Some(home.clone())),
            ("USERPROFILE", Some(home)),
        ]);

        let files = discover_session_files().unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].updates.to_string_lossy().contains("session-a"));
    }

    #[test]
    fn missing_roots_yield_empty_discovery() {
        let fixture = fs_fixture!({});
        let missing = fixture.path("does-not-exist");
        let _guard =
            EnvVarsGuard::set_many([(GROK_HOME_ENV, Some(OsString::from(missing.as_os_str())))]);
        let files = discover_session_files().unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn non_directory_grok_home_yields_empty_discovery() {
        let fixture = fs_fixture!({
            "not-a-dir": "file",
        });
        let file_path = fixture.path("not-a-dir");
        let _guard =
            EnvVarsGuard::set_many([(GROK_HOME_ENV, Some(OsString::from(file_path.as_os_str())))]);
        let files = discover_session_files().unwrap();
        assert!(files.is_empty());
    }
}
