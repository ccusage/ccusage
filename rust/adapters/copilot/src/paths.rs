use std::{collections::HashSet, env, fs, path::PathBuf};

use crate::Result;
use ccusage_adapter_common::{collect_files_with_extension, dir_entries};
use ccusage_core::{Diagnostics, Skip, SkipKind};

pub const COPILOT_OTEL_FILE_EXPORTER_PATH_ENV: &str = "COPILOT_OTEL_FILE_EXPORTER_PATH";
pub const COPILOT_HOME_ENV: &str = "COPILOT_HOME";

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum CopilotSourceKind {
    Otel,
    SessionState,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct CopilotSourceFile {
    pub(super) kind: CopilotSourceKind,
    pub(super) path: PathBuf,
}

pub(super) fn paths(diagnostics: &Diagnostics) -> Result<Vec<CopilotSourceFile>> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    if let Some(copilot_root) = copilot_root() {
        let otel_dir = copilot_root.join("otel");
        let mut otel_files = Vec::new();
        collect_files_with_extension(&otel_dir, "jsonl", &mut otel_files, diagnostics);
        for path in otel_files {
            add_file(&mut files, &mut seen, CopilotSourceKind::Otel, path);
        }
        let session_state_dir = copilot_root.join("session-state");
        for (session, file_type) in dir_entries(&session_state_dir, diagnostics) {
            if !file_type.is_dir() {
                continue;
            }
            let path = session.join("events.jsonl");
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    add_file(&mut files, &mut seen, CopilotSourceKind::SessionState, path)
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    record_discovery_failure(diagnostics, &path)
                }
                _ => {}
            }
        }
    }
    if let Some(path) = copilot_exporter_path() {
        add_file(&mut files, &mut seen, CopilotSourceKind::Otel, path);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn record_discovery_failure(diagnostics: &Diagnostics, path: &std::path::Path) {
    diagnostics.record(Skip::unit(
        path,
        SkipKind::Unreadable,
        "Copilot source directory could not be read",
    ));
}

fn copilot_root() -> Option<PathBuf> {
    env::var(COPILOT_HOME_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".copilot")))
}

fn add_file(
    files: &mut Vec<CopilotSourceFile>,
    seen: &mut HashSet<PathBuf>,
    kind: CopilotSourceKind,
    path: PathBuf,
) {
    if seen.insert(path.clone()) {
        files.push(CopilotSourceFile { kind, path });
    }
}

fn copilot_exporter_path() -> Option<PathBuf> {
    let path = env::var(COPILOT_OTEL_FILE_EXPORTER_PATH_ENV).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn diagnoses_inaccessible_sessions_and_keeps_healthy_siblings() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = fs_fixture!({
            "session-state/blocked/events.jsonl": "{}",
            "session-state/healthy/events.jsonl": "{}",
        });
        let _guard = EnvVarsGuard::set_many([
            (COPILOT_HOME_ENV, Some(OsString::from(fixture.root()))),
            (COPILOT_OTEL_FILE_EXPORTER_PATH_ENV, None),
        ]);
        let blocked = fixture.path("session-state/blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::metadata(blocked.join("events.jsonl")).is_err();
        let diagnostics = Diagnostics::new("copilot");
        let files = paths(&diagnostics);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        let files = files.unwrap();
        if !denied {
            return;
        }

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            fixture.path("session-state/healthy/events.jsonl")
        );
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn discovers_session_state_and_otel_files_without_nested_session_files() {
        let fixture = fs_fixture!({
            "home/.copilot/otel/otel.jsonl": "{}\n",
            "home/.copilot/session-state/session-a/events.jsonl": "{}\n",
            "home/.copilot/session-state/session-a/other.jsonl": "{}\n",
            "home/.copilot/session-state/session-b/events.jsonl": "{}\n",
            "home/.copilot/session-state/nested/session-c/events.jsonl": "{}\n",
            "explicit.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (COPILOT_HOME_ENV, None),
            (
                COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                Some(OsString::from(fixture.path("explicit.jsonl"))),
            ),
        ]);

        let files = paths(&Diagnostics::new("copilot")).unwrap();

        assert_eq!(
            files,
            vec![
                CopilotSourceFile {
                    kind: CopilotSourceKind::Otel,
                    path: fixture.path("explicit.jsonl"),
                },
                CopilotSourceFile {
                    kind: CopilotSourceKind::Otel,
                    path: fixture.path("home/.copilot/otel/otel.jsonl"),
                },
                CopilotSourceFile {
                    kind: CopilotSourceKind::SessionState,
                    path: fixture.path("home/.copilot/session-state/session-a/events.jsonl"),
                },
                CopilotSourceFile {
                    kind: CopilotSourceKind::SessionState,
                    path: fixture.path("home/.copilot/session-state/session-b/events.jsonl"),
                },
            ]
        );
    }

    #[test]
    fn discovers_files_from_copilot_home() {
        let fixture = fs_fixture!({
            "relocated/otel/trace.jsonl": "{}\n",
            "relocated/session-state/session-1/events.jsonl": "{}\n",
            "home/.copilot/otel/default.jsonl": "{}\n",
            "home/.copilot/session-state/session-2/events.jsonl": "{}\n"
        });
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (
                COPILOT_HOME_ENV,
                Some(OsString::from(fixture.path("relocated"))),
            ),
            (COPILOT_OTEL_FILE_EXPORTER_PATH_ENV, None),
        ]);

        let files = paths(&Diagnostics::new("copilot")).unwrap();

        assert_eq!(
            files,
            vec![
                CopilotSourceFile {
                    kind: CopilotSourceKind::Otel,
                    path: fixture.path("relocated/otel/trace.jsonl"),
                },
                CopilotSourceFile {
                    kind: CopilotSourceKind::SessionState,
                    path: fixture.path("relocated/session-state/session-1/events.jsonl"),
                },
            ]
        );
    }
}
