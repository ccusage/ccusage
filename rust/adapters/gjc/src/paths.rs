use std::{env, path::PathBuf};

use crate::{Result, collect_files_with_extension};

pub const GJC_CONFIG_DIR_ENV: &str = "GJC_CONFIG_DIR";

pub(super) fn discover_session_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_with_extension(&gjc_sessions_dir()?, "jsonl", &mut files);
    files.sort();
    Ok(files)
}

fn gjc_sessions_dir() -> Result<PathBuf> {
    let config_dir = match env::var(GJC_CONFIG_DIR_ENV) {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => crate::home::home_dir()
            .ok_or_else(|| crate::cli_error("home directory is not set"))?
            .join(".gjc"),
    };
    Ok(config_dir.join("agent").join("sessions"))
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::*;

    #[test]
    fn discovers_jsonl_files_under_gjc_config_dir() {
        let fixture = fs_fixture!({
            "agent/sessions/project/session.jsonl": "{}\n",
            "agent/sessions/project/metadata.json": "{}",
        });
        let _guard = EnvVarGuard::set(GJC_CONFIG_DIR_ENV, fixture.root());

        let files = discover_session_files().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "session.jsonl");
    }
}
