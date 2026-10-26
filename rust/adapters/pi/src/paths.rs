use std::{
    collections::{BTreeSet, HashSet},
    env, fs,
    path::{Component, Path, PathBuf},
};

use crate::{Result, path_utils::expand_home_path};

const PI_AGENT_DIR_ENV: &str = "PI_AGENT_DIR";
const OMP_CONFIG_DIR_ENV: &str = "PI_CONFIG_DIR";
const OMP_AGENT_DIR_ENV: &str = "PI_CODING_AGENT_DIR";
const OMP_XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";
const OMP_APP_DIR: &str = "omp";

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

/// Returns existing default, custom, profile, and XDG OMP session roots for unified reports.
pub fn omp_default_paths() -> Result<Vec<PathBuf>> {
    let Some(home) = crate::home::home_dir() else {
        return Ok(Vec::new());
    };
    let config_dir = env::var(OMP_CONFIG_DIR_ENV)
        .ok()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| ".omp".to_string());
    // OMP joins PI_CONFIG_DIR to HOME, including when the value starts with `/`.
    let config_root = join_under_home(&home, &config_dir);
    let xdg_app_root = xdg_app_root();
    let agent_dir_override = env::var(OMP_AGENT_DIR_ENV)
        .ok()
        .filter(|path| !path.is_empty())
        .and_then(|path| resolve_from_current_dir(&path));
    let default_agent_dir = config_root.join("agent");
    let mut candidates = Vec::new();

    if let Some(xdg_app_root) = &xdg_app_root {
        candidates.push(xdg_app_root.join("sessions"));
    }
    candidates.push(default_agent_dir.join("sessions"));
    if let Some(agent_dir) = agent_dir_override {
        candidates.push(agent_dir.join("sessions"));
    }

    // Unified reports include every existing profile root, even when another is active.
    let mut profile_roots = vec![config_root.join("profiles")];
    if let Some(xdg_app_root) = &xdg_app_root {
        profile_roots.push(xdg_app_root.join("profiles"));
    }
    for profile in profile_names(profile_roots) {
        if let Some(xdg_app_root) = &xdg_app_root {
            candidates.push(
                xdg_app_root
                    .join("profiles")
                    .join(&profile)
                    .join("sessions"),
            );
        }
        candidates.push(
            config_root
                .join("profiles")
                .join(profile)
                .join("agent/sessions"),
        );
    }

    Ok(existing_non_overlapping_paths(candidates))
}

fn join_under_home(home: &Path, raw: &str) -> PathBuf {
    let mut joined = normalize_path(home);
    for component in Path::new(raw).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() && !joined.has_root() {
                    joined.push(component.as_os_str());
                }
            }
            Component::Normal(_) => {
                joined.push(component.as_os_str());
            }
        }
    }
    joined
}

fn resolve_from_current_dir(raw: &str) -> Option<PathBuf> {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return Some(normalize_path(&path));
    }
    let current_dir = env::current_dir().ok();
    resolve_relative_path(&path, current_dir.as_deref())
}

fn resolve_relative_path(path: &Path, current_dir: Option<&Path>) -> Option<PathBuf> {
    current_dir.map(|current_dir| normalize_path(&current_dir.join(path)))
}

fn normalize_path(path: &Path) -> PathBuf {
    // Node resolves dot segments before comparing the override with the default path.
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn xdg_app_root() -> Option<PathBuf> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return None;
    }
    let data_home = env::var_os(OMP_XDG_DATA_HOME_ENV)?;
    if data_home.is_empty() {
        return None;
    }
    let app_root = PathBuf::from(data_home).join(OMP_APP_DIR);
    app_root.exists().then_some(app_root)
}

fn profile_names(roots: impl IntoIterator<Item = PathBuf>) -> BTreeSet<std::ffi::OsString> {
    roots
        .into_iter()
        .filter_map(|root| fs::read_dir(root).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name())
        .collect()
}

