use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Result;

pub(super) const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";

/// Default Antigravity data roots under `~/.gemini`. The CLI, IDE, and backup
/// variants share the same `<root>/conversations/<uuid>.db` layout.
const DEFAULT_ROOT_NAMES: [&str; 4] = [
    "antigravity",
    "antigravity-cli",
    "antigravity-ide",
    "antigravity-backup",
];

/// Returns the Antigravity data roots that exist. `ANTIGRAVITY_DATA_DIR`
/// overrides the defaults with one root or a comma-separated list of roots.
pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
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
        return Ok(paths);
    }

    if let Some(home) = crate::home::home_dir() {
        for root_name in DEFAULT_ROOT_NAMES {
            let path = home.join(".gemini").join(root_name);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

/// Lists conversation databases under `<root>/conversations`. The `.db`
/// extension filter already skips `*.db-wal`/`*.db-shm` sidecars and the
/// legacy compressed/encrypted `.pb` conversation files, which are
/// unsupported.
pub(super) fn conversation_db_paths(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_with_extension(&root.join("conversations"), "db", &mut files);
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn env_override_accepts_comma_separated_roots() {
        let fixture = fs_fixture!({
            "first/conversations/a.db": "",
            "second/conversations/b.db": "",
        });
        let raw = format!(
            " {}, {}, {}",
            fixture.path("first").display(),
            fixture.path("missing").display(),
            fixture.path("second").display()
        );
        let _guard = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, &raw);

        let paths = paths().unwrap();

        assert_eq!(paths, vec![fixture.path("first"), fixture.path("second")]);
    }

    #[test]
    fn env_override_dedupes_roots() {
        let fixture = fs_fixture!({
            "root/conversations/a.db": "",
        });
        let raw = format!("{0},{0}", fixture.path("root").display());
        let _guard = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, &raw);

        let paths = paths().unwrap();

        assert_eq!(paths, vec![fixture.path("root")]);
    }

    #[test]
    fn discovers_conversation_dbs_and_skips_sidecars_and_pb_files() {
        let fixture = fs_fixture!({
            "conversations/a.db": "",
            "conversations/b.db": "",
            "conversations/b.db-wal": "",
            "conversations/b.db-shm": "",
            "conversations/legacy.pb": "",
            "conversations/notes.txt": "",
        });

        let files = conversation_db_paths(fixture.root());

        assert_eq!(
            files,
            vec![
                fixture.path("conversations/a.db"),
                fixture.path("conversations/b.db")
            ]
        );
    }
}
