use std::{collections::HashSet, env, path::PathBuf};

use crate::{Result, path_utils::expand_home_path};

const PI_AGENT_DIR_ENV: &str = "PI_AGENT_DIR";

pub fn paths(custom_path: Option<&str>) -> Result<Vec<PathBuf>> {
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
    let path = home.join(".pi/agent/sessions");
    Ok(path.is_dir().then_some(path).into_iter().collect())
}

pub fn named_store_paths(raw: &str) -> Result<Vec<PathBuf>> {
    Ok(existing_named_store_path_list(raw))
}

fn existing_path_list(raw: &str) -> Vec<PathBuf> {
    // `--pi-path` / `PI_AGENT_DIR` deliberately keep their pre-existing
    // no-`~`-expansion semantics; named store paths expand `~` like other
    // config paths.
    existing_paths(raw, |path| PathBuf::from(path))
}

fn existing_named_store_path_list(raw: &str) -> Vec<PathBuf> {
    existing_paths(raw, expand_home_path)
}

fn existing_paths(raw: &str, to_path: impl Fn(&str) -> PathBuf) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(to_path)
        .filter(|path| path.is_dir() && seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn named_store_path_expands_home_directory() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".omp/agent/sessions");
        let _home = EnvVarGuard::set("HOME", fixture.root());

        let paths = named_store_paths("~/.omp/agent/sessions").unwrap();

        assert_eq!(paths, vec![store]);
    }

    #[test]
    fn named_store_path_matches_pi_path_list_semantics() {
        let fixture = fs_fixture!({});
        let first = fixture.create_dir_all("first/sessions");
        let second = fixture.create_dir_all("second/sessions");
        let missing = fixture.path("missing/sessions");

        let raw = format!(
            " {}, {}, {}, {} ",
            first.display(),
            second.display(),
            first.display(),
            missing.display()
        );
        let paths = named_store_paths(&raw).unwrap();

        assert_eq!(paths, vec![first, second]);
    }
}
