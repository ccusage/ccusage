use std::{
    collections::HashSet,
    env,
    path::{Component, Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

const QWEN_DATA_DIR_ENV: &str = "QWEN_DATA_DIR";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let candidates = if let Ok(paths) = env::var(QWEN_DATA_DIR_ENV) {
        paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    } else {
        crate::home::home_dir()
            .map(|home| vec![home.join(".qwen")])
            .unwrap_or_default()
    };
    let mut seen = HashSet::new();
    Ok(candidates
        .into_iter()
        .filter(|path| path.is_dir())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

pub(super) fn discover_chat_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in paths()? {
        let projects = root.join("projects");
        if !projects.is_dir() {
            continue;
        }
        let mut root_files = Vec::new();
        collect_files_with_extension(&projects, "jsonl", &mut root_files);
        root_files.retain(|file| is_chat_file(&projects, file));
        files.extend(root_files);
    }
    files.sort();
    Ok(files)
}

fn is_chat_file(projects: &Path, file: &Path) -> bool {
    let Ok(relative) = file.strip_prefix(projects) else {
        return false;
    };
    let parts = relative.components().collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        [Component::Normal(project), Component::Normal(chats), Component::Normal(file)]
            if !project.is_empty()
                && chats.to_str() == Some("chats")
                && file.to_string_lossy().ends_with(".jsonl")
    )
}

pub(super) fn project_from_file(file: &Path) -> Option<String> {
    let parts = file.components().collect::<Vec<_>>();
    for window in parts.windows(4).rev() {
        if let [
            Component::Normal(projects),
            Component::Normal(project),
            Component::Normal(chats),
            Component::Normal(_),
        ] = window
            && projects.to_str() == Some("projects")
            && chats.to_str() == Some("chats")
        {
            return Some(project.to_string_lossy().into_owned());
        }
    }
    None
}
