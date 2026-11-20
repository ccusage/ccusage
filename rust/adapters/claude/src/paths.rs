use std::{
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Deserialize;

use crate::{
    Diagnostics, MILLIS_PER_DAY, Result, TimestampMs,
    cli::{SharedArgs, home},
    cli_error, date_range_bounds_ms,
    fast::{FxHashMap, FxHashSet, LinePrefilter},
    resolve_tz,
};
use ccusage_adapter_common::{
    jsonl::{self, ReadPolicy},
    read_discovered_paths, read_paths_discovered_in_parallel,
    visit_usage_files as visit_usage_files_in, visit_usage_files_in_parallel,
};

pub(crate) fn claude_paths() -> Result<Vec<PathBuf>> {
    let (env_value, dirs) = match home::claude_config_dirs() {
        home::ClaudeConfigDirs::Env { value, dirs } => (Some(value), dirs),
        home::ClaudeConfigDirs::Default(dirs) => (None, dirs),
        home::ClaudeConfigDirs::NoHome => return Err(cli_error("home directory is not set")),
    };
    let paths: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|path| path.join("projects").is_dir())
        .collect();
    if let Some(env_paths) = env_value
        && paths.is_empty()
    {
        return Err(cli_error(format!(
            "No valid Claude data directories found in CLAUDE_CONFIG_DIR. Expected each path to be a Claude config directory containing 'projects/', or the 'projects/' directory itself: {env_paths}"
        )));
    }
    Ok(paths)
}

#[cfg(test)]
pub(crate) fn usage_files(
    paths: &[PathBuf],
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    visit_usage_files(
        paths,
        project_filter,
        &mut |path| files.push(path),
        diagnostics,
    );
    files.sort_by_cached_key(|path| usage_file_order(path));
    files
}

fn visit_usage_files(
    paths: &[PathBuf],
    project_filter: Option<&str>,
    visit: &mut dyn FnMut(PathBuf),
    diagnostics: &Diagnostics,
) {
    let mut filtered_visit = |path: PathBuf| {
        if matches_project(&path, project_filter) {
            visit(path);
        }
    };
    for path in paths {
        visit_usage_files_in(
            &projects_dir(path, project_filter),
            &mut filtered_visit,
            diagnostics,
        );
    }
}

pub(crate) fn read_usage_files<T: Send>(
    single_thread: bool,
    paths: &[PathBuf],
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
    read: impl Fn(&PathBuf) -> T + Sync,
) -> Vec<(PathBuf, T)> {
    if single_thread {
        return read_discovered_paths(
            true,
            |visit| visit_usage_files(paths, project_filter, visit, diagnostics),
            read,
        );
    }
    read_paths_discovered_in_parallel(
        |visit| {
            let filtered_visit = |path: PathBuf| {
                if matches_project(&path, project_filter) {
                    visit(path);
                }
            };
            for path in paths {
                visit_usage_files_in_parallel(
                    &projects_dir(path, project_filter),
                    &filtered_visit,
                    diagnostics,
                );
            }
        },
        read,
    )
}

fn matches_project(path: &Path, project_filter: Option<&str>) -> bool {
    project_filter.is_none_or(|filter| extract_project(path) == filter)
}

fn projects_dir(path: &Path, project_filter: Option<&str>) -> PathBuf {
    let projects_dir = path.join("projects");
    match project_filter.filter(|filter| is_project_path_segment(filter)) {
        Some(project_filter) => projects_dir.join(project_filter),
        None => projects_dir,
    }
}

