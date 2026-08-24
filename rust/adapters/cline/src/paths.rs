use std::{collections::HashSet, env, fs, path::{Path, PathBuf}};

use crate::Result;

const CLINE_HOME_ENV: &str = "CLINE_HOME";
const CLINE_EXTENSION_ID: &str = "saoudrizwan.claude-dev";

/// Discovers Cline `*.messages.json` transcripts.
///
/// Cline writes the same `*.messages.json` transcript format in two places:
///
/// - the Cline CLI (and the JetBrains plugin, which shares its data dir)
///   stores them under `~/.cline` (`%USERPROFILE%\.cline` on Windows), with
///   historical sessions under `data/sessions`;
/// - the VS Code extension stores them in its `globalStorage` directory,
///   whose parent `User` folder lives under a per-OS VS Code data root.
///
/// Scanning both surfaces usage regardless of which front end produced the
/// session, and each assistant message carries its own model + metrics, which
/// is what lets ccusage show a per-model breakdown when a session switches
/// models mid-conversation.
pub(super) fn cline_messages_files() -> Result<Vec<PathBuf>> {
    let roots = discovery_roots()?;
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for root in &roots {
        collect_messages_files(root, &mut files, &mut seen);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Roots to search for Cline transcripts, honoring `CLINE_HOME`.
///
/// The override accepts comma-separated directories, like every other
/// source-specific path variable, so a current profile and an archive can be
/// reported together. A blank value falls back to the default roots.
fn discovery_roots() -> Result<Vec<PathBuf>> {
    if let Ok(env_roots) = env::var(CLINE_HOME_ENV) {
        let roots = env_roots
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            return Ok(roots);
        }
    }
    let home =
        ccusage_core::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;

    // The CLI and JetBrains plugin share `~/.cline`. `data/sessions` holds the
    // historical layout, but newer builds also write transcripts elsewhere
    // under the root, so discovery scans both from the root itself.
    let mut roots = vec![home.join(".cline")];

    // VS Code's globalStorage parent differs per OS because VS Code itself
    // keeps its user data in a platform-specific location.
    let mut code_roots = vs_code_user_roots(&home);
    roots.append(&mut code_roots);

    for root in &mut roots {
        if let Ok(canonical) = root.canonicalize() {
            *root = canonical;
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// `User` directories of every stock VS Code distribution worth scanning.
///
/// Insiders and VSCodium builds keep separate user-data directories, so each
/// one may hold its own Cline `globalStorage`.
fn vs_code_user_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push_code_root = |user_dir: PathBuf| {
        roots.push(user_dir.join("globalStorage").join(CLINE_EXTENSION_ID));
    };

    #[cfg(target_os = "linux")]
    {
        let config = home.join(".config");
        for name in ["Code", "Code - Insiders", "VSCodium"] {
            push_code_root(config.join(name).join("User"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let app_support = home.join("Library").join("Application Support");
        for name in ["Code", "Code - Insiders", "VSCodium"] {
            push_code_root(app_support.join(name).join("User"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            let appdata = PathBuf::from(appdata);
            for name in ["Code", "Code - Insiders", "VSCodium"] {
                push_code_root(appdata.join(name).join("User"));
            }
        }
    }

    roots.retain(|root| root.is_dir());
    roots
}

fn collect_messages_files(dir: &Path, files: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".messages.json"))
            && seen.insert(path.clone())
        {
            files.push(path);
        } else if file_type.is_dir() {
            collect_messages_files(&path, files, seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    fn file_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn discovers_transcripts_recursively_under_cline_home() {
        let fixture = fs_fixture!({
            "data/sessions/sess-a/sess-a.messages.json": "",
            "tasks/sess-b/sess-b.messages.json": "",
            "data/sessions/sess-a/notes.json": "",
            "data/sessions/sess-a/sess-a.ui_messages.json": "",
        });
        let _cleanup = EnvVarGuard::set(CLINE_HOME_ENV, fixture.root());

        let names = file_names(&cline_messages_files().unwrap());
        assert_eq!(names, vec!["sess-a.messages.json", "sess-b.messages.json"]);
    }

    #[test]
    fn ignores_a_blank_cline_home_override() {
        let fixture = fs_fixture!({});
        // Pin `HOME` alongside the override so the fallback roots live under
        // the fixture rather than whatever home directory the test process
        // inherits. The fixture holds no Cline data.
        let _cleanup = EnvVarsGuard::set_many([
            (CLINE_HOME_ENV, Some(OsString::from("   "))),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
        ]);

        assert!(cline_messages_files().unwrap().is_empty());
    }

    #[test]
    fn reads_comma_separated_cline_homes() {
        let current = fs_fixture!({ "data/sessions/sess-a/sess-a.messages.json": "" });
        let archive = fs_fixture!({ "sessions/sess-b/sess-b.messages.json": "" });
        let _cleanup = EnvVarGuard::set(
            CLINE_HOME_ENV,
            format!("{}, {}", current.root().display(), archive.root().display()),
        );

        // Snapshot tempdirs may be created out of order, so assert as a set;
        // path-level ordering is covered by the sorting in cline_messages_files.
        let mut names = file_names(&cline_messages_files().unwrap());
        names.sort();
        assert_eq!(names, vec!["sess-a.messages.json", "sess-b.messages.json"]);
    }

    #[test]
    fn dedupes_overlapping_cline_homes() {
        let root = fs_fixture!({ "data/sessions/sess-a/sess-a.messages.json": "" });
        let _cleanup = EnvVarGuard::set(
            CLINE_HOME_ENV,
            format!("{},{}", root.root().display(), root.root().display()),
        );

        let names = file_names(&cline_messages_files().unwrap());
        assert_eq!(names, vec!["sess-a.messages.json"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovers_vs_code_extension_transcripts() {
        let fixture = fs_fixture!({});
        // Point the default `~/.config` at the fixture through `HOME` while
        // clearing the override, exercising the default discovery path.
        let _cleanup = EnvVarsGuard::set_many([
            (CLINE_HOME_ENV, None),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
        ]);
        let global_storage = fixture
            .root()
            .join(".config")
            .join("Code")
            .join("User")
            .join("globalStorage")
            .join(CLINE_EXTENSION_ID)
            .join("tasks")
            .join("sess-v");
        std::fs::create_dir_all(&global_storage).unwrap();
        std::fs::write(global_storage.join("sess-v.messages.json"), "").unwrap();

        let names = file_names(&cline_messages_files().unwrap());
        assert_eq!(names, vec!["sess-v.messages.json"]);
    }
}
