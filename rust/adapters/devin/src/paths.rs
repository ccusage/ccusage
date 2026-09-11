use std::{collections::HashSet, env, path::PathBuf};

use crate::Result;

pub(super) const DEVIN_TRANSCRIPTS_DIR_ENV: &str = "DEVIN_TRANSCRIPTS_DIR";
const XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    if let Some(env_paths) = env::var_os(DEVIN_TRANSCRIPTS_DIR_ENV) {
        let configured_paths = match env_paths.into_string() {
            Ok(env_paths) => env_paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect(),
            Err(env_path) => vec![PathBuf::from(env_path)],
        };
        return Ok(dedupe_dirs(configured_paths));
    }

    let mut roots = Vec::new();
    // Windows stores the CLI data root under %APPDATA% instead of the
    // XDG-style ~/.local/share path used on macOS and Linux.
    if let Some(appdata) = env::var_os("APPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        roots.push(appdata);
    }
    match env::var_os(XDG_DATA_HOME_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        Some(path) => roots.push(path),
        None => {
            if let Some(home) = crate::home::home_dir() {
                roots.push(home.join(".local/share"));
            }
        }
    };
    if roots.is_empty() {
        return Err(crate::cli_error("data directory is not set"));
    }
    // The CLI migrated its data root from cognition/ to devin/ and left a
    // compatibility symlink at the old path, so both are candidates and the
    // canonical dedupe keeps a symlinked legacy dir from counting twice.
    Ok(dedupe_dirs(
        roots
            .into_iter()
            .flat_map(|root| {
                [
                    root.join("devin/cli/transcripts"),
                    root.join("cognition/cli/transcripts"),
                ]
            })
            .collect(),
    ))
}

fn dedupe_dirs(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| path.canonicalize().unwrap_or(path))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::{DEVIN_TRANSCRIPTS_DIR_ENV, XDG_DATA_HOME_ENV, paths};

    fn isolated_env(
        devin_transcripts_dir: Option<OsString>,
        xdg_data_home: Option<OsString>,
        home: Option<OsString>,
    ) -> EnvVarsGuard {
        EnvVarsGuard::set_many([
            (DEVIN_TRANSCRIPTS_DIR_ENV, devin_transcripts_dir),
            (XDG_DATA_HOME_ENV, xdg_data_home),
            ("HOME", home),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            ("APPDATA", None),
        ])
    }

    #[test]
    fn uses_xdg_data_home_for_default_path() {
        let fixture = fs_fixture!({
            "xdg/devin/cli/transcripts/session.json": "{}",
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
        });
        let _guard = isolated_env(
            None,
            Some(fixture.path("xdg").into_os_string()),
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(
            paths().unwrap(),
            vec![
                fixture
                    .path("xdg/devin/cli/transcripts")
                    .canonicalize()
                    .unwrap()
            ]
        );
    }

    #[test]
    fn includes_appdata_transcripts_dir() {
        let fixture = fs_fixture!({
            "appdata/devin/cli/transcripts/session.json": "{}",
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
        });
        let _guard = EnvVarsGuard::set_many([
            (DEVIN_TRANSCRIPTS_DIR_ENV, None),
            (XDG_DATA_HOME_ENV, None),
            ("HOME", Some(fixture.path("home").into_os_string())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            ("APPDATA", Some(fixture.path("appdata").into_os_string())),
        ]);

        let paths = paths().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("appdata/devin/cli/transcripts"));
        assert!(paths[1].ends_with(".local/share/devin/cli/transcripts"));
    }

    #[test]
    fn includes_legacy_cognition_transcripts_dir() {
        let fixture = fs_fixture!({
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
            "home/.local/share/cognition/cli/transcripts/legacy.json": "{}",
        });
        let _guard = isolated_env(None, None, Some(fixture.path("home").into_os_string()));

        let paths = paths().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(
            paths[0].ends_with("devin/cli/transcripts")
                && paths[1].ends_with("cognition/cli/transcripts")
        );
    }

    #[cfg(unix)]
    #[test]
    fn dedupes_legacy_cognition_symlink() {
        let fixture = fs_fixture!({
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
        });
        let cognition = fixture.create_dir_all("home/.local/share/cognition/cli");
        std::os::unix::fs::symlink(
            fixture.path("home/.local/share/devin/cli/transcripts"),
            cognition.join("transcripts"),
        )
        .unwrap();
        let _guard = isolated_env(None, None, Some(fixture.path("home").into_os_string()));

        assert_eq!(paths().unwrap().len(), 1);
    }

    #[test]
    fn keeps_configured_dirs_before_default_paths() {
        let fixture = fs_fixture!({
            "configured/first/a.json": "{}",
            "configured/second/b.json": "{}",
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
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
            None,
            Some(fixture.path("home").into_os_string()),
        );

        assert_eq!(
            paths().unwrap(),
            vec![
                first.canonicalize().unwrap(),
                second.canonicalize().unwrap()
            ]
        );
    }

    #[test]
    fn does_not_fall_back_when_configured_dir_is_empty() {
        let fixture = fs_fixture!({
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
        });
        let _guard = isolated_env(
            Some(OsString::new()),
            None,
            Some(fixture.path("home").into_os_string()),
        );

        assert!(paths().unwrap().is_empty());
    }

    #[test]
    fn falls_back_to_home_data_directory_when_xdg_data_home_is_unset() {
        let fixture = fs_fixture!({
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
        });
        let _guard = isolated_env(None, None, Some(fixture.path("home").into_os_string()));

        assert_eq!(paths().unwrap().len(), 1);
        assert!(paths().unwrap()[0].ends_with("devin/cli/transcripts"));
    }

    #[test]
    fn does_not_fall_back_to_home_when_xdg_data_home_is_set() {
        let fixture = fs_fixture!({
            "home/.local/share/devin/cli/transcripts/session.json": "{}",
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
