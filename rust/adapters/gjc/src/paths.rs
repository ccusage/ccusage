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
    if let Some(agent_dir) = non_empty_env(GJC_CODING_AGENT_DIR_ENV) {
        return Ok(PathBuf::from(agent_dir).join("sessions"));
    }
    let home =
        crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
    Ok(gjc_sessions_dir_for_home(&home))
}

fn gjc_sessions_dir_for_home(home: &Path) -> PathBuf {
    if let Some(xdg_data_home) = non_empty_env("XDG_DATA_HOME") {
        let xdg_root = PathBuf::from(xdg_data_home).join("gjc");
        if xdg_root.is_dir() {
            return xdg_root.join("sessions");
        }
    }
    home.join(config_dir_name()).join("agent").join("sessions")
}

fn config_dir_name() -> PathBuf {
    let Some(value) = non_empty_env(GJC_CONFIG_DIR_ENV) else {
        return PathBuf::from(DEFAULT_CONFIG_DIR_NAME);
    };
    let path = Path::new(&value);
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

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
            gjc_sessions_dir_for_home(fixture.root()),
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
            gjc_sessions_dir_for_home(fixture.root()),
            fixture.root().join(".gjc/agent/sessions")
        );
    }
}
