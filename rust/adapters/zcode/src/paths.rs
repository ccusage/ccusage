use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use crate::Result;

pub(super) const ZCODE_HOME_ENV: &str = "ZCODE_HOME";
pub(super) const ZCODE_DB_RELATIVE_PATH: &str = "cli/db/db.sqlite";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(ZCODE_HOME_ENV)
        && !env_paths.trim().is_empty()
    {
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

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".zcode");
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub(super) fn db_path(zcode_home: &Path) -> Option<PathBuf> {
    let path = zcode_home.join(ZCODE_DB_RELATIVE_PATH);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::*;

    #[test]
    fn empty_zcode_home_falls_back_to_default_home() {
        let fixture = fs_fixture!({ ".zcode/placeholder": "" });
        let home = OsString::from(fixture.root().as_os_str());
        let _guard = EnvVarsGuard::set_many([
            (ZCODE_HOME_ENV, Some(OsString::from("  "))),
            ("HOME", Some(home.clone())),
            ("USERPROFILE", Some(home)),
        ]);

        assert_eq!(paths().unwrap(), vec![fixture.path(".zcode")]);
    }
}
