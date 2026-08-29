use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

const OPENCODE_DATA_DIR_ENV: &str = "OPENCODE_DATA_DIR";
const XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(OPENCODE_DATA_DIR_ENV) {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(raw);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        return Ok(paths);
    }

    let data_home = match env::var_os(XDG_DATA_HOME_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        Some(path) => path,
        None => {
            let home = crate::home::home_dir()
                .ok_or_else(|| crate::cli_error("home directory is not set"))?;
            home.join(".local/share")
        }
    };
    let path = data_home.join("opencode");
    if path.is_dir() && seen.insert(path.clone()) {
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::{OPENCODE_DATA_DIR_ENV, XDG_DATA_HOME_ENV, paths};

    fn isolated_env(
        opencode_data_dir: Option<OsString>,
        xdg_data_home: Option<OsString>,
        home: Option<OsString>,
    ) -> EnvVarsGuard {
        EnvVarsGuard::set_many([
            (OPENCODE_DATA_DIR_ENV, opencode_data_dir),
            (XDG_DATA_HOME_ENV, xdg_data_home),
            ("HOME", home),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
    }

    #[test]
    fn uses_xdg_data_home_for_default_path() {
        let fixture = fs_fixture!({
            "xdg/opencode/opencode.db": "",
            "home/.local/share/opencode/opencode.db": "",
        });
        let _guard = isolated_env(
            None,
            Some(fixture.path("xdg").into_os_string()),
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(paths().unwrap(), vec![fixture.path("xdg/opencode")]);
    }

    #[test]
    fn uses_xdg_data_home_without_a_home_directory() {
        let fixture = fs_fixture!({
            "xdg/opencode/opencode.db": "",
        });
        let _guard = isolated_env(None, Some(fixture.path("xdg").into_os_string()), None);

        assert_eq!(paths().unwrap(), vec![fixture.path("xdg/opencode")]);
    }

    #[test]
    fn keeps_configured_data_dirs_before_default_paths() {
        let fixture = fs_fixture!({
            "configured/first/opencode.db": "",
            "configured/second/opencode.db": "",
            "xdg/opencode/opencode.db": "",
            "home/.local/share/opencode/opencode.db": "",
        });
        let first = fixture.path("configured/first");
        let second = fixture.path("configured/second");
        let raw = format!(
            " {}, {}, {}, {} ",
            first.display(),
            second.display(),
            first.display(),
            fixture.path("missing").display()
        );
        let _guard = isolated_env(
            Some(OsString::from(raw)),
            Some(fixture.path("xdg").into_os_string()),
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(paths().unwrap(), vec![first, second]);
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_unset() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        let _guard = isolated_env(None, None, Some(fixture.path("home").into_os_string()));

        assert_eq!(
            paths().unwrap(),
            vec![fixture.path("home/.local/share/opencode")]
        );
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_empty() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        let _guard = isolated_env(
            None,
            Some(OsString::new()),
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(
            paths().unwrap(),
            vec![fixture.path("home/.local/share/opencode")]
        );
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_relative() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        let _guard = isolated_env(
            None,
            Some(OsString::from("relative-xdg-data-home")),
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(
            paths().unwrap(),
            vec![fixture.path("home/.local/share/opencode")]
        );
    }

    #[test]
    fn does_not_fall_back_to_home_when_xdg_data_home_is_set() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        let xdg_data_home = fixture.create_dir_all("xdg");
        let _guard = isolated_env(
            None,
            Some(xdg_data_home.into_os_string()),
            Some(fixture.path("home").into_os_string()),
        );

        assert!(paths().unwrap().is_empty());
    }
}
