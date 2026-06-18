use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use crate::Result;

const PI_AGENT_DIR_ENV: &str = "PI_AGENT_DIR";

/// Session directories scanned by default when neither `--pi-path` nor
/// `PI_AGENT_DIR` is set. `oh-my-pi` (omp) is a widely used pi fork that writes
/// identical JSONL session files, so its directory is auto-detected alongside
/// pi's; entries from both are deduplicated by the loader.
const DEFAULT_SESSION_DIRS: [&str; 2] = [".pi/agent/sessions", ".omp/agent/sessions"];

pub(super) fn paths(custom_path: Option<&str>) -> Result<Vec<PathBuf>> {
    if let Some(custom_path) = custom_path.filter(|path| !path.trim().is_empty()) {
        return Ok(existing_path_list(custom_path));
    }
    if let Ok(env_paths) = env::var(PI_AGENT_DIR_ENV)
        && !env_paths.trim().is_empty()
    {
        return Ok(existing_path_list(&env_paths));
    }

    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(default_session_dirs(&home))
}

fn existing_path_list(raw: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir() && seen.insert(path.clone()))
        .collect()
}

fn default_session_dirs(home: &Path) -> Vec<PathBuf> {
    DEFAULT_SESSION_DIRS
        .iter()
        .map(|relative| home.join(relative))
        .filter(|path| path.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::Fixture;

    #[test]
    fn detects_pi_and_omp_session_dirs() {
        let fixture = Fixture::new();
        let pi = fixture.create_dir_all(".pi/agent/sessions");
        let omp = fixture.create_dir_all(".omp/agent/sessions");

        assert_eq!(default_session_dirs(fixture.root()), vec![pi, omp]);
    }

    #[test]
    fn detects_omp_when_pi_dir_is_missing() {
        let fixture = Fixture::new();
        let omp = fixture.create_dir_all(".omp/agent/sessions");

        assert_eq!(default_session_dirs(fixture.root()), vec![omp]);
    }

    #[test]
    fn detects_pi_when_omp_dir_is_missing() {
        let fixture = Fixture::new();
        let pi = fixture.create_dir_all(".pi/agent/sessions");

        assert_eq!(default_session_dirs(fixture.root()), vec![pi]);
    }

    #[test]
    fn returns_no_dirs_when_neither_default_exists() {
        let fixture = Fixture::new();

        assert!(default_session_dirs(fixture.root()).is_empty());
    }
}