fn existing_non_overlapping_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut candidates = paths
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| (canonical_path(&path), path))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(key, _)| key.components().count());

    let mut accepted_keys: Vec<PathBuf> = Vec::new();
    let mut accepted_paths = Vec::new();
    for (key, path) in candidates {
        // Resolving aliases and ancestor roots prevents scanning the same session tree twice.
        if accepted_keys
            .iter()
            .any(|accepted| paths_overlap(&key, accepted))
        {
            continue;
        }
        accepted_keys.push(key);
        accepted_paths.push(path);
    }
    accepted_paths
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
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
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    fn isolated_omp_path_environment(
        fixture: &ccusage_test_support::Fixture,
        config_dir: Option<&str>,
        agent_dir: Option<&Path>,
        xdg_data_home: Option<&Path>,
    ) -> EnvVarsGuard {
        EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            ("USERPROFILE", None),
            ("PI_AGENT_DIR", None),
            ("PI_CONFIG_DIR", config_dir.map(std::ffi::OsString::from)),
            (
                "PI_CODING_AGENT_DIR",
                agent_dir.map(|path| path.as_os_str().to_os_string()),
            ),
            ("OMP_PROFILE", None),
            ("PI_PROFILE", None),
            (
                "XDG_DATA_HOME",
                xdg_data_home.map(|path| path.as_os_str().to_os_string()),
            ),
            ("XDG_STATE_HOME", None),
            ("XDG_CACHE_HOME", None),
        ])
    }

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

    #[test]
    fn omp_default_paths_uses_pi_config_dir_under_home() {
        let fixture = fs_fixture!({});
        let sessions = fixture.create_dir_all("custom-root/agent/sessions");
        let _environment =
            isolated_omp_path_environment(&fixture, Some("/custom-root"), None, None);

        let paths = omp_default_paths().unwrap();

        assert_eq!(paths, vec![sessions]);
    }

    #[test]
    fn omp_default_paths_adds_custom_agent_dir_without_hiding_default_history() {
        let fixture = fs_fixture!({});
        let default_sessions = fixture.create_dir_all(".omp/agent/sessions");
        let agent_dir = fixture.path("custom-agent");
        let sessions = fixture.create_dir_all("custom-agent/sessions");
        let _environment = isolated_omp_path_environment(&fixture, None, Some(&agent_dir), None);

        let paths = omp_default_paths().unwrap();

        assert!(paths.contains(&default_sessions));
        assert!(paths.contains(&sessions));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn omp_default_paths_keeps_xdg_when_agent_override_is_the_default() {
        let fixture = fs_fixture!({});
        let default_agent = fixture.path(".omp/agent/../agent");
        let default_sessions = fixture.create_dir_all(".omp/agent/sessions");
        let xdg_data_home = fixture.path("xdg-data");
        let xdg_sessions = fixture.create_dir_all("xdg-data/omp/sessions");
        let _environment = isolated_omp_path_environment(
            &fixture,
            None,
            Some(&default_agent),
            Some(&xdg_data_home),
        );

        let paths = omp_default_paths().unwrap();

        assert!(paths.contains(&xdg_sessions));
        assert!(paths.contains(&default_sessions));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn omp_default_paths_keeps_xdg_with_custom_agent_override() {
        let fixture = fs_fixture!({});
        let default_sessions = fixture.create_dir_all(".omp/agent/sessions");
        let custom_sessions = fixture.create_dir_all("custom-agent/sessions");
        let xdg_data_home = fixture.path("xdg-data");
        let xdg_sessions = fixture.create_dir_all("xdg-data/omp/sessions");
        let custom_agent_dir = fixture.path("custom-agent");
        let _environment = isolated_omp_path_environment(
            &fixture,
            None,
            Some(&custom_agent_dir),
            Some(&xdg_data_home),
        );

        let paths = omp_default_paths().unwrap();

        assert!(paths.contains(&default_sessions));
        assert!(paths.contains(&xdg_sessions));
        assert!(paths.contains(&custom_sessions));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn omp_default_paths_discovers_profile_and_xdg_roots() {
        let fixture = fs_fixture!({});
        let default_sessions = fixture.create_dir_all(".omp/agent/sessions");
        let profile_sessions = fixture.create_dir_all(".omp/profiles/work/agent/sessions");
        let xdg_data_home = fixture.path("xdg-data");
        let xdg_sessions = fixture.create_dir_all("xdg-data/omp/sessions");
        let xdg_profile_sessions =
            fixture.create_dir_all("xdg-data/omp/profiles/research/sessions");
        let _environment =
            isolated_omp_path_environment(&fixture, None, None, Some(&xdg_data_home));

        let paths = omp_default_paths().unwrap();

        assert_eq!(paths.len(), 4);
        assert!(paths.contains(&xdg_sessions));
        assert!(paths.contains(&default_sessions));
        assert!(paths.contains(&profile_sessions));
        assert!(paths.contains(&xdg_profile_sessions));
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn omp_default_paths_deduplicates_symlink_roots() {
        let fixture = fs_fixture!({});
        let sessions = fixture.create_dir_all(".omp/agent/sessions");
        let xdg_data_home = fixture.path("xdg-data");
        let xdg_app_root = fixture.create_dir_all("xdg-data/omp");
        let xdg_sessions = xdg_app_root.join("sessions");
        std::os::unix::fs::symlink(&sessions, &xdg_sessions).unwrap();
        let _environment =
            isolated_omp_path_environment(&fixture, None, None, Some(&xdg_data_home));

        let paths = omp_default_paths().unwrap();

        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].canonicalize().unwrap(),
            sessions.canonicalize().unwrap()
        );
    }

    #[test]
    fn omp_default_paths_keeps_an_ancestor_when_a_nested_candidate_comes_first() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("omp/sessions");
        let nested = fixture.create_dir_all("omp/sessions/project-a");

        let paths = existing_non_overlapping_paths([nested, root.clone()]);

        assert_eq!(paths, vec![root]);
    }

    #[test]
    fn omp_default_paths_is_empty_when_home_is_unavailable() {
        let fixture = fs_fixture!({});
        let _environment = EnvVarsGuard::set_many([
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (
                "PI_AGENT_DIR",
                Some(fixture.path("custom-pi-agent").into_os_string()),
            ),
            ("PI_CONFIG_DIR", None),
            ("PI_CODING_AGENT_DIR", None),
            ("OMP_PROFILE", None),
            ("PI_PROFILE", None),
            ("XDG_DATA_HOME", None),
        ]);

        assert!(omp_default_paths().unwrap().is_empty());
    }

    #[test]
    fn relative_custom_agent_directory_is_skipped_when_current_directory_is_unavailable() {
        assert_eq!(resolve_relative_path(Path::new("custom-agent"), None), None);
    }
}
