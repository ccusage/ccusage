use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

const AMP_DATA_DIR_ENV: &str = "AMP_DATA_DIR";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(AMP_DATA_DIR_ENV), || {
        crate::cli::home::home_dir()
            .into_iter()
            .flat_map(|home| [".local/share/amp"].map(|path| home.join(path)))
            .collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn blank_data_dir_falls_back_to_home() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all(".local/share/amp");
        Environment::with_vars([
            (AMP_DATA_DIR_ENV, Some(" , ".into())),
            ("HOME", Some(fixture.root().into())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert_eq!(paths().unwrap(), vec![root]);
        })
    }

    #[test]
    fn missing_home_returns_no_roots() {
        Environment::with_vars([
            (AMP_DATA_DIR_ENV, None),
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
        ])
        .run(|| {
            assert_eq!(paths().unwrap(), Vec::<PathBuf>::new());
        })
    }
}
