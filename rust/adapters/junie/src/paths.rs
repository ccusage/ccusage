use std::{env, fs, io::ErrorKind, path::PathBuf};

use crate::{cli::SharedArgs, debug_log};

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
/// Entries are judged by `file_type()`/`symlink_metadata`, as the shared
/// collectors do, so a symlink to another session is not counted twice.
pub(super) fn discover_events_files(shared: &SharedArgs) -> Vec<PathBuf> {
    let Some(sessions) = sessions_dir() else {
        return Vec::new();
    };
    let entries = match fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != ErrorKind::NotFound {
                debug_log(
                    shared,
                    format!(
                        "Failed to read Junie sessions directory {}: {error}",
                        sessions.display()
                    ),
                );
            }
            return Vec::new();
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let session_dir = match entry.and_then(|entry| {
            let file_type = entry.file_type()?;
            Ok((entry.path(), file_type))
        }) {
            Ok((path, file_type)) if file_type.is_dir() => path,
            Ok(_) => continue,
            Err(error) => {
                debug_log(
                    shared,
                    format!(
                        "Failed to read a Junie session entry in {}: {error}",
                        sessions.display()
                    ),
                );
                continue;
            }
        };
        let events = session_dir.join("events.jsonl");
        match fs::symlink_metadata(&events) {
            Ok(metadata) if metadata.file_type().is_file() => files.push(events),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => debug_log(
                shared,
                format!(
                    "Failed to inspect Junie events file {}: {error}",
                    events.display()
                ),
            ),
        }
    }
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
            discover_events_files(&SharedArgs::default()),
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
            discover_events_files(&SharedArgs::default()),
            vec![fixture.path("home/.junie/sessions/session-a/events.jsonl")]
        );
    }

    #[test]
    fn missing_sessions_directory_yields_no_files() {
        let fixture = fs_fixture!({});
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        assert!(discover_events_files(&SharedArgs::default()).is_empty());
    }

    #[test]
    fn unreadable_sessions_path_yields_no_files() {
        let fixture = fs_fixture!({
            "sessions": "not a directory\n",
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        assert!(discover_events_files(&SharedArgs::default()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sessions_and_event_files_are_not_counted() {
        let fixture = fs_fixture!({
            "sessions/session-a/events.jsonl": "{}\n",
            "sessions/session-b/state.json": "{}\n",
            "elsewhere/events.jsonl": "{}\n",
        });
        std::os::unix::fs::symlink(
            fixture.path("sessions/session-a"),
            fixture.path("sessions/session-a-link"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            fixture.path("elsewhere/events.jsonl"),
            fixture.path("sessions/session-b/events.jsonl"),
        )
        .unwrap();
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        assert_eq!(
            discover_events_files(&SharedArgs::default()),
            vec![fixture.path("sessions/session-a/events.jsonl")]
        );
    }
}
