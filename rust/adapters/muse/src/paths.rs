use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use crate::Result;
use ccusage_adapter_common::collect_files_with_extension;

const MUSE_DATA_DIR_ENV: &str = "MUSE_DATA_DIR";
const MUSE_SESSIONS_DIR: &str = "sessions";
const MUSE_SESSION_FILE: &str = "session.jsonl";

fn data_roots(
    muse_data_dir: Option<&str>,
    xdg_data_home: Option<&str>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(muse_data_dir) = muse_data_dir {
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        for root in muse_data_dir
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
        {
            let root = PathBuf::from(root);
            if seen.insert(root.clone()) {
                roots.push(root);
            }
        }
        return roots;
    }

    let data_home = xdg_data_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".local").join("share")));

    data_home
        .map(|path| vec![path.join("muse")])
        .unwrap_or_default()
}

fn configured_roots() -> Vec<PathBuf> {
    let muse_data_dir = env::var(MUSE_DATA_DIR_ENV).ok();
    let xdg_data_home = env::var("XDG_DATA_HOME").ok();
    let home = crate::home::home_dir();
    data_roots(
        muse_data_dir.as_deref(),
        xdg_data_home.as_deref(),
        home.as_deref(),
    )
}

fn discover_session_files_under_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in roots {
        let sessions = root.join(MUSE_SESSIONS_DIR);
        let mut candidates = Vec::new();
        collect_files_with_extension(&sessions, "jsonl", &mut candidates);
        files.extend(candidates.into_iter().filter(|file| {
            file.file_name().and_then(|name| name.to_str()) == Some(MUSE_SESSION_FILE)
        }));
    }
    files.sort();
    files.dedup();
    files
}

pub(super) fn discover_session_files() -> Result<Vec<PathBuf>> {
    let roots = configured_roots();
    Ok(discover_session_files_under_roots(&roots))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn resolves_comma_separated_data_dir_roots_and_empty_override() {
        assert_eq!(
            data_roots(Some(" /first/muse, ,/second/muse,/first/muse "), None, None),
            vec![PathBuf::from("/first/muse"), PathBuf::from("/second/muse")]
        );
        assert!(data_roots(Some(" , "), Some("/xdg"), Some(Path::new("/home/u"))).is_empty());
    }

    #[test]
    fn uses_xdg_data_home_and_falls_back_when_it_is_empty() {
        assert_eq!(
            data_roots(None, Some("/xdg/data"), Some(Path::new("/home/u"))),
            vec![PathBuf::from("/xdg/data/muse")]
        );
        assert_eq!(
            data_roots(None, Some(""), Some(Path::new("/home/u"))),
            vec![PathBuf::from("/home/u/.local/share/muse")]
        );
    }

    #[test]
    fn has_no_default_root_without_xdg_or_home() {
        assert!(data_roots(None, None, None).is_empty());
    }

    #[test]
    fn recursively_discovers_session_files_including_subagents() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": "{}\n",
            "muse/sessions/2026/09/07/session-a/subagent/child-a/session.jsonl": "{}\n",
            "muse/sessions/2026/09/07/session-a/events.jsonl": "{}\n",
        });
        let files = discover_session_files_under_roots(&[fixture.path("muse")]);

        assert_eq!(
            files,
            vec![
                fixture.path("muse/sessions/2026/09/07/session-a/session.jsonl"),
                fixture.path("muse/sessions/2026/09/07/session-a/subagent/child-a/session.jsonl"),
            ]
        );
    }
}
