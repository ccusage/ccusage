use std::path::PathBuf;

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;
use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

pub(super) const CODEBUFF_DATA_DIR_ENV: &str = "CODEBUFF_DATA_DIR";
// NOTE(2026-05-19, #1029): Codebuff stores chat-messages.json under Manicode-branded channel roots. Discovery must include those names to find production, development and staging history.
const CHANNELS: &[&str] = &["manicode", "manicode-dev", "manicode-staging"];

pub(super) fn discover_chat_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in codebuff_project_roots()? {
        collect_files_with_extension(&root, "json", &mut files, diagnostics);
    }
    Ok(files
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "chat-messages.json")
        })
        .collect())
}

fn codebuff_project_roots() -> Result<Vec<PathBuf>> {
    Ok(roots::directories(
        RootsEnv::List(CODEBUFF_DATA_DIR_ENV)
            .candidates(|| {
                crate::cli::home::home_dir()
                    .into_iter()
                    .flat_map(|home| {
                        CHANNELS
                            .iter()
                            .map(move |channel| home.join(".config").join(channel))
                    })
                    .collect()
            })
            .into_iter()
            .map(|root| {
                if root.file_name().is_some_and(|name| name == "projects") {
                    root
                } else {
                    root.join("projects")
                }
            }),
    ))
}
