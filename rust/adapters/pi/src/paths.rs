use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const PI_AGENT_DIR_ENV: &str = "PI_AGENT_DIR";

pub fn paths(custom_path: Option<&str>) -> Result<Vec<PathBuf>> {
    if let Some(paths) = custom_path.and_then(roots::from_list) {
        return Ok(roots::directories(paths.into_iter().map(store_root)));
    }
    let candidates: Vec<PathBuf> = RootsEnv::List(PI_AGENT_DIR_ENV)
        .configured()
        .map(|paths| paths.into_iter().map(store_root).collect())
        .unwrap_or_else(|| {
            roots::default_candidates(false, || {
                crate::home::home_dir()
                    .map(|home| home.join(".pi/agent/sessions"))
                    .into_iter()
                    .collect()
            })
        });
    Ok(roots::directories(candidates))
}

pub fn named_store_paths(raw: &str) -> Result<Vec<PathBuf>> {
    Ok(roots::directories(
        roots::from_list(raw).into_iter().flatten().map(store_root),
    ))
}

// NOTE(2026-11-17): Pi writes sessions to <agent dir>/sessions/<encoded cwd>/<timestamp>_<uuid>.jsonl,
// so a configured directory with a `sessions` child is the agent directory and the project is the
// directory below that child.
fn store_root(path: PathBuf) -> PathBuf {
    let sessions = path.join("sessions");
    if sessions.is_dir() { sessions } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn default_store_keeps_a_project_named_sessions() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".pi/agent/sessions");
        let _ = fixture.create_dir_all(".pi/agent/sessions/sessions");
        let _ = fixture.create_dir_all(".pi/agent/sessions/another-project");
        let _guard = ccusage_test_support::EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().into())),
            (PI_AGENT_DIR_ENV, None),
        ]);
        assert_eq!(paths(None).unwrap(), vec![store]);
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
    fn agent_directory_resolves_to_its_sessions_store() {
        let fixture = fs_fixture!({});
        let agent = fixture.create_dir_all(".pi/agent");
        let sessions = fixture.create_dir_all(".pi/agent/sessions/project-a");
        let sessions = sessions.parent().unwrap().to_path_buf();

        let raw = format!("{},{}", agent.display(), sessions.display());

        assert_eq!(named_store_paths(&raw).unwrap(), vec![sessions.clone()]);
        assert_eq!(paths(Some(&raw)).unwrap(), vec![sessions]);
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
    fn configured_and_environment_paths_expand_home_and_keep_the_sessions_root() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".pi/agent/sessions/project-a");
        let store = store.parent().unwrap().to_path_buf();
        let _guard = ccusage_test_support::EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().into())),
            (PI_AGENT_DIR_ENV, Some(" ~/.pi/agent ".into())),
        ]);

        assert_eq!(paths(None).unwrap(), vec![store.clone()]);
        assert_eq!(paths(Some(" , , ")).unwrap(), vec![store.clone()]);
        assert_eq!(paths(Some("~/.pi/agent")).unwrap(), vec![store]);
    }
}
