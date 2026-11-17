use std::{collections::HashSet, env, path::PathBuf};

use ccusage_adapter_common::collect_files_matching;
use ccusage_core::Diagnostics;

use crate::Result;

pub(super) const GEMINI_DATA_DIR_ENV: &str = "GEMINI_DATA_DIR";
/// Returns all discovery candidate directories for Gemini CLI logs.
fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    if let Ok(env_paths) = env::var(GEMINI_DATA_DIR_ENV) {
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
        let path = home.join(".gemini").join("tmp");
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Discovers all `.json` and `.jsonl` log files across known directories.
pub(super) fn discover_log_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let is_log = |path: &std::path::Path| {
        path.extension()
            .is_some_and(|extension| extension == "json" || extension == "jsonl")
    };
    for path in paths()? {
        collect_files_matching(&path, &is_log, &mut files, diagnostics);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    #[test]
    fn discovers_json_and_jsonl_logs() {
        let fixture = fs_fixture!({
            "chats/a.json": "{}",
            "chats/b.jsonl": "{}\n",
            "chats/ignore.txt": "no",
        });
        let _env_guard = ccusage_test_support::EnvVarGuard::set(
            crate::paths::GEMINI_DATA_DIR_ENV,
            fixture.root(),
        );
        let files = discover_log_files(&Diagnostics::new("gemini")).unwrap();

        assert_eq!(
            files,
            vec![fixture.path("chats/a.json"), fixture.path("chats/b.jsonl")]
        );
    }

    #[test]
    fn antigravity_override_does_not_replace_gemini_discovery() {
        let fixture = fs_fixture!({
            "gemini/chats/a.json": "{}",
            "antigravity/conversations/a.db": "not a Gemini log",
        });
        let _guard = EnvVarsGuard::set_many([
            (
                GEMINI_DATA_DIR_ENV,
                Some(OsString::from(fixture.path("gemini"))),
            ),
            (
                "ANTIGRAVITY_DATA_DIR",
                Some(OsString::from(fixture.path("antigravity"))),
            ),
        ]);

        let files = discover_log_files(&Diagnostics::new("gemini")).unwrap();

        assert_eq!(files, vec![fixture.path("gemini/chats/a.json")]);
    }
}
