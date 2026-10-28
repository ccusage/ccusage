use std::{
    collections::HashSet,
    env,
    path::{Component, Path, PathBuf},
};

use crate::Result;

pub(super) const CLINE_DATA_DIR_ENV: &str = "CLINE_DATA_DIR";
const CLINE_EXTENSION_ID: &str = "saoudrizwan.claude-dev";
const EDITORS: [&str; 5] = ["Code", "Code - Insiders", "Cursor", "Windsurf", "VSCodium"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceKind {
    Extension,
    Cli,
    CliTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceFile {
    pub path: PathBuf,
    pub kind: SourceKind,
    pub session_id: String,
}

pub(super) fn discover_files() -> Result<Vec<SourceFile>> {
    let roots = data_roots()?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        let mut candidates = Vec::new();
        for relative_root in ["tasks", "data/tasks", "data/sessions", "sessions"] {
            ccusage_adapter_common::collect_files_matching(
                &root.join(relative_root),
                &|path| {
                    path.file_name().is_some_and(|name| {
                        name == "ui_messages.json"
                            || name
                                .to_str()
                                .is_some_and(|name| name.ends_with(".messages.json"))
                    })
                },
                &mut candidates,
            );
        }
        candidates.sort();

        for path in candidates {
            let Some((kind, session_id)) = classify_file(&root, &path) else {
                continue;
            };
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
            if seen.insert(identity) {
                files.push(SourceFile {
                    path,
                    kind,
                    session_id,
                });
            }
        }
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn data_roots() -> Result<Vec<PathBuf>> {
    let candidates = if let Ok(configured_roots) = env::var(CLINE_DATA_DIR_ENV) {
        configured_roots
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .collect()
    } else {
        default_roots()
    };

    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for path in candidates {
        if !path.is_dir() {
            continue;
        }
        let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(identity) {
            roots.push(path);
        }
    }
    Ok(roots)
}

fn default_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = crate::home::home_dir() {
        roots.push(home.join(".cline"));
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        for editor in EDITORS {
            roots.push(
                home.join("Library/Application Support")
                    .join(editor)
                    .join("User/globalStorage")
                    .join(CLINE_EXTENSION_ID),
            );
            roots.push(
                config_home
                    .join(editor)
                    .join("User/globalStorage")
                    .join(CLINE_EXTENSION_ID),
            );
        }
    }

    let app_data = env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .or_else(|| crate::home::home_dir().map(PathBuf::into_os_string))
                .map(PathBuf::from)
                .map(|home| home.join("AppData/Roaming"))
        });
    if let Some(app_data) = app_data {
        for editor in EDITORS {
            roots.push(
                app_data
                    .join(editor)
                    .join("User/globalStorage")
                    .join(CLINE_EXTENSION_ID),
            );
        }
    }
    roots
}

fn classify_file(root: &Path, path: &Path) -> Option<(SourceKind, String)> {
    let components = path
        .strip_prefix(root)
        .ok()?
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();

    match components.as_slice() {
        ["tasks", session_id, "ui_messages.json"] if !session_id.is_empty() => {
            Some((SourceKind::Extension, (*session_id).to_string()))
        }
        ["data", "tasks", session_id, "ui_messages.json"] if !session_id.is_empty() => {
            Some((SourceKind::CliTask, (*session_id).to_string()))
        }
        ["data", "sessions", session_id, filename] if is_cli_session_file(session_id, filename) => {
            Some((SourceKind::Cli, (*session_id).to_string()))
        }
        ["sessions", session_id, filename] if is_cli_session_file(session_id, filename) => {
            Some((SourceKind::Cli, (*session_id).to_string()))
        }
        _ => None,
    }
}

