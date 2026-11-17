use std::path::{Path, PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;

pub(super) const KILO_DATA_DIR_ENV: &str = "KILO_DATA_DIR";
pub(super) const KILO_DB_FILE_NAME: &str = "kilo.db";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(KILO_DATA_DIR_ENV), || {
        crate::home::home_dir()
            .into_iter()
            .flat_map(|home| [".local/share/kilo"].map(|path| home.join(path)))
            .collect()
    }))
}

pub(super) fn db_path(kilo_dir: &Path) -> Option<PathBuf> {
    let path = kilo_dir.join(KILO_DB_FILE_NAME);
    path.is_file().then_some(path)
}
