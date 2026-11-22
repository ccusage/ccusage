use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;
use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

pub(super) const DROID_SESSIONS_DIR_ENV: &str = "DROID_SESSIONS_DIR";

// NOTE(2026-05-19, #1028): Factory Droid stores cumulative session counters in .settings.json snapshots alongside its JSONL transcripts. Read the snapshots as the source of token totals.
pub(super) fn discover_settings_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in droid_session_paths()? {
        collect_files_with_extension(&root, "json", &mut files, diagnostics);
    }
    Ok(files
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".settings.json"))
        })
        .collect())
}

fn droid_session_paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(
        RootsEnv::List(DROID_SESSIONS_DIR_ENV),
        || {
            crate::cli::home::home_dir()
                .into_iter()
                .flat_map(|home| [".factory/sessions"].map(|path| home.join(path)))
                .collect()
        },
    ))
}