pub(crate) fn usage_file_order(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const MTIME_PRUNE_MARGIN_MS: i64 = MILLIS_PER_DAY;

pub(super) struct SinceFiles<T> {
    pub(super) kept: Vec<(PathBuf, T)>,
    pub(super) pruned: Vec<PathBuf>,
}

// NOTE(2026-09-25, #1794): Claude Code appends session JSONL, and a transcript can record a session ID different from its directory. Keep related transcripts together when pruning by mtime so a replay still has its parent.
pub(super) fn read_usage_files_since<T: Send>(
    paths: &[PathBuf],
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
    shared: &SharedArgs,
    now: TimestampMs,
    read: impl Fn(&PathBuf) -> T + Sync,
) -> SinceFiles<T> {
    read_files_since(
        shared.single_thread,
        prune_threshold(shared, now),
        |check| {
            read_usage_files(
                shared.single_thread,
                paths,
                project_filter,
                diagnostics,
                check,
            )
        },
        read,
    )
}

fn prune_threshold(shared: &SharedArgs, now: TimestampMs) -> Option<i64> {
    let timezone = resolve_tz(shared.timezone.as_ref());
    let (since, _) = date_range_bounds_ms(shared.since, None, &timezone);
    since
        .filter(|since| *since <= now.as_millis())
        .map(|since| since.saturating_sub(MTIME_PRUNE_MARGIN_MS))
}

type ReadIfLive<'a, T> = dyn Fn(&PathBuf) -> Option<T> + Sync + 'a;

fn read_files_since<T: Send>(
    single_thread: bool,
    threshold: Option<i64>,
    read_discovered: impl FnOnce(&ReadIfLive<'_, T>) -> Vec<(PathBuf, Option<T>)>,
    read: impl Fn(&PathBuf) -> T + Sync,
) -> SinceFiles<T> {
    let mut kept = Vec::new();
    let mut stale = Vec::new();
    let checked = read_discovered(&|file| {
        threshold
            .is_none_or(|threshold| {
                file_modified_millis(file).is_none_or(|modified| modified >= threshold)
            })
            .then(|| read(file))
    });
    for (file, loaded) in checked {
        match loaded {
            Some(loaded) => kept.push((file, loaded)),
            None => stale.push(file),
        }
    }
    if kept.is_empty() || stale.is_empty() {
        return SinceFiles {
            kept,
            pruned: stale,
        };
    }

    let recorded_ids = |files: &mut dyn Iterator<Item = &PathBuf>| {
        read_discovered_paths(
            single_thread,
            |visit| files.cloned().for_each(visit),
            |file| first_recorded_session_id(file),
        )
    };
    let mut sessions = SessionGroups::default();
    let live_groups = recorded_ids(&mut kept.iter().map(|(file, _)| file))
        .into_iter()
        .map(|(file, recorded)| sessions.file(&file, recorded.as_deref()))
        .collect::<Vec<_>>();
    let stale_groups = recorded_ids(&mut stale.iter())
        .into_iter()
        .map(|(file, recorded)| {
            let group = sessions.file(&file, recorded.as_deref());
            (file, group)
        })
        .collect::<Vec<_>>();
    let live_groups = live_groups
        .into_iter()
        .map(|group| sessions.root(group))
        .collect::<FxHashSet<_>>();
    let (mut revived, pruned) = stale_groups
        .into_iter()
        .partition::<Vec<_>, _>(|(_, group)| live_groups.contains(&sessions.root(*group)));
    kept.extend(read_discovered_paths(
        single_thread,
        |visit| revived.drain(..).for_each(|(file, _)| visit(file)),
        read,
    ));
    SinceFiles {
        kept,
        pruned: pruned.into_iter().map(|(file, _)| file).collect(),
    }
}

#[derive(Default)]
struct SessionGroups {
    ids: FxHashMap<String, usize>,
    parents: Vec<usize>,
}

impl SessionGroups {
    fn file(&mut self, file: &Path, recorded: Option<&str>) -> usize {
        let (path_session, _) = extract_session_parts(file);
        let group = self.id(&path_session);
        if let Some(recorded) = recorded {
            let recorded = self.id(recorded);
            self.union(group, recorded);
        }
        group
    }

    fn id(&mut self, session: &str) -> usize {
        if let Some(&id) = self.ids.get(session) {
            return id;
        }
        let id = self.parents.len();
        self.parents.push(id);
        self.ids.insert(session.to_string(), id);
        id
    }

    fn root(&mut self, mut id: usize) -> usize {
        while self.parents[id] != id {
            self.parents[id] = self.parents[self.parents[id]];
            id = self.parents[id];
        }
        id
    }

    fn union(&mut self, left: usize, right: usize) {
        let (left, right) = (self.root(left), self.root(right));
        self.parents[left] = right;
    }
}

fn first_recorded_session_id(path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct SessionIdProbe {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
    }

    let marker = LinePrefilter::all(&[br#""sessionId""#]);
    jsonl::read(path, None, ReadPolicy::PLAIN, |line, _, _| {
        if marker.matches(line.raw)
            && let Ok(SessionIdProbe {
                session_id: Some(session_id),
            }) = line.decode()
        {
            return ControlFlow::Break(session_id);
        }
        ControlFlow::Continue(())
    })
    .ok()
    .flatten()
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

pub(super) fn is_project_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

pub(crate) fn extract_project(path: &Path) -> String {
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

pub(crate) fn extract_session_parts(path: &Path) -> (String, String) {
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
    if let Some(subagents_index) = relative
        .iter()
        .enumerate()
        .take(relative.len().saturating_sub(1))
        .skip(2)
        .find_map(|(index, part)| (*part == "subagents").then_some(index))
    {
        let session_id = relative[subagents_index - 1].to_string();
        let project_path = relative[..subagents_index - 1].join(std::path::MAIN_SEPARATOR_STR);
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

#[cfg(test)]
mod tests;
