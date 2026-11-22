use std::{collections::HashSet, fs, path::PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

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

fn conversation_dirs(debug: bool) -> Vec<PathBuf> {
    roots::directories(
        RootsEnv::List(ANTIGRAVITY_DATA_DIR_ENV)
            .configured()
            .map(|paths| {
                paths
                    .into_iter()
                    .map(custom_conversation_dir)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                roots::default_candidates(debug, || {
                    crate::cli::home::home_dir()
                        .into_iter()
                        .flat_map(|home| {
                            DEFAULT_ANTIGRAVITY_ROOTS
                                .map(|root| home.join(root).join("conversations"))
                        })
                        .collect()
                })
            }),
    )
}

fn custom_conversation_dir(root: PathBuf) -> PathBuf {
    let nested = root.join("conversations");
    if nested.is_dir() { nested } else { root }
}

pub(super) fn conversation_db_paths(
    diagnostics: &Diagnostics,
    debug: bool,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for dir in conversation_dirs(debug) {
        let mut files = Vec::new();
        collect_files_with_extension(&dir, "db", &mut files, diagnostics);
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

    use ccusage_test_support::{Environment, fs_fixture};

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
        Environment::with_vars([
            (ANTIGRAVITY_DATA_DIR_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ])
        .run(|| {
            let paths = conversation_db_paths(&Diagnostics::new("antigravity"), false).unwrap();

            assert_eq!(paths.len(), 6);
            assert!(paths.iter().all(|path| path.extension().unwrap() == "db"));
        })
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
        Environment::with_vars([(
            ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )])
        .run(|| {
            let paths = conversation_db_paths(&Diagnostics::new("antigravity"), false).unwrap();

            assert_eq!(
                paths,
                vec![
                    fixture.path("direct/conversation.db"),
                    fixture.path("parent/conversations/parent.db"),
                ]
            );
        })
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
        Environment::with_vars([(
            ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )])
        .run(|| {
            assert_eq!(
                conversation_db_paths(&Diagnostics::new("antigravity"), false).unwrap(),
                vec![fixture.path("conversations/session.db")]
            );
        })
    }

    #[test]
    fn skips_default_roots_without_a_conversations_directory() {
        let fixture = fs_fixture!({
            ".gemini/antigravity-cli/conversation_summaries.db": "",
            ".gemini/antigravity-cli/conversations/session.db": "",
            ".gemini/antigravity/conversation_summaries.db": "",
        });
        Environment::with_vars([
            (ANTIGRAVITY_DATA_DIR_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ])
        .run(|| {
            assert_eq!(
                conversation_db_paths(&Diagnostics::new("antigravity"), false).unwrap(),
                vec![fixture.path(".gemini/antigravity-cli/conversations/session.db")]
            );
        })
    }
}
