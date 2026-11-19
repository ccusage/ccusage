use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;
use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

pub(crate) const GROK_HOME_ENV: &str = "GROK_HOME";

fn resolve_root() -> Option<PathBuf> {
    roots::resolve(RootsEnv::Single(GROK_HOME_ENV), || {
        crate::cli::home::home_dir()
            .map(|home| home.join(".grok"))
            .into_iter()
            .collect()
    })
    .into_iter()
    .next()
}

pub(super) fn discover_session_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Some(root) = resolve_root() {
        let sessions = root.join("sessions");
        if sessions.is_dir() {
            let mut updates = Vec::new();
            collect_files_with_extension(&sessions, "jsonl", &mut updates, diagnostics);
            updates.retain(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some("updates.jsonl")
            });
            files.extend(updates);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{Environment, fs_fixture};
    use std::ffi::OsString;

    #[test]
    fn discovers_updates_jsonl_under_sessions() {
        let fixture = fs_fixture!({
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/updates.jsonl": "{}\n",
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/summary.json": "{}",
            "sessions/C%3A%5Cproj/019fa1b1-0000-7000-8000-000000000001/events.jsonl": "{}\n",
        });
        Environment::with_vars([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
            assert_eq!(files.len(), 1);
            assert!(files[0].ends_with("updates.jsonl"));
        })
    }

    #[test]
    fn ignores_jsonl_files_that_are_not_named_updates() {
        let fixture = fs_fixture!({
            "sessions/proj/sess-a/updates.jsonl": "{}\n",
            "sessions/proj/sess-a/events.jsonl": "{}\n",
            "sessions/proj/sess-a/notes.txt": "nope\n",
            "logs/unified.jsonl": "{}\n",
        });
        Environment::with_vars([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
            let names: Vec<_> = files
                .iter()
                .map(|file| file.file_name().unwrap().to_string_lossy().to_string())
                .collect();
            assert_eq!(names, vec!["updates.jsonl".to_string()]);
        })
    }

    #[test]
    fn discovers_nested_session_trees_under_multiple_projects() {
        let fixture = fs_fixture!({
            "sessions/proj-a/sess-1/updates.jsonl": "{}\n",
            "sessions/proj-b/sess-2/updates.jsonl": "{}\n",
            "sessions/proj-b/sess-2/summary.json": "{}",
        });
        Environment::with_vars([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
            assert_eq!(files.len(), 2);
            assert!(files.iter().all(|file| file.ends_with("updates.jsonl")));
            assert!(files[0] < files[1]);
        })
    }

    #[test]
    fn grok_home_is_used() {
        let fixture = fs_fixture!({
            "sessions/proj/session-a/updates.jsonl": "{}\n",
        });
        Environment::with_vars([(
            GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )])
        .run(|| {
            let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
            assert_eq!(files.len(), 1);
            assert!(files[0].ends_with("updates.jsonl"));
        })
    }

    #[test]
    fn empty_grok_home_falls_back_to_default_home() {
        let fixture = fs_fixture!({
            ".grok/sessions/home/session-a/updates.jsonl": "{}\n",
        });
        let home = OsString::from(fixture.root().as_os_str());
        Environment::with_vars([
            (GROK_HOME_ENV, Some(OsString::from("  "))),
            ("HOME", Some(home.clone())),
            ("USERPROFILE", Some(home)),
        ])
        .run(|| {
            let files = discover_session_files(&Diagnostics::new("grok")).unwrap();

            assert_eq!(files.len(), 1);
            assert!(files[0].to_string_lossy().contains("session-a"));
        })
    }

    #[test]
    fn missing_roots_yield_empty_discovery() {
        let fixture = fs_fixture!({});
        let missing = fixture.path("does-not-exist");
        Environment::with_vars([(GROK_HOME_ENV, Some(OsString::from(missing.as_os_str())))]).run(
            || {
                let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
                assert!(files.is_empty());
            },
        )
    }

    #[test]
    fn non_directory_grok_home_yields_empty_discovery() {
        let fixture = fs_fixture!({
            "not-a-dir": "file",
        });
        let file_path = fixture.path("not-a-dir");
        Environment::with_vars([(GROK_HOME_ENV, Some(OsString::from(file_path.as_os_str())))]).run(
            || {
                let files = discover_session_files(&Diagnostics::new("grok")).unwrap();
                assert!(files.is_empty());
            },
        )
    }
}
