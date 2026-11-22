use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_adapter_common::collect_files_matching;

use crate::{Diagnostics, Result};

pub(super) const DSH_HOME_ENV: &str = "DSH_HOME";
const DSH_HOME_DIR_NAME: &str = ".dsh";
const DSH_SESSIONS_DIR_NAME: &str = "sessions";
const COMPRESSED_SUFFIX: &str = ".zstd";

fn roots() -> Vec<PathBuf> {
    roots::resolve(RootsEnv::List(DSH_HOME_ENV), || {
        crate::cli::home::home_dir()
            .map(|home| home.join(DSH_HOME_DIR_NAME))
            .into_iter()
            .collect()
    })
}

// NOTE(2026-11-16, #2112): DeepSeek Harness retains older immutable log generations after migrating a session. Only the newest generation should contribute usage.
pub(super) fn discover_session_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in roots() {
        let mut newest_by_dir = BTreeMap::<PathBuf, (u32, PathBuf)>::new();
        let mut candidates = Vec::new();
        collect_files_matching(
            &root.join(DSH_SESSIONS_DIR_NAME),
            &|path| session_generation(path).is_some(),
            &mut candidates,
            diagnostics,
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
    use ccusage_test_support::{Environment, fs_fixture};

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
        Environment::with_var(DSH_HOME_ENV, fixture.root()).run(|| {
            assert_eq!(
                discover_session_files(&Diagnostics::new("dsh")).unwrap(),
                vec![
                    fixture.path("sessions/--home-u-proj--/s1/session.v4.jsonl.zstd"),
                    fixture.path("sessions/--home-u-proj--/s2/session.jsonl"),
                    fixture.path("sessions/_no-cwd/s3/session.v4.jsonl"),
                ]
            );
        })
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
        Environment::with_var(DSH_HOME_ENV, &value).run(|| {
            assert_eq!(
                discover_session_files(&Diagnostics::new("dsh")).unwrap(),
                vec![
                    first.path("sessions/p/a/session.v4.jsonl"),
                    second.path("sessions/p/b/session.v4.jsonl"),
                ]
            );
        })
    }
}
