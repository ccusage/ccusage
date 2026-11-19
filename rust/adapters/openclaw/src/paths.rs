use std::path::{Path, PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_adapter_common::collect_files_matching;
use ccusage_core::Diagnostics;

const OPENCLAW_DIR_ENV: &str = "OPENCLAW_DIR";

pub(super) fn paths(custom_path: Option<&str>) -> Vec<PathBuf> {
    if let Some(paths) = custom_path.and_then(roots::from_list) {
        return paths;
    }
    roots::resolve(RootsEnv::List(OPENCLAW_DIR_ENV), || {
        crate::cli::home::home_dir()
            .into_iter()
            .flat_map(|home| {
                [".openclaw", ".clawdbot", ".moltbot", ".moldbot"].map(|path| home.join(path))
            })
            .collect()
    })
}

pub(super) fn collect_session_files(root: &Path, diagnostics: &Diagnostics) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_matching(
        root,
        &|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_openclaw_session_file)
        },
        &mut files,
        diagnostics,
    );
    files.sort();
    files
}

fn is_openclaw_session_file(name: &str) -> bool {
    let Some(index) = name.find(".jsonl") else {
        return false;
    };
    let suffix = &name[index..];
    suffix == ".jsonl"
        || suffix.starts_with(".jsonl.deleted.")
        || suffix.starts_with(".jsonl.reset.")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ccusage_core::{Diagnostics, SkipKind, cli::SharedArgs};
    use ccusage_test_support::{fs_fixture, read_fixture_dir};

    use super::*;

    #[test]
    fn detects_archived_session_files_as_openclaw_sessions() {
        assert!(is_openclaw_session_file("a.jsonl.deleted.1700000000000"));
        assert!(is_openclaw_session_file(
            "a.jsonl.reset.2026-03-20T06-34-44.520Z"
        ));
        assert!(!is_openclaw_session_file("a.json"));
    }

    #[cfg(unix)]
    #[test]
    fn keeps_sibling_files_when_a_session_subdirectory_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = fs_fixture!({
            "blocked/hidden.jsonl": "{}",
            "healthy/session.jsonl": "{}",
        });
        let blocked = fixture.path("blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let permission_denied = read_fixture_dir(&blocked).is_err();

        let diagnostics = Diagnostics::new("openclaw");
        let files = collect_session_files(fixture.root(), &diagnostics);

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();

        if !permission_denied {
            return;
        }
        assert_eq!(files, [fixture.path("healthy/session.jsonl")]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(warnings.contains(blocked.to_str().unwrap()));
        assert!(warnings.contains("cannot read directory"));
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_session_directories() {
        use std::os::unix::fs::symlink;

        let fixture = fs_fixture!({
            "sessions/real/session.jsonl": "{}",
        });
        let external = fs_fixture!({
            "external/linked.jsonl": "{}",
        });
        symlink(external.path("external"), fixture.path("sessions/linked")).unwrap();

        let files = collect_session_files(&fixture.path("sessions"), &Diagnostics::new("openclaw"));

        assert_eq!(
            files,
            [
                fixture.path("sessions/linked/linked.jsonl"),
                fixture.path("sessions/real/session.jsonl"),
            ]
        );
    }
}
