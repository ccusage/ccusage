use std::{collections::HashSet, env, path::PathBuf};

use ccusage_adapter_common::collect_files_with_extension;

use crate::Result;

pub(super) const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";

const DEFAULT_ANTIGRAVITY_ROOTS: [&str; 5] = [
    ".gemini/antigravity",
    ".gemini/antigravity-cli",
    ".gemini/antigravity-ide",
    ".gemini/antigravity-backup",
    ".config/antigravity",
];

fn roots() -> Vec<PathBuf> {
    if let Ok(value) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
        return value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect();
    }

    crate::home::home_dir()
        .into_iter()
        .flat_map(|home| {
            DEFAULT_ANTIGRAVITY_ROOTS
                .into_iter()
                .map(move |root| home.join(root))
        })
        .collect()
}

fn conversation_dir(root: PathBuf) -> PathBuf {
    let nested = root.join("conversations");
    if nested.is_dir() { nested } else { root }
}

pub(super) fn conversation_db_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for root in roots().into_iter().map(conversation_dir) {
        let mut files = Vec::new();
        collect_files_with_extension(&root, "db", &mut files);
        for path in files {
            if seen.insert(path.clone()) {
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
            ".config/antigravity/conversations/config.db": "",
        });
        let _guard = EnvVarsGuard::set_many([
            (ANTIGRAVITY_DATA_DIR_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = conversation_db_paths().unwrap();

        assert_eq!(paths.len(), 5);
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
}
