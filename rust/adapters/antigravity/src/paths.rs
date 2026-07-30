use std::{collections::HashSet, env, path::PathBuf};

use ccusage_adapter_common::collect_files_with_extension;

use crate::Result;

const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";
const ANTIGRAVITY_CONVERSATIONS_DIR_NAME: &str = "conversations";

/// Roots to search for Antigravity CLI data, honoring `ANTIGRAVITY_DATA_DIR`.
///
/// The override accepts comma-separated directories, like every other
/// source-specific path variable, so a current profile and an archive can be
/// reported together.
fn antigravity_roots() -> Result<Vec<PathBuf>> {
    if let Ok(env_roots) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
        let roots = env_roots
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            return Ok(roots);
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
    use std::ffi::OsString;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

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
    fn reads_comma_separated_data_dirs() {
        let current = fs_fixture!({ "conversations/current.db": "" });
        let archive = fs_fixture!({ "conversations/archive.db": "" });
        let _cleanup = EnvVarGuard::set(
            ANTIGRAVITY_DATA_DIR_ENV,
            format!("{}, {}", current.root().display(), archive.root().display()),
        );

        let names = antigravity_db_paths()
            .unwrap()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["current.db", "archive.db"]);
    }

    #[test]
    fn ignores_a_blank_data_dir_override() {
        let fixture = fs_fixture!({});
        // Pin `HOME` alongside the override so the fallback root is the fixture
        // rather than whatever home directory the test process inherits. Both go
        // through one guard because each guard holds the same global env lock.
        let _cleanup = EnvVarsGuard::set_many([
            (ANTIGRAVITY_DATA_DIR_ENV, Some(OsString::from("   "))),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
        ]);

        // Falls back to the default root, which holds no databases under test.
        assert!(antigravity_db_paths().unwrap().is_empty());
    }
}
