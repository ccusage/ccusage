use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
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
    parse_ts_timestamp, resolve_tz,
};
use ccusage_adapter_common::{
    jsonl::{self, ReadPolicy},
    read_discovered_paths, visit_usage_files as visit_usage_files_in,
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
    files.sort_by(|left, right| usage_file_order(left, right));
    files
}

pub(crate) fn visit_usage_files(
    paths: &[PathBuf],
    project_filter: Option<&str>,
    visit: &mut dyn FnMut(PathBuf),
    diagnostics: &Diagnostics,
) {
    let mut filtered_visit = |path: PathBuf| {
        if project_filter.is_none_or(|filter| extract_project(&path) == filter) {
            visit(path);
        }
    };
    for path in paths {
        let projects_dir = path.join("projects");
        if let Some(project_filter) =
            project_filter.filter(|filter| is_project_path_segment(filter))
        {
            visit_usage_files_in(
                &projects_dir.join(project_filter),
                &mut filtered_visit,
                diagnostics,
            );
        } else {
            visit_usage_files_in(&projects_dir, &mut filtered_visit, diagnostics);
        }
    }
}

pub(crate) fn usage_file_order(left: &Path, right: &Path) -> std::cmp::Ordering {
    left.as_os_str().cmp(right.as_os_str())
}

const MTIME_PRUNE_MARGIN_MS: i64 = MILLIS_PER_DAY;
const TAIL_PROBE_BYTES: u64 = 16 * 1024;

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
        |visit| visit_usage_files(paths, project_filter, visit, diagnostics),
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

fn read_files_since<T: Send>(
    single_thread: bool,
    threshold: Option<i64>,
    discover: impl FnMut(&mut dyn FnMut(PathBuf)),
    read: impl Fn(&PathBuf) -> T + Sync,
) -> SinceFiles<T> {
    let Some(threshold) = threshold else {
        return SinceFiles {
            kept: read_discovered_paths(single_thread, discover, read),
            pruned: Vec::new(),
        };
    };
    // NOTE(2026-11-20, #2355): Stat'ing and probing from the parallel readers contends in the kernel. The walker checks each mtime before queuing, and probes run serially after parsing.
    let mut discovered = Vec::new();
    let mut discover = discover;
    let mut kept = read_discovered_paths(
        single_thread,
        |visit| {
            discover(&mut |file| {
                // NOTE(2026-11-20, #2376): A restored or imported transcript can keep an mtime older than its records. Pruning on mtime alone silently drops in-window usage, so the latest timestamp in the tail must also be stale.
                let live = file_modified_millis(&file).is_none_or(|modified| modified >= threshold)
                    || latest_tail_timestamp(&file).is_none_or(|latest| latest >= threshold);
                if live {
                    visit(file.clone());
                }
                discovered.push((file, live));
            });
        },
        &read,
    );
    if kept.is_empty() || kept.len() == discovered.len() {
        return SinceFiles {
            kept,
            pruned: discovered
                .into_iter()
                .filter_map(|(file, live)| (!live).then_some(file))
                .collect(),
        };
    }

    let mut sessions = SessionGroups::default();
    let groups = discovered
        .into_iter()
        .map(|(file, live)| {
            let group = sessions.file(&file, first_recorded_session_id(&file).as_deref());
            (file, live, group)
        })
        .collect::<Vec<_>>();
    let live_groups = groups
        .iter()
        .filter(|(_, live, _)| *live)
        .map(|&(_, _, group)| sessions.root(group))
        .collect::<FxHashSet<_>>();
    let (mut revived, pruned) = groups
        .into_iter()
        .filter(|(_, live, _)| !live)
        .map(|(file, _, group)| (file, group))
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

fn latest_tail_timestamp(path: &Path) -> Option<i64> {
    #[derive(Deserialize)]
    struct TimestampProbe<'a> {
        timestamp: Option<&'a str>,
    }

    let mut file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(TAIL_PROBE_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::with_capacity(TAIL_PROBE_BYTES as usize);
    file.take(TAIL_PROBE_BYTES).read_to_end(&mut tail).ok()?;
    let whole_lines = if start == 0 {
        &tail[..]
    } else {
        &tail[memchr::memchr(b'\n', &tail)? + 1..]
    };
    whole_lines
        .split(|&byte| byte == b'\n')
        .filter_map(|line| {
            serde_json::from_slice::<TimestampProbe<'_>>(line)
                .ok()?
                .timestamp
        })
        .filter_map(parse_ts_timestamp)
        .map(|timestamp| timestamp.as_millis())
        .max()
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
