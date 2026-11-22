use std::path::PathBuf;

use ccusage_core::cli::environment::Environment;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const OPENCODE_DATA_DIR_ENV: &str = "OPENCODE_DATA_DIR";
const XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(
        RootsEnv::List(OPENCODE_DATA_DIR_ENV),
        || {
            let data_home = Environment::current()
                .var_os(XDG_DATA_HOME_ENV)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| crate::cli::home::home_dir().map(|home| home.join(".local/share")));
            data_home
                .map(|home| home.join("opencode"))
                .into_iter()
                .collect()
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{Environment, fs_fixture};

    use super::{OPENCODE_DATA_DIR_ENV, XDG_DATA_HOME_ENV, paths};

    fn isolated_env(
        opencode_data_dir: Option<OsString>,
        xdg_data_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Environment {
        Environment::with_vars([
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
        isolated_env(
            None,
            Some(fixture.path("xdg").into_os_string()),
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert_eq!(paths().unwrap(), vec![fixture.path("xdg/opencode")]);
        })
    }

    #[test]
    fn uses_xdg_data_home_without_a_home_directory() {
        let fixture = fs_fixture!({
            "xdg/opencode/opencode.db": "",
        });
        isolated_env(None, Some(fixture.path("xdg").into_os_string()), None).run(|| {
            assert_eq!(paths().unwrap(), vec![fixture.path("xdg/opencode")]);
        })
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
        isolated_env(
            Some(OsString::from(raw)),
            Some(fixture.path("xdg").into_os_string()),
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert_eq!(paths().unwrap(), vec![first, second]);
        })
    }

    #[cfg(unix)]
    #[test]
    fn does_not_fall_back_when_configured_data_dir_is_non_utf8() {
        use std::os::unix::ffi::OsStringExt;

        isolated_env(
            Some(OsString::from_vec(b"opencode-\xFF".to_vec())),
            None,
            None,
        )
        .run(|| {
            assert!(paths().unwrap().is_empty());
        })
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovers_non_utf8_configured_data_directory() {
        use std::{os::unix::ffi::OsStringExt, path::PathBuf};

        let fixture = fs_fixture!({});
        let configured_dir =
            fixture.create_dir_all(PathBuf::from(OsString::from_vec(b"opencode-\xFF".to_vec())));
        isolated_env(Some(configured_dir.clone().into_os_string()), None, None).run(|| {
            assert_eq!(paths().unwrap(), vec![configured_dir]);
        })
    }

    #[test]
    fn falls_back_when_configured_data_dir_is_empty() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        isolated_env(
            Some(OsString::new()),
            None,
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert_eq!(
                paths().unwrap(),
                vec![fixture.path("home/.local/share/opencode")]
            );
        })
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_unset() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        isolated_env(None, None, Some(fixture.path("home").into_os_string())).run(|| {
            assert_eq!(
                paths().unwrap(),
                vec![fixture.path("home/.local/share/opencode")]
            );
        })
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_empty() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        isolated_env(
            None,
            Some(OsString::new()),
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert_eq!(
                paths().unwrap(),
                vec![fixture.path("home/.local/share/opencode")]
            );
        })
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_relative() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        isolated_env(
            None,
            Some(OsString::from("relative-xdg-data-home")),
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert_eq!(
                paths().unwrap(),
                vec![fixture.path("home/.local/share/opencode")]
            );
        })
    }

    #[test]
    fn does_not_fall_back_to_home_when_xdg_data_home_is_set() {
        let fixture = fs_fixture!({
            "home/.local/share/opencode/opencode.db": "",
        });
        let xdg_data_home = fixture.create_dir_all("xdg");
        isolated_env(
            None,
            Some(xdg_data_home.into_os_string()),
            Some(fixture.path("home").into_os_string()),
        )
        .run(|| {
            assert!(paths().unwrap().is_empty());
        })
    }
}
