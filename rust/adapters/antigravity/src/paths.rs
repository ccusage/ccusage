use std::{collections::HashSet, env, path::PathBuf};

use ccusage_adapter_common::collect_files_with_extension;

use crate::Result;

const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";
const ANTIGRAVITY_CONVERSATIONS_DIR_NAME: &str = "conversations";

/// Roots to search for Antigravity CLI data, honoring `ANTIGRAVITY_DATA_DIR`.
fn antigravity_roots() -> Result<Vec<PathBuf>> {
    if let Ok(root) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
        let root = root.trim();
        if !root.is_empty() {
            return Ok(vec![PathBuf::from(root)]);
        }
    }
    let home = ccusage_core::home::home_dir()
        .ok_or_else(|| crate::cli_error("home directory is not set"))?;
    // Antigravity ships as a Gemini-family tool, so its CLI state lives under the
    // shared `.gemini` directory rather than a directory of its own.
    Ok(vec![home.join(".gemini").join("antigravity-cli")])
}

/// Every per-conversation SQLite database Antigravity has written.
///
/// Antigravity stores one database per conversation under `conversations/`, and
/// sub-conversations (subagents, and the arms of a model comparison) get their own
/// database rather than being folded into their parent. Collecting all of them is
/// what makes those calls visible; the loader's dedup by response id is what keeps
/// a call recorded in two places from being counted twice.
pub(super) fn antigravity_db_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for root in antigravity_roots()? {
        let mut found = Vec::new();
        collect_files_with_extension(
            &root.join(ANTIGRAVITY_CONVERSATIONS_DIR_NAME),
            "db",
            &mut found,
        );
        // Read order decides which duplicate survives, so keep it stable across
        // runs rather than inheriting the filesystem's directory order.
        found.sort();
        for path in found {
            let path = path.canonicalize().unwrap_or(path);
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_conversation_databases_under_the_data_dir() {
        let fixture = fs_fixture!({
            "conversations/adf2fd49.db": "",
            "conversations/f672f900.db": "",
            "conversation_summaries.db": "",
            "conversations/notes.txt": "",
        });
        let _cleanup = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, fixture.root());

        let paths = antigravity_db_paths().unwrap();

        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        // The summaries database sits outside `conversations/` and holds no usage.
        assert_eq!(names, vec!["adf2fd49.db", "f672f900.db"]);
    }

    #[test]
    fn ignores_a_blank_data_dir_override() {
        let _cleanup = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, "   ");

        // Falls back to the default root, which holds no databases under test.
        assert!(antigravity_db_paths().is_ok());
    }
}
