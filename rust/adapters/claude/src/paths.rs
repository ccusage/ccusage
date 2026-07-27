use std::{
    env,
    path::{Path, PathBuf},
};

use memchr::memmem;

use crate::{Result, cli_error, fast::FxHashSet, home, path_utils::expand_home_path};
use crate::{TimestampMs, parse_ts_timestamp};
use ccusage_adapter_common::collect_usage_files;

pub fn claude_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = FxHashSet::default();
    if let Ok(env_paths) = env::var("CLAUDE_CONFIG_DIR") {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = normalize_claude_config_path(raw);
            if path.join("projects").is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        if !paths.is_empty() {
            return Ok(paths);
        }
        return Err(cli_error(format!(
            "No valid Claude data directories found in CLAUDE_CONFIG_DIR. Expected each path to be a Claude config directory containing 'projects/', or the 'projects/' directory itself: {env_paths}"
        )));
    }

    let home = home::home_dir().ok_or_else(|| cli_error("home directory is not set"))?;
    let xdg = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".config"));
    for path in [xdg.join("claude"), home.join(".claude")] {
        if path.join("projects").is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn normalize_claude_config_path(raw: &str) -> PathBuf {
    let path = expand_home_path(raw);
    if path.file_name().is_some_and(|name| name == "projects") && path.is_dir() {
        return path.parent().map(Path::to_path_buf).unwrap_or(path);
    }
    path
}

pub fn usage_files(paths: &[PathBuf], project_filter: Option<&str>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        let projects_dir = path.join("projects");
        if let Some(project_filter) =
            project_filter.filter(|filter| is_project_path_segment(filter))
        {
            collect_usage_files(&projects_dir.join(project_filter), &mut files);
        } else {
            collect_usage_files(&projects_dir, &mut files);
        }
    }
    files.sort_by_cached_key(|path| path.to_string_lossy().into_owned());
    files
}

pub(super) fn is_project_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

#[doc(hidden)]
pub fn timestamp_from_line(line: &str) -> Option<TimestampMs> {
    timestamp_from_line_bytes(line.as_bytes())
}

fn timestamp_from_line_bytes(line: &[u8]) -> Option<TimestampMs> {
    let marker = br#""timestamp":""#;
    let start = memmem::find(line, marker)? + marker.len();
    let end = memchr::memchr(b'"', &line[start..])? + start;
    let timestamp = std::str::from_utf8(&line[start..end]).ok()?;
    parse_ts_timestamp(timestamp)
}

pub fn extract_project(path: &Path) -> String {
    let mut saw_projects = false;
    for part in path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
    {
        if saw_projects {
            return if part.trim().is_empty() {
                "unknown"
            } else {
                part
            }
            .to_string();
        }
        if part == "projects" {
            saw_projects = true;
        }
    }
    "unknown".to_string()
}

pub fn extract_session_parts(path: &Path) -> (String, String) {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let projects_index = parts.iter().position(|part| *part == "projects");
    let relative = projects_index
        .map(|index| &parts[index + 1..])
        .unwrap_or(&parts);
    let file_session_id = relative
        .last()
        .and_then(|file_name| file_name.strip_suffix(".jsonl"))
        .filter(|session_id| !session_id.is_empty());
    if relative.len() == 2
        && let Some(session_id) = file_session_id
    {
        return (session_id.to_string(), relative[0].to_string());
    }
    if relative.len() >= 4 && relative.get(relative.len() - 2) == Some(&"subagents") {
        let session_id = relative[relative.len() - 3].to_string();
        let project_path = relative[..relative.len() - 3].join(std::path::MAIN_SEPARATOR_STR);
        return (
            session_id,
            if project_path.is_empty() {
                "Unknown Project".to_string()
            } else {
                project_path
            },
        );
    }
    let session_id = relative
        .get(relative.len().saturating_sub(2))
        .copied()
        .unwrap_or("unknown")
        .to_string();
    let project_path = if relative.len() > 2 {
        relative[..relative.len() - 2].join(std::path::MAIN_SEPARATOR_STR)
    } else {
        "Unknown Project".to_string()
    };
    (session_id, project_path)
}
