use std::{collections::HashSet, env, path::PathBuf};

use crate::{Result, collect_files_with_extension};

pub const COPILOT_OTEL_FILE_EXPORTER_PATH_ENV: &str = "COPILOT_OTEL_FILE_EXPORTER_PATH";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    if let Some(home) = crate::home::home_dir() {
        let default_dir = home.join(".copilot").join("otel");
        if default_dir.is_dir() {
            collect_files_with_extension(&default_dir, "jsonl", &mut files);
        }
    }
    if let Some(path) = copilot_exporter_path() {
        files.push(path);
    }
    files.retain(|path| seen.insert(path.clone()));
    files.sort();
    Ok(files)
}

fn copilot_exporter_path() -> Option<PathBuf> {
    let path = env::var(COPILOT_OTEL_FILE_EXPORTER_PATH_ENV).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}
