use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const HERMES_HOME_ENV: &str = "HERMES_HOME";

pub(super) fn hermes_state_db_paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::Single(HERMES_HOME_ENV), || {
        crate::home::home_dir()
            .map(|home| home.join(".hermes"))
            .into_iter()
            .collect()
    })
    .into_iter()
    .map(|home| home.join("state.db"))
    .filter(|path| path.is_file())
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_home_is_one_root_even_with_commas() {
        let fixture = ccusage_test_support::fs_fixture!({
            "first,second/state.db": "",
        });
        let _guard = ccusage_test_support::EnvVarGuard::set(
            HERMES_HOME_ENV,
            format!(" {} ", fixture.path("first,second").display()),
        );

        assert_eq!(
            hermes_state_db_paths().unwrap(),
            vec![fixture.path("first,second/state.db")]
        );
    }
}
