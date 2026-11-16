use std::{
    collections::{BTreeMap, HashSet},
    env,
    path::{Path, PathBuf},
};

use crate::{Result, collect_files_matching};

pub(super) const DSH_HOME_ENV: &str = "DSH_HOME";
const DSH_HOME_DIR_NAME: &str = ".dsh";
const DSH_SESSIONS_DIR_NAME: &str = "sessions";
const COMPRESSED_SUFFIX: &str = ".zstd";

fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(DSH_HOME_ENV)
        && !env_paths.trim().is_empty()
    {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(raw);
            if path.is_dir() && seen.insert(path.clone()) {
                roots.push(path);
            }
        }
        return roots;
    }

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(DSH_HOME_DIR_NAME);
        if path.is_dir() {
            roots.push(path);
        }
    }
    roots
}

/// Returns the newest session log generation in every session directory.
///
/// dsh keeps each format generation as its own immutable file and leaves the
/// older ones behind after migrating a session, so reading every generation
/// would count the same requests more than once.
pub(super) fn discover_session_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in roots() {
        let mut newest_by_dir = BTreeMap::<PathBuf, (u32, PathBuf)>::new();
        let mut candidates = Vec::new();
        collect_files_matching(
            &root.join(DSH_SESSIONS_DIR_NAME),
            &|path| session_generation(path).is_some(),
            &mut candidates,
        );
        for file in candidates {
            let (Some(generation), Some(dir)) = (session_generation(&file), file.parent()) else {
                continue;
            };
            let dir = dir.to_path_buf();
            match newest_by_dir.get(&dir) {
                Some((newest, _)) if *newest >= generation => {}
                _ => {
                    newest_by_dir.insert(dir, (generation, file));
                }
            }
        }
        files.extend(newest_by_dir.into_values().map(|(_, file)| file));
    }
    Ok(files)
}

/// `session.jsonl` is generation 0 and `session.vN.jsonl` is generation N,
/// each optionally zstd-compressed with a `.zstd` suffix.
fn session_generation(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(COMPRESSED_SUFFIX).unwrap_or(name);
    let stem = name.strip_prefix("session")?.strip_suffix(".jsonl")?;
    if stem.is_empty() {
        return Some(0);
    }
    let digits = stem.strip_prefix(".v")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub(super) fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(COMPRESSED_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn parses_session_generations_from_file_names() {
        assert_eq!(session_generation(Path::new("a/session.jsonl")), Some(0));
        assert_eq!(session_generation(Path::new("a/session.v4.jsonl")), Some(4));
        assert_eq!(
            session_generation(Path::new("a/session.v12.jsonl.zstd")),
            Some(12)
        );
        assert_eq!(session_generation(Path::new("a/session.lock")), None);
        assert_eq!(session_generation(Path::new("a/session.v.jsonl")), None);
        assert_eq!(session_generation(Path::new("a/session.vx.jsonl")), None);
        assert_eq!(session_generation(Path::new("a/other.jsonl")), None);
    }

    #[test]
    fn discovers_only_the_newest_generation_per_session_directory() {
        let fixture = fs_fixture!({
            "sessions/--home-u-proj--/s1/session.v3.jsonl.zstd": "",
            "sessions/--home-u-proj--/s1/session.v4.jsonl.zstd": "",
            "sessions/--home-u-proj--/s1/session.lock": "",
            "sessions/--home-u-proj--/s2/session.jsonl": "",
            "sessions/_no-cwd/s3/session.v4.jsonl": "",
            "sessions/_no-cwd/s3/notes.jsonl": "",
        });
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, fixture.root());

        assert_eq!(
            discover_session_files().unwrap(),
            vec![
                fixture.path("sessions/--home-u-proj--/s1/session.v4.jsonl.zstd"),
                fixture.path("sessions/--home-u-proj--/s2/session.jsonl"),
                fixture.path("sessions/_no-cwd/s3/session.v4.jsonl"),
            ]
        );
    }

    #[test]
    fn reads_every_root_listed_in_dsh_home() {
        let first = fs_fixture!({ "sessions/p/a/session.v4.jsonl": "" });
        let second = fs_fixture!({ "sessions/p/b/session.v4.jsonl": "" });
        let value = format!(
            "{}, ,{},/missing/dsh-home",
            first.root().display(),
            second.root().display()
        );
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, &value);

        assert_eq!(
            discover_session_files().unwrap(),
            vec![
                first.path("sessions/p/a/session.v4.jsonl"),
                second.path("sessions/p/b/session.v4.jsonl"),
            ]
        );
    }
}
