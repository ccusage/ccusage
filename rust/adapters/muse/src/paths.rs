use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::Result;

const XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";

/// Discovers Muse Code `session.jsonl` logs under the data directory. Each
/// session directory `sessions/YYYY/MM/DD/<session-uuid>/` holds one log, and
/// `subagent/<child-uuid>/` logs hold the child agents' own model calls, which
/// the parent log does not record — they must be walked too or the session
/// undercounts.
///
/// Muse Code currently ships Linux and macOS builds only, and writes its
/// XDG-shaped tree under `$XDG_DATA_HOME` (default `~/.local/share`) on both.
/// The macOS (`~/Library/Application Support`) and Windows (`%APPDATA%`)
/// locations are scanned too as defensive candidates: they are harmless when
/// absent and keep discovery working if Muse later ships there.
pub(super) fn muse_session_files() -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Ok(dir) = env::var(XDG_DATA_HOME_ENV) {
        if dir.is_empty() {
            return Ok(Vec::new());
        }
        roots.push(PathBuf::from(dir));
    } else {
        let home =
            crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
        roots.push(home.join(".local").join("share"));

        #[cfg(target_os = "macos")]
        roots.push(home.join("Library").join("Application Support"));

        #[cfg(target_os = "windows")]
        if let Ok(appdata) = env::var("APPDATA") {
            if !appdata.is_empty() {
                roots.push(PathBuf::from(appdata));
            }
        }
    }

    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for root in roots {
        collect_session_files(&root.join("muse").join("sessions"), &mut files, &mut seen);
    }
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    files.dedup();
    Ok(files)
}

fn collect_session_files(dir: &Path, files: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file()
            && path.file_name().is_some_and(|name| name == "session.jsonl")
            && seen.insert(path.clone())
        {
            files.push(path);
        } else if file_type.is_dir() {
            collect_session_files(&path, files, seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    #[test]
    fn xdg_override_discovers_session_logs_recursively() {
        let fixture = fs_fixture!({
            "muse/sessions/2099/01/02/sess-a/session.jsonl": "",
            "muse/sessions/2099/01/02/sess-a/subagent/child-1/session.jsonl": "",
            "muse/sessions/2099/01/02/sess-b/session.jsonl": "",
            "muse/sessions/2099/01/02/sess-b/notes.txt": "",
        });
        let _cleanup = EnvVarGuard::set(XDG_DATA_HOME_ENV, fixture.root());

        let paths = muse_session_files().unwrap();

        let names = paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        // Sorted, deduped, and including the subagent tree.
        assert_eq!(names.len(), 3);
        assert!(names[0].ends_with("sess-a/session.jsonl"));
        assert!(names[1].ends_with("sess-a/subagent/child-1/session.jsonl"));
        assert!(names[2].ends_with("sess-b/session.jsonl"));
    }

    #[test]
    fn empty_xdg_override_returns_no_files() {
        let fixture = fs_fixture!({});
        let _cleanup = EnvVarsGuard::set_many([
            (XDG_DATA_HOME_ENV, Some(OsString::new())),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
        ]);

        assert!(muse_session_files().unwrap().is_empty());
    }

    #[test]
    fn default_fallback_roots_resolve_without_error() {
        let fixture = fs_fixture!({});
        let _cleanup = EnvVarsGuard::set_many([
            (XDG_DATA_HOME_ENV, None::<OsString>),
            ("HOME", Some(fixture.root().as_os_str().to_owned())),
            ("USERPROFILE", Some(fixture.root().as_os_str().to_owned())),
            #[cfg(target_os = "windows")]
            ("APPDATA", Some(fixture.root().as_os_str().to_owned())),
        ]);

        // The fixture home holds no Muse tree; discovery must still resolve
        // the per-OS default roots and come back empty rather than error.
        assert!(muse_session_files().unwrap().is_empty());
    }
}
