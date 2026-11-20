use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_adapter_common::collect_files_matching;
use ccusage_core::Diagnostics;

use crate::Result;

pub(super) const GEMINI_DATA_DIR_ENV: &str = "GEMINI_DATA_DIR";
fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(GEMINI_DATA_DIR_ENV), || {
        crate::cli::home::home_dir()
            .into_iter()
            .flat_map(|home| [".gemini/tmp"].map(|path| home.join(path)))
            .collect()
    }))
}

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
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn discovers_json_and_jsonl_logs() {
        let fixture = fs_fixture!({
            "chats/a.json": "{}",
            "chats/b.jsonl": "{}\n",
            "chats/ignore.txt": "no",
        });
        ccusage_test_support::Environment::with_var(
            crate::paths::GEMINI_DATA_DIR_ENV,
            fixture.root(),
        )
        .run(|| {
            let files = discover_log_files(&Diagnostics::new("gemini")).unwrap();

            assert_eq!(
                files,
                vec![fixture.path("chats/a.json"), fixture.path("chats/b.jsonl")]
            );
        })
    }

    #[test]
    fn discovers_logs_outside_chat_directories() {
        let fixture = fs_fixture!({
            "overflow.jsonl": "{}\n",
            "abc/chats/a.json": "{}",
        });
        Environment::with_var(crate::paths::GEMINI_DATA_DIR_ENV, fixture.root()).run(|| {
            let files = discover_log_files(&Diagnostics::new("gemini")).unwrap();

            assert_eq!(
                files,
                vec![
                    fixture.path("abc/chats/a.json"),
                    fixture.path("overflow.jsonl")
                ]
            );
        });
    }

    #[test]
    fn antigravity_override_does_not_replace_gemini_discovery() {
        let fixture = fs_fixture!({
            "gemini/chats/a.json": "{}",
            "antigravity/conversations/a.db": "not a Gemini log",
        });
        Environment::with_vars([
            (
                GEMINI_DATA_DIR_ENV,
                Some(OsString::from(fixture.path("gemini"))),
            ),
            (
                "ANTIGRAVITY_DATA_DIR",
                Some(OsString::from(fixture.path("antigravity"))),
            ),
        ])
        .run(|| {
            let files = discover_log_files(&Diagnostics::new("gemini")).unwrap();

            assert_eq!(files, vec![fixture.path("gemini/chats/a.json")]);
        })
    }
}
