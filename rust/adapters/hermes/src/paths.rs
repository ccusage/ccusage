use std::{
    collections::HashSet,
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

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
            paths.extend(profile_state_db_paths(&home)?);
        }
    }
    let mut seen = HashSet::new();
    Ok(paths
        .into_iter()
        .filter(|path| path.is_file())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

fn profile_state_db_paths(home: &Path) -> Result<Vec<PathBuf>> {
    let profiles_dir = home.join("profiles");
    let entries = match fs::read_dir(&profiles_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(crate::cli_error(format!(
                "failed to read Hermes profiles directory {}: {error}",
                profiles_dir.display()
            )));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            crate::cli_error(format!(
                "failed to read an entry in Hermes profiles directory {}: {error}",
                profiles_dir.display()
            ))
        })?;
        let state_db = entry.path().join("state.db");
        if state_db.is_file() {
            paths.push(state_db);
        }
    }
    paths.sort();
    Ok(paths)
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
        let _ = fixture.create_dir_all(".hermes/profiles/directory-db/state.db");
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
    fn reports_non_missing_profile_directory_errors() {
        let fixture = fs_fixture!({
            ".hermes/profiles": "not a directory",
        });
        let _env_guard = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (HERMES_HOME_ENV, None),
        ]);

        assert!(hermes_state_db_paths().is_err());
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
