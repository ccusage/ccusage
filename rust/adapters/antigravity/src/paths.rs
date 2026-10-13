use std::{collections::HashSet, env, fs, path::PathBuf};

use ccusage_adapter_common::collect_files_with_extension;

use crate::Result;

pub(super) const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";

const DEFAULT_ANTIGRAVITY_ROOTS: [&str; 6] = [
    ".gemini/antigravity",
    ".gemini/antigravity-cli",
    ".gemini/antigravity-ide",
    ".gemini/antigravity-backup",
    ".gemini/antigravity-acp",
    ".config/antigravity",
];

/// Resolves the Antigravity data roots. The flag is true when the roots come
/// from the `ANTIGRAVITY_DATA_DIR` override rather than the default home roots.
fn resolve_roots() -> (Vec<PathBuf>, bool) {
    if let Ok(value) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
        return (
            value
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect(),
            true,
        );
    }

    (
        crate::home::home_dir()
            .into_iter()
            .flat_map(|home| {
                DEFAULT_ANTIGRAVITY_ROOTS
                    .into_iter()
                    .map(move |root| home.join(root))
            })
            .collect(),
        false,
    )
}

fn conversation_dir(root: PathBuf) -> PathBuf {
    let nested = root.join("conversations");
    if nested.is_dir() { nested } else { root }
}

pub(super) fn conversation_db_paths() -> Result<Vec<PathBuf>> {
    let (roots, custom_root) = resolve_roots();
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        // Default roots must contain `conversations/`; otherwise a fresh install
        // with only `conversation_summaries.db` at the root would abort the
        // whole report with `no such table: gen_metadata`. Custom
        // `ANTIGRAVITY_DATA_DIR` entries keep the fallback so a directory can
        // be either a data root or the conversation directory itself.
        if !custom_root && !root.join("conversations").is_dir() {
            continue;
        }
        let dir = conversation_dir(root);
        let mut files = Vec::new();
        collect_files_with_extension(&dir, "db", &mut files);
        for path in files {
            let canonical_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.insert(canonical_path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::*;

    #[test]
    fn discovers_databases_from_independent_roots() {
        let fixture = fs_fixture!({
            ".gemini/antigravity/conversations/ide.db": "",
            ".gemini/antigravity-cli/conversations/cli.db": "",
            ".gemini/antigravity-ide/conversations/ide-alt.db": "",
            ".gemini/antigravity-backup/conversations/backup.db": "",
            ".gemini/antigravity-acp/conversations/acp.db": "",
            ".config/antigravity/conversations/config.db": "",
        });
        let _guard = EnvVarsGuard::set_many([
            (ANTIGRAVITY_DATA_DIR_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = conversation_db_paths().unwrap();

        assert_eq!(paths.len(), 6);
        assert!(paths.iter().all(|path| path.extension().unwrap() == "db"));
    }

    #[test]
    fn accepts_conversation_directory_and_parent_directory_overrides() {
        let fixture = fs_fixture!({
            "parent/conversations/parent.db": "",
            "direct/conversation.db": "",
        });
        let override_value = format!(
            "{},{}",
            fixture.path("parent").display(),
            fixture.path("direct").display()
        );
        let _guard = EnvVarsGuard::set_many([(
            ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        let paths = conversation_db_paths().unwrap();

        assert_eq!(
            paths,
            vec![
                fixture.path("direct/conversation.db"),
                fixture.path("parent/conversations/parent.db"),
            ]
        );
    }

    #[test]
    fn deduplicates_canonicalized_override_aliases() {
        let fixture = fs_fixture!({
            "conversations/session.db": "",
        });
        let override_value = format!(
            "{},{}",
            fixture.root().display(),
            fixture.path("conversations/..").display()
        );
        let _guard = EnvVarsGuard::set_many([(
            ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        assert_eq!(
            conversation_db_paths().unwrap(),
            vec![fixture.path("conversations/session.db")]
        );
    }

    #[test]
    fn skips_default_roots_without_a_conversations_directory() {
        let fixture = fs_fixture!({
            ".gemini/antigravity-cli/conversation_summaries.db": "",
            ".gemini/antigravity-cli/conversations/session.db": "",
            ".gemini/antigravity/conversation_summaries.db": "",
        });
        let _guard = EnvVarsGuard::set_many([
            (ANTIGRAVITY_DATA_DIR_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        assert_eq!(
            conversation_db_paths().unwrap(),
            vec![fixture.path(".gemini/antigravity-cli/conversations/session.db")]
        );
    }
}
