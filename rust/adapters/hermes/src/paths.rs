use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const HERMES_HOME_ENV: &str = "HERMES_HOME";

pub(super) fn hermes_state_db_paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(HERMES_HOME_ENV), || {
        crate::cli::home::home_dir()
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
    fn configured_homes_preserve_list_order_and_deduplicate() {
        let fixture = ccusage_test_support::fs_fixture!({
            "first/state.db": "",
            "second/state.db": "",
        });
        ccusage_test_support::Environment::with_var(
            HERMES_HOME_ENV,
            format!(
                " {}, {}, {} ",
                fixture.path("first").display(),
                fixture.path("second").display(),
                fixture.path("first/.").display()
            ),
        )
        .run(|| {
            assert_eq!(
                hermes_state_db_paths().unwrap(),
                vec![
                    fixture.path("first/state.db"),
                    fixture.path("second/state.db")
                ]
            );
        })
    }
}
