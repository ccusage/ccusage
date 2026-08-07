use std::{collections::HashSet, env, fs, path::PathBuf};

use crate::Result;

const HERMES_HOME_ENV: &str = "HERMES_HOME";

pub(super) fn hermes_state_db_paths() -> Result<Vec<PathBuf>> {
    let (homes, discover_profiles) = if let Ok(paths) = env::var(HERMES_HOME_ENV) {
        (
            paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect::<Vec<_>>(),
            false,
        )
    } else {
        let home =
            crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
        (vec![home.join(".hermes")], true)
    };
    let mut paths = Vec::new();
    for home in homes {
        paths.push(home.join("state.db"));
        if discover_profiles {
            let mut profile_paths = fs::read_dir(home.join("profiles"))
                .map(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .map(|entry| entry.path().join("state.db"))
                        .filter(|path| path.is_file())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            profile_paths.sort();
            paths.extend(profile_paths);
        }
    }
    let mut seen = HashSet::new();
    Ok(paths
        .into_iter()
        .filter(|path| path.is_file())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    #[test]
    fn discovers_default_and_named_profile_databases() {
        let fixture = fs_fixture!({
            ".hermes/state.db": "",
            ".hermes/profiles/work/state.db": "",
            ".hermes/profiles/personal/state.db": "",
            ".hermes/profiles/ignored/not-state.db": "",
        });
        let _env_guard = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (HERMES_HOME_ENV, None),
        ]);

        let paths = hermes_state_db_paths().unwrap();

        assert_eq!(
            paths,
            vec![
                fixture.path(".hermes/state.db"),
                fixture.path(".hermes/profiles/personal/state.db"),
                fixture.path(".hermes/profiles/work/state.db"),
            ]
        );
    }

    #[test]
    fn discovers_named_profiles_without_a_default_database() {
        let fixture = fs_fixture!({
            ".hermes/profiles/work/state.db": "",
        });
        let _env_guard = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (HERMES_HOME_ENV, None),
        ]);

        let paths = hermes_state_db_paths().unwrap();

        assert_eq!(paths, vec![fixture.path(".hermes/profiles/work/state.db")]);
    }

    #[test]
    fn explicit_homes_are_authoritative_and_deduplicated() {
        let fixture = fs_fixture!({
            "first/state.db": "",
            "first/profiles/ignored/state.db": "",
            "second/state.db": "",
        });
        let homes = [
            fixture.path("first"),
            fixture.path("second"),
            fixture.path("first"),
        ]
        .map(|path| path.display().to_string())
        .join(",");
        let _env_guard = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (HERMES_HOME_ENV, Some(OsString::from(homes))),
        ]);

        let paths = hermes_state_db_paths().unwrap();

        assert_eq!(
            paths,
            vec![
                fixture.path("first/state.db"),
                fixture.path("second/state.db")
            ]
        );
    }
}
