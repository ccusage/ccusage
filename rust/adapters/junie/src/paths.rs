use std::{env, path::PathBuf};

pub(super) const JUNIE_HOME_ENV: &str = "JUNIE_HOME";

fn junie_home() -> Option<PathBuf> {
    if let Some(home) = env::var_os(JUNIE_HOME_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    crate::home::home_dir().map(|home| home.join(".junie"))
}

fn sessions_dir() -> Option<PathBuf> {
    junie_home().map(|home| home.join("sessions"))
}

pub(super) fn session_index_path() -> Option<PathBuf> {
    sessions_dir().map(|dir| dir.join("index.jsonl"))
}

/// Discovers `sessions/<session-id>/events.jsonl` files.
///
/// Only direct children of `sessions/` are sessions: an `events.jsonl` deeper
/// in the tree belongs to some other run and must not be counted here.
pub(super) fn discover_events_files() -> Vec<PathBuf> {
    let Some(sessions) = sessions_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&sessions) else {
        return Vec::new();
    };
    let mut files = entries
        .flatten()
        .map(|entry| entry.path().join("events.jsonl"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    #[test]
    fn discovers_events_in_direct_session_directories_only() {
        let fixture = fs_fixture!({
            "sessions/session-b/events.jsonl": "{}\n",
            "sessions/session-a/events.jsonl": "{}\n",
            "sessions/session-c/nested/events.jsonl": "{}\n",
            "sessions/session-d/state.json": "{}\n",
            "sessions/index.jsonl": "{}\n",
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        assert_eq!(
            discover_events_files(),
            vec![
                fixture.path("sessions/session-a/events.jsonl"),
                fixture.path("sessions/session-b/events.jsonl"),
            ]
        );
        assert_eq!(
            session_index_path(),
            Some(fixture.path("sessions/index.jsonl"))
        );
    }

    #[test]
    fn defaults_to_dot_junie_under_home() {
        let fixture = fs_fixture!({
            "home/.junie/sessions/session-a/events.jsonl": "{}\n",
        });
        let _env = EnvVarsGuard::set_many([
            (JUNIE_HOME_ENV, None),
            ("HOME", Some(fixture.path("home").into_os_string())),
            ("USERPROFILE", Some(fixture.path("home").into_os_string())),
        ]);

        assert_eq!(
            discover_events_files(),
            vec![fixture.path("home/.junie/sessions/session-a/events.jsonl")]
        );
    }

    #[test]
    fn missing_sessions_directory_yields_no_files() {
        let fixture = fs_fixture!({});
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        assert!(discover_events_files().is_empty());
    }
}
