use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{Result, cli_error, fast::FxHashSet, home};

pub(super) fn codex_usage_sources() -> Result<Vec<CodexUsageSource>> {
    Ok(codex_usage_sources_from_homes(codex_home_paths()?))
}

#[cfg(test)]
fn codex_usage_paths_from_homes(homes: Vec<PathBuf>) -> Vec<PathBuf> {
    codex_usage_sources_from_homes(homes)
        .into_iter()
        .map(|source| source.dir)
        .collect()
}

fn codex_usage_sources_from_homes(homes: Vec<PathBuf>) -> Vec<CodexUsageSource> {
    let mut paths = Vec::new();
    let mut seen = FxHashSet::default();
    for path in homes {
        let sessions = path.join("sessions");
        let archived_sessions = path.join("archived_sessions");
        let mut found_usage_dir = false;
        if sessions.is_dir() {
            if seen.insert(sessions.clone()) {
                paths.push(CodexUsageSource {
                    dir: sessions,
                    dedupe_scope: path.clone(),
                });
            }
            found_usage_dir = true;
        }
        if archived_sessions.is_dir() {
            if seen.insert(archived_sessions.clone()) {
                paths.push(CodexUsageSource {
                    dir: archived_sessions,
                    dedupe_scope: path.clone(),
                });
            }
            found_usage_dir = true;
        }
        if !found_usage_dir && seen.insert(path.clone()) {
            paths.push(CodexUsageSource {
                dir: path.clone(),
                dedupe_scope: path,
            });
        }
    }
    paths
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodexUsageSource {
    pub(super) dir: PathBuf,
    dedupe_scope: PathBuf,
}

#[cfg(test)]
impl CodexUsageSource {
    pub(super) fn new_for_test(dir: PathBuf, dedupe_scope: PathBuf) -> Self {
        Self { dir, dedupe_scope }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CodexUsageFileGroup {
    pub(super) dir: PathBuf,
    pub(super) files: Vec<PathBuf>,
}

pub(super) fn collect_codex_usage_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    crate::collect_usage_files(dir, &mut files);
    files.sort();
    files
}

pub(super) fn collect_deduped_codex_usage_files(
    sources: &[CodexUsageSource],
) -> Vec<CodexUsageFileGroup> {
    let mut seen = FxHashSet::default();
    let mut groups = Vec::new();
    for source in sources {
        let files = collect_codex_usage_files(&source.dir)
            .into_iter()
            .filter(|file| seen.insert(codex_usage_file_key(source, file)))
            .collect::<Vec<_>>();
        groups.push(CodexUsageFileGroup {
            dir: source.dir.clone(),
            files,
        });
    }
    groups
}

fn codex_usage_file_key(source: &CodexUsageSource, file: &Path) -> (PathBuf, PathBuf) {
    let relative = file.strip_prefix(&source.dir).unwrap_or(file).to_path_buf();
    (source.dedupe_scope.clone(), relative)
}

pub(super) fn codex_home_paths() -> Result<Vec<PathBuf>> {
    if let Ok(env_paths) = env::var("CODEX_HOME") {
        return Ok(env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect());
    }

    let home = home::home_dir().ok_or_else(|| cli_error("home directory is not set"))?;
    Ok(vec![home.join(".codex")])
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::Fixture;

    #[test]
    fn includes_archived_sessions_next_to_sessions() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");
        let _ = fixture.create_dir_all("codex/archived_sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(
            paths,
            vec![
                fixture.path("codex/sessions"),
                fixture.path("codex/archived_sessions"),
            ]
        );
    }

    #[test]
    fn uses_sessions_without_missing_archived_sessions_path() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn uses_archived_sessions_without_direct_path_fallback() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/archived_sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(paths, vec![fixture.path("codex/archived_sessions")]);
    }

    #[test]
    fn falls_back_to_direct_path_when_no_session_directories_exist() {
        let fixture = Fixture::new();
        let home = fixture.create_dir_all("codex");

        let paths = codex_usage_paths_from_homes(vec![home.clone()]);

        assert_eq!(paths, vec![home]);
    }

    #[test]
    fn deduplicates_usage_paths_across_repeated_homes() {
        let fixture = Fixture::new();
        let home = fixture.create_dir_all("codex");
        let _ = fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![home.clone(), home]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn keeps_active_session_file_when_archived_file_has_same_relative_path() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");
        let _ = fixture.create_dir_all("codex/archived_sessions");
        let _ = fixture.write_file("codex/sessions/session.jsonl", "");
        let _ = fixture.write_file("codex/archived_sessions/session.jsonl", "");
        let _ = fixture.write_file("codex/archived_sessions/archive-only.jsonl", "");

        let sources = codex_usage_sources_from_homes(vec![fixture.path("codex")]);
        let groups = collect_deduped_codex_usage_files(&sources);

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    dir: fixture.path("codex/sessions"),
                    files: vec![fixture.path("codex/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    dir: fixture.path("codex/archived_sessions"),
                    files: vec![fixture.path("codex/archived_sessions/archive-only.jsonl")],
                },
            ]
        );
    }

    #[test]
    fn keeps_same_relative_session_file_across_different_homes() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("work/sessions");
        let _ = fixture.create_dir_all("personal/sessions");
        let _ = fixture.write_file("work/sessions/session.jsonl", "");
        let _ = fixture.write_file("personal/sessions/session.jsonl", "");

        let sources =
            codex_usage_sources_from_homes(vec![fixture.path("work"), fixture.path("personal")]);
        let groups = collect_deduped_codex_usage_files(&sources);

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    dir: fixture.path("work/sessions"),
                    files: vec![fixture.path("work/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    dir: fixture.path("personal/sessions"),
                    files: vec![fixture.path("personal/sessions/session.jsonl")],
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn deduplicates_non_utf8_relative_session_paths_without_lossy_strings() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let file_name = PathBuf::from(OsString::from_vec(b"session-\xFF.jsonl".to_vec()));
        let source = CodexUsageSource::new_for_test(
            PathBuf::from("/codex/sessions"),
            PathBuf::from("/codex"),
        );

        assert_eq!(
            codex_usage_file_key(&source, &source.dir.join(&file_name)),
            (PathBuf::from("/codex"), file_name)
        );
    }
}
