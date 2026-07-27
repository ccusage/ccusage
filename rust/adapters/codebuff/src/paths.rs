use std::{collections::HashSet, env, path::PathBuf};

use crate::{Result, collect_files_with_extension};

pub(super) const CODEBUFF_DATA_DIR_ENV: &str = "CODEBUFF_DATA_DIR";
const CHANNELS: &[&str] = &["manicode", "manicode-dev", "manicode-staging"];

pub(super) fn discover_chat_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in codebuff_project_roots()? {
        collect_files_with_extension(&root, "json", &mut files);
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
    let roots = if let Ok(paths) = env::var(CODEBUFF_DATA_DIR_ENV) {
        paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    } else {
        let home =
            crate::home::home_dir().ok_or_else(|| crate::cli_error("home directory is not set"))?;
        CHANNELS
            .iter()
            .map(|channel| home.join(".config").join(channel))
            .collect()
    };
    let mut seen = HashSet::new();
    let mut project_roots = Vec::new();
    for root in roots {
        let project_root = if root.file_name().is_some_and(|name| name == "projects") {
            root
        } else {
            root.join("projects")
        };
        if project_root.is_dir() && seen.insert(project_root.clone()) {
            project_roots.push(project_root);
        }
    }
    Ok(project_roots)
}
