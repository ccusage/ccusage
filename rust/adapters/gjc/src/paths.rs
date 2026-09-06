use std::{
    env,
    path::{Component, Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

pub const GJC_CONFIG_DIR_ENV: &str = "GJC_CONFIG_DIR";
pub const GJC_CODING_AGENT_DIR_ENV: &str = "GJC_CODING_AGENT_DIR";
const DEFAULT_CONFIG_DIR_NAME: &str = ".gjc";

pub(super) fn discover_session_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_with_extension(&gjc_sessions_dir()?, "jsonl", &mut files);
    files.sort();
    Ok(files)
}

fn gjc_sessions_dir() -> Result<PathBuf> {
    if let Some(agent_dir) = non_empty_env_path(GJC_CODING_AGENT_DIR_ENV) {
        return Ok(agent_dir.join("sessions"));
    }
    if let Some(sessions_dir) = xdg_sessions_dir() {
        return Ok(sessions_dir);
    }
    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(home_sessions_dir(&home))
}

fn home_sessions_dir(home: &Path) -> PathBuf {
    home.join(config_dir_name()).join("agent").join("sessions")
}

fn xdg_sessions_dir() -> Option<PathBuf> {
    let xdg_data_home = non_empty_env_path("XDG_DATA_HOME")?;
    let xdg_root = xdg_data_home.join("gjc");
    (xdg_data_home.is_absolute() && xdg_root.is_dir()).then(|| xdg_root.join("sessions"))
}

fn config_dir_name() -> PathBuf {
    let Some(path) = non_empty_env_path(GJC_CONFIG_DIR_ENV) else {
        return PathBuf::from(DEFAULT_CONFIG_DIR_NAME);
    };
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return PathBuf::from(DEFAULT_CONFIG_DIR_NAME);
    }
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect()
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    let value = env::var_os(name)?;
    if value.is_empty() {
        return None;
    }
    if let Some(value) = value.to_str() {
        let value = value.trim();
        return (!value.is_empty()).then(|| PathBuf::from(value));
    }
    Some(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    use super::*;

    #[test]
    fn discovers_jsonl_files_under_gjc_config_dir() {
        let fixture = fs_fixture!({
            "sessions/project/session.jsonl": "{}\n",
            "sessions/project/metadata.json": "{}",
        });
        let _guard = EnvVarGuard::set(GJC_CODING_AGENT_DIR_ENV, fixture.root());

        let files = discover_session_files().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "session.jsonl");
    }

    #[test]
    fn resolves_config_directory_names_below_home() {
        let fixture = fs_fixture!({});
        let _guard = EnvVarsGuard::set_many([
            (GJC_CONFIG_DIR_ENV, Some(OsString::from(".gjc-alt"))),
            ("XDG_DATA_HOME", None),
        ]);

        assert_eq!(
            home_sessions_dir(fixture.root()),
            fixture.root().join(".gjc-alt/agent/sessions")
        );
    }

    #[test]
    fn rejects_config_directory_names_that_escape_home() {
        let fixture = fs_fixture!({});
        let _guard = EnvVarsGuard::set_many([
            (GJC_CONFIG_DIR_ENV, Some(OsString::from("../outside"))),
            ("XDG_DATA_HOME", None),
        ]);

        assert_eq!(
            home_sessions_dir(fixture.root()),
            fixture.root().join(".gjc/agent/sessions")
        );
    }

    #[test]
    fn ignores_relative_xdg_data_home() {
        let _fixture = fs_fixture!({
            "relative/gjc/sessions/session.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([
            (GJC_CONFIG_DIR_ENV, None),
            ("XDG_DATA_HOME", Some(OsString::from("relative"))),
        ]);

        assert!(xdg_sessions_dir().is_none());
    }

    #[test]
    fn prefers_existing_absolute_xdg_data_home() {
        let fixture = fs_fixture!({
            "gjc/sessions/session.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([
            (GJC_CONFIG_DIR_ENV, None),
            (
                "XDG_DATA_HOME",
                Some(fixture.root().as_os_str().to_os_string()),
            ),
        ]);

        assert_eq!(
            xdg_sessions_dir(),
            Some(fixture.root().join("gjc/sessions"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_agent_directory_overrides() {
        use std::os::unix::ffi::OsStringExt;

        let value = OsString::from_vec(b"/tmp/gjc-\xFF".to_vec());
        let _guard = EnvVarGuard::set(GJC_CODING_AGENT_DIR_ENV, &value);

        assert_eq!(
            gjc_sessions_dir().unwrap(),
            PathBuf::from(value).join("sessions")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_config_directory_names() {
        use std::os::unix::ffi::OsStringExt;

        let value = OsString::from_vec(b".gjc-\xFF".to_vec());
        let _guard = EnvVarsGuard::set_many([
            (GJC_CONFIG_DIR_ENV, Some(value.clone())),
            ("XDG_DATA_HOME", None),
        ]);

        assert_eq!(
            home_sessions_dir(Path::new("/home/user")),
            PathBuf::from("/home/user")
                .join(value)
                .join("agent/sessions")
        );
    }
}
