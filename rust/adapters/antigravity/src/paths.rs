use std::{collections::HashSet, env, path::PathBuf};

use ccusage_adapter_common::collect_files_with_extension;

use crate::Result;

const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";
const ANTIGRAVITY_CONVERSATIONS_DIR_NAME: &str = "conversations";
const ANTIGRAVITY_BRAIN_DIR_NAME: &str = "brain";

const EXCLUDED_DB_FILENAMES: &[&str] = &[
    "conversation_summaries.db",
    "heavy_ad_intervention_opt_out.db",
    "state.vscdb",
    "state.vscdb.backup",
];

/// Roots to search for Antigravity CLI / IDE / Desktop data, honoring `ANTIGRAVITY_DATA_DIR`.
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

    #[allow(unused_mut)]
    let mut default_roots = vec![
        home.join(".gemini").join("antigravity"),
        home.join(".gemini").join("antigravity-cli"),
        home.join(".gemini").join("antigravity-ide"),
        home.join(".gemini").join("Antigravity"),
        home.join(".gemini").join("antigravity-backup"),
        home.join(".config").join("Antigravity"),
        home.join(".config").join("Antigravity IDE"),
    ];

    #[cfg(target_os = "macos")]
    {
        default_roots.push(
            home.join("Library")
                .join("Application Support")
                .join("Antigravity"),
        );
        default_roots.push(
            home.join("Library")
                .join("Application Support")
                .join("Antigravity IDE"),
        );
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            let appdata_path = PathBuf::from(appdata);
            default_roots.push(appdata_path.join("Antigravity"));
            default_roots.push(appdata_path.join("Antigravity IDE"));
        }
    }

    Ok(default_roots)
}

fn is_excluded_db(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    EXCLUDED_DB_FILENAMES.iter().any(|&excluded| file_name.eq_ignore_ascii_case(excluded))
}

/// Check whether any Antigravity conversation database exists.
pub fn has_data() -> bool {
    antigravity_db_paths().map_or(false, |paths| !paths.is_empty())
}

/// Every per-conversation SQLite database Antigravity has written.
///
/// Antigravity stores conversation databases under `conversations/`, `brain/`, or
/// nested subagent directories. Sub-conversations (subagents, and the arms of a model
/// comparison) get their own database. Collecting all of them makes those calls
/// visible; the loader's dedup by response id prevents duplicate counting.
pub(super) fn antigravity_db_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for root in antigravity_roots()? {
        if !root.exists() {
            continue;
        }

        let mut found = Vec::new();

        // 1. Search `conversations/` directory
        let conv_dir = root.join(ANTIGRAVITY_CONVERSATIONS_DIR_NAME);
        if conv_dir.exists() {
            collect_files_with_extension(&conv_dir, "db", &mut found);
        }

        // 2. Search `brain/` directory
        let brain_dir = root.join(ANTIGRAVITY_BRAIN_DIR_NAME);
        if brain_dir.exists() {
            collect_files_with_extension(&brain_dir, "db", &mut found);
        }

        // 3. If root itself contains databases (or other subdirectories), collect them too
        collect_files_with_extension(&root, "db", &mut found);

        // Read order decides which duplicate survives, so keep it stable across
        // runs rather than inheriting the filesystem's directory order.
        found.sort();
        for path in found {
            if is_excluded_db(&path) {
                continue;
            }
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
        // The summaries database sits outside `conversations/` and is in EXCLUDED_DB_FILENAMES.
        assert_eq!(names, vec!["adf2fd49.db", "f672f900.db"]);
    }

    #[test]
    fn discovers_databases_in_brain_and_custom_subdirectories() {
        let fixture = fs_fixture!({
            "brain/sess-1/conversation.db": "",
            "conversations/subagent/sess-2.db": "",
            "state.vscdb": "",
        });
        let _cleanup = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, fixture.root());

        let paths = antigravity_db_paths().unwrap();
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["conversation.db", "sess-2.db"]);
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
        // rather than whatever home directory the test process inherits.
        let _cleanup = EnvVarsGuard::set_many([
            (ANTIGRAVITY_DATA_DIR_ENV, Some(OsString::from("   "))),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
        ]);

        // Falls back to the default root, which holds no databases under test.
        assert!(antigravity_db_paths().unwrap().is_empty());
    }
}