fn is_cli_session_file(session_id: &str, filename: &str) -> bool {
    !session_id.is_empty() && filename == format!("{session_id}.messages.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_extension_cli_task_and_both_cli_session_layouts_from_configured_roots() {
        let extension = fs_fixture!({
            "tasks/task-a/ui_messages.json": "[]",
            "tasks/nested/task-b/ui_messages.json": "[]",
            "tasks/task-c/other.json": "{}",
        });
        let cli_root = fs_fixture!({
            "data/sessions/cli-a/cli-a.messages.json": "{}",
            "data/sessions/cli-b/another.messages.json": "{}",
        });
        let cli_data = fs_fixture!({
            "sessions/cli-c/cli-c.messages.json": "{}",
        });
        let legacy_cli_tasks = fs_fixture!({
            "data/tasks/cli-task-a/ui_messages.json": "[]",
        });
        let configured = format!(
            "{},{},{},{}",
            extension.root().display(),
            cli_root.root().display(),
            cli_data.root().display(),
            legacy_cli_tasks.root().display()
        );
        let _cleanup = EnvVarGuard::set(CLINE_DATA_DIR_ENV, configured);

        let files = discover_files().unwrap();

        let mut expected = vec![
            SourceFile {
                path: cli_root.path("data/sessions/cli-a/cli-a.messages.json"),
                kind: SourceKind::Cli,
                session_id: "cli-a".to_string(),
            },
            SourceFile {
                path: cli_data.path("sessions/cli-c/cli-c.messages.json"),
                kind: SourceKind::Cli,
                session_id: "cli-c".to_string(),
            },
            SourceFile {
                path: legacy_cli_tasks.path("data/tasks/cli-task-a/ui_messages.json"),
                kind: SourceKind::CliTask,
                session_id: "cli-task-a".to_string(),
            },
            SourceFile {
                path: extension.path("tasks/task-a/ui_messages.json"),
                kind: SourceKind::Extension,
                session_id: "task-a".to_string(),
            },
        ];
        expected.sort_by(|left, right| left.path.cmp(&right.path));
        assert_eq!(files, expected);
    }

    #[test]
    fn configured_roots_replace_defaults_and_alias_roots_are_deduplicated() {
        let fixture = fs_fixture!({
            "tasks/task-a/ui_messages.json": "[]",
        });
        let configured = format!("{},{}", fixture.root().display(), fixture.root().display());
        let _cleanup = EnvVarGuard::set(CLINE_DATA_DIR_ENV, configured);

        let files = discover_files().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].session_id, "task-a");
    }

    #[test]
    fn empty_configured_roots_disable_default_discovery() {
        let _cleanup = EnvVarGuard::set(CLINE_DATA_DIR_ENV, " , ");

        assert!(discover_files().unwrap().is_empty());
    }

    #[test]
    fn default_roots_cover_supported_editors_and_cline_cli() {
        let _env = ccusage_test_support::EnvVarsGuard::set_many([
            ("HOME", Some("/tmp/cline-home".into())),
            ("APPDATA", Some("/tmp/cline-appdata".into())),
            ("XDG_CONFIG_HOME", Some("/tmp/cline-config".into())),
            ("USERPROFILE", None),
        ]);

        let roots = default_roots();

        for editor in EDITORS {
            assert!(
                roots.contains(
                    &PathBuf::from("/tmp/cline-home/Library/Application Support")
                        .join(editor)
                        .join("User/globalStorage")
                        .join(CLINE_EXTENSION_ID)
                )
            );
            assert!(
                roots.contains(
                    &PathBuf::from("/tmp/cline-config")
                        .join(editor)
                        .join("User/globalStorage")
                        .join(CLINE_EXTENSION_ID)
                )
            );
            assert!(
                roots.contains(
                    &PathBuf::from("/tmp/cline-appdata")
                        .join(editor)
                        .join("User/globalStorage")
                        .join(CLINE_EXTENSION_ID)
                )
            );
        }
        assert!(roots.contains(&PathBuf::from("/tmp/cline-home/.cline")));
    }

    #[test]
    fn uses_windows_roaming_storage_when_appdata_is_unset() {
        let _env = ccusage_test_support::EnvVarsGuard::set_many([
            ("HOME", Some("/tmp/cline-home".into())),
            ("USERPROFILE", Some("/tmp/cline-profile".into())),
            ("APPDATA", None),
            ("XDG_CONFIG_HOME", None),
        ]);

        let roots = default_roots();

        assert!(
            roots.contains(
                &PathBuf::from("/tmp/cline-profile/AppData/Roaming/Code/User/globalStorage")
                    .join(CLINE_EXTENSION_ID)
            )
        );
    }
}
