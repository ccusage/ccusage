use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
};

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

use crate::Result;

pub(super) const KIMI_DATA_DIR_ENV: &str = "KIMI_DATA_DIR";
const KIMI_SESSIONS_DIR_NAME: &str = "sessions";
const KIMI_WIRE_FILE_NAME: &str = "wire.jsonl";

fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(KIMI_DATA_DIR_ENV), || {
        crate::cli::home::home_dir()
            .into_iter()
            .flat_map(|home| [".kimi", ".kimi-code"].map(|path| home.join(path)))
            .collect()
    }))
}

pub(super) fn discover_wire_files(diagnostics: &Diagnostics) -> Result<Vec<WireFile>> {
    let mut files = Vec::new();
    for kimi_path in paths()? {
        let sessions_path = kimi_path.join(KIMI_SESSIONS_DIR_NAME);
        let mut candidates = Vec::new();
        collect_files_with_extension(&sessions_path, "jsonl", &mut candidates, diagnostics);
        files.extend(
            candidates
                .into_iter()
                .filter(|file| is_kimi_wire_file(&sessions_path, file)),
        );
    }
    files.sort();
    files.dedup();
    Ok(WireFile::from_paths(files))
}

// NOTE(2026-07-09, #1362): Kimi CLI writes `sessions/<group>/<session>/wire.jsonl`. Kimi Code adds an `agents/<agent>` level under the session, so discovery accepts both depths.
fn is_kimi_wire_file(sessions_path: &Path, file_path: &Path) -> bool {
    file_path
        .file_name()
        .is_some_and(|name| name == KIMI_WIRE_FILE_NAME)
        && file_path.strip_prefix(sessions_path).is_ok_and(|relative| {
            matches!(
                relative
                    .components()
                    .filter(|component| matches!(component, Component::Normal(_)))
                    .count(),
                3 | 5
            )
        })
}

#[derive(Debug)]
pub(super) struct WireConfig {
    pub(super) root: Option<PathBuf>,
    pub(super) model: OnceLock<String>,
}

#[derive(Debug)]
pub(super) struct WireFile {
    path: PathBuf,
    pub(super) session_id: String,
    pub(super) config: Arc<WireConfig>,
}

impl AsRef<Path> for WireFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl WireFile {
    pub(super) fn from_paths(paths: Vec<PathBuf>) -> Vec<Self> {
        let mut configs = HashMap::new();
        paths
            .into_iter()
            .filter_map(|path| {
                if path
                    .file_name()
                    .is_none_or(|name| name != KIMI_WIRE_FILE_NAME)
                {
                    return None;
                }
                let parent = path.parent();
                let session = if parent
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    == Some("agents")
                {
                    parent.and_then(Path::parent).and_then(Path::parent)
                } else {
                    parent
                };
                let root = session
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .map(Path::to_path_buf);
                let session_id = session
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("unknown")
                    .to_string();
                let config = Arc::clone(configs.entry(root.clone()).or_insert_with(|| {
                    Arc::new(WireConfig {
                        root,
                        model: OnceLock::new(),
                    })
                }));
                Some(Self {
                    path,
                    session_id,
                    config,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_roots_ignore_unrelated_sessions_ancestors() {
        let fixture = fs_fixture!({
            "backup/sessions/kimi/sessions/wire.jsonl": "{}\n",
            "backup/sessions/kimi/sessions/group/session/wire.jsonl": "{}\n",
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.path("backup/sessions/kimi")).run(|| {
            let files = discover_wire_files(&Diagnostics::new("kimi")).unwrap();
            assert_eq!(
                files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>(),
                vec![fixture.path("backup/sessions/kimi/sessions/group/session/wire.jsonl")]
            );
        })
    }
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn discovers_wire_jsonl_files_under_sessions_group_session() {
        let fixture = fs_fixture!({
            "sessions/group/session/wire.jsonl": "{}\n",
            "sessions/group/session/other.jsonl": "{}\n",
            "sessions/nested/path/session/wire.jsonl": "{}\n",
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let files = discover_wire_files(&Diagnostics::new("kimi"))
                .unwrap()
                .into_iter()
                .map(|file| file.path)
                .collect::<Vec<_>>();

            assert_eq!(
                files,
                vec![fixture.path("sessions/group/session/wire.jsonl")]
            );
        })
    }

    #[test]
    fn discovers_both_old_and_new_layouts_and_skips_non_wire_files() {
        let fixture = fs_fixture!({
            "sessions/ws/session-c/agents/agent-1/wire.jsonl": "{}\n",
            "sessions/ws/session-c/agents/agent-1/other.jsonl": "{}\n",
            "sessions/group/session-d/wire.jsonl": "{}\n",
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let files = discover_wire_files(&Diagnostics::new("kimi"))
                .unwrap()
                .into_iter()
                .map(|file| file.path)
                .collect::<Vec<_>>();

            assert_eq!(
                files,
                vec![
                    fixture.path("sessions/group/session-d/wire.jsonl"),
                    fixture.path("sessions/ws/session-c/agents/agent-1/wire.jsonl"),
                ]
            );
        })
    }
}
