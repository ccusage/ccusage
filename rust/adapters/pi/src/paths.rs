use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const PI_AGENT_DIR_ENV: &str = "PI_AGENT_DIR";

pub fn paths(sessions_path: Option<&str>, agent_dir: Option<&str>) -> Result<Vec<PathBuf>> {
    let stores = sessions_path.and_then(roots::from_list);
    let agents = agent_dir.and_then(roots::from_list);
    if stores.is_some() || agents.is_some() {
        let agent_stores = agents
            .into_iter()
            .flatten()
            .map(|agent| agent.join("sessions"));
        return Ok(roots::directories(
            stores.into_iter().flatten().chain(agent_stores),
        ));
    }
    let candidates: Vec<PathBuf> = RootsEnv::List(PI_AGENT_DIR_ENV)
        .configured()
        .unwrap_or_else(|| {
            roots::default_candidates(false, || {
                crate::cli::home::home_dir()
                    .map(|home| home.join(".pi/agent/sessions"))
                    .into_iter()
                    .collect()
            })
        });
    Ok(roots::directories(candidates))
}

pub fn named_store_paths(raw: &str) -> Result<Vec<PathBuf>> {
    Ok(roots::from_list(raw).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn default_store_keeps_a_project_named_sessions() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".pi/agent/sessions");
        fixture.create_dir_all(".pi/agent/sessions/sessions");
        fixture.create_dir_all(".pi/agent/sessions/another-project");
        ccusage_test_support::Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (PI_AGENT_DIR_ENV, None),
        ])
        .run(|| {
            assert_eq!(paths(None, None).unwrap(), vec![store]);
        })
    }

    #[test]
    fn named_store_path_expands_home_directory() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".omp/agent/sessions");
        Environment::with_var("HOME", fixture.root()).run(|| {
            let paths = named_store_paths("~/.omp/agent/sessions").unwrap();

            assert_eq!(paths, vec![store]);
        })
    }

    #[test]
    fn explicit_paths_keep_a_project_named_sessions() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all("pi-store");
        fixture.create_dir_all("pi-store/sessions");
        fixture.create_dir_all("pi-store/another-project");
        let raw = store.display().to_string();

        assert_eq!(paths(Some(&raw), None).unwrap(), vec![store.clone()]);
        assert_eq!(named_store_paths(&raw).unwrap(), vec![store.clone()]);
        Environment::with_var(PI_AGENT_DIR_ENV, &raw).run(|| {
            assert_eq!(paths(None, None).unwrap(), vec![store]);
        })
    }

    #[test]
    fn agent_directory_option_reads_each_sessions_store() {
        let fixture = fs_fixture!({});
        let agent = fixture.create_dir_all(".pi/agent");
        let sessions = fixture.create_dir_all(".pi/agent/sessions");
        let other = fixture.create_dir_all("other-store");
        let without_sessions = fixture.create_dir_all("empty-agent");
        let agents = format!("{},{}", agent.display(), without_sessions.display());
        Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (PI_AGENT_DIR_ENV, Some(other.clone().into())),
        ])
        .run(|| {
            assert_eq!(paths(None, Some(&agents)).unwrap(), vec![sessions.clone()]);
            assert_eq!(
                paths(None, Some("~/.pi/agent")).unwrap(),
                vec![sessions.clone()]
            );
            assert_eq!(
                paths(Some(&other.display().to_string()), Some(&agents)).unwrap(),
                vec![other.clone(), sessions]
            );
        })
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
    fn configured_and_environment_paths_expand_home() {
        let fixture = fs_fixture!({});
        let store = fixture.create_dir_all(".pi/agent/sessions/project-a");
        let store = store.parent().unwrap().to_path_buf();
        ccusage_test_support::Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (PI_AGENT_DIR_ENV, Some(" ~/.pi/agent/sessions ".into())),
        ])
        .run(|| {
            assert_eq!(paths(None, None).unwrap(), vec![store.clone()]);
            assert_eq!(
                paths(Some(" , , "), Some(" , ")).unwrap(),
                vec![store.clone()]
            );
            assert_eq!(
                paths(Some("~/.pi/agent/sessions"), None).unwrap(),
                vec![store]
            );
        })
    }
}
