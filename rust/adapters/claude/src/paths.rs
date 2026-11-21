use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::UNIX_EPOCH,
};

use serde::Deserialize;

use memchr::memmem::Finder;

use crate::{
    Diagnostics, MILLIS_PER_DAY, Result, TimestampMs,
    cli::{SharedArgs, home},
    cli_error, date_range_bounds_ms,
    fast::{FxHashMap, FxHashSet, LinePrefilter},
    parse_ts_timestamp, resolve_tz,
    usage_wire::Text,
};
use ccusage_adapter_common::{read_discovered_paths, visit_usage_files as visit_usage_files_in};
use ccusage_core::parallel::spawn_scoped_worker;

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
    let roots = projects_roots(paths);
    let mut filtered_visit = |file: PathBuf| {
        if project_filter.is_none_or(|filter| {
            extract_project(&file, containing_projects_root(&file, &roots)) == filter
        }) {
            visit(file);
        }
    };
    for projects_dir in &roots {
        if let Some(project_filter) =
            project_filter.filter(|filter| is_project_path_segment(filter))
        {
            visit_usage_files_in(
                &projects_dir.join(project_filter),
                &mut filtered_visit,
                diagnostics,
            );
        } else {
            visit_usage_files_in(projects_dir, &mut filtered_visit, diagnostics);
        }
    }
}

pub(crate) fn projects_roots(config_dirs: &[PathBuf]) -> Vec<PathBuf> {
    config_dirs
        .iter()
        .map(|config_dir| config_dir.join("projects"))
        .collect()
}

// NOTE(2026-11-20, #2374): CLAUDE_CONFIG_DIR can list a config directory and one of its descendants. The deepest root keeps a transcript attributed to its own config directory.
pub(crate) fn containing_projects_root<'a>(
    file: &Path,
    projects_roots: &'a [PathBuf],
) -> Option<&'a Path> {
    projects_roots
        .iter()
        .map(PathBuf::as_path)
        .filter(|root| file.starts_with(root))
        .max_by_key(|root| root.components().count())
}

#[cfg(test)]
pub(crate) fn fixture_projects_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "projects"))
        .map(Path::to_path_buf)
}

pub(crate) fn usage_file_order(left: &Path, right: &Path) -> std::cmp::Ordering {
    left.as_os_str().cmp(right.as_os_str())
}

const MTIME_PRUNE_MARGIN_MS: i64 = MILLIS_PER_DAY;
const TAIL_PROBE_BYTES: u64 = 16 * 1024;
const PROBE_READ_LIMIT: u64 = 1024 * 1024;
const HEAD_PROBE_BYTES: u64 = 64 * 1024;
const WALK_BACKLOG: usize = 1024;
const WALK_BATCH: usize = 64;

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
        &projects_roots(paths),
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
    projects_roots: &[PathBuf],
    discover: impl FnMut(&mut dyn FnMut(PathBuf)) + Send,
    read: impl Fn(&PathBuf) -> T + Sync,
) -> SinceFiles<T> {
    let Some(threshold) = threshold else {
        return SinceFiles {
            kept: read_discovered_paths(single_thread, discover, read),
            pruned: Vec::new(),
        };
    };
    // NOTE(2026-11-20, #2355): Stat'ing and probing from the parallel readers contends in the kernel. The walker checks each mtime and probes stale files before queuing; live files' session probes run serially after parsing.
    let mut discovered = Vec::new();
    let mut probe = StaleProbe::new(threshold);
    let mut kept = walk_beside(single_thread, discover, |files| {
        read_discovered_paths(
            single_thread,
            |visit| {
                files(&mut |file| {
                    let (live, recorded_session) = probe.file(&file);
                    if live {
                        visit(file.clone());
                    }
                    discovered.push((file, live, recorded_session));
                });
            },
            &read,
        )
    });
    if kept.is_empty() || kept.len() == discovered.len() {
        return SinceFiles {
            kept,
            pruned: discovered
                .into_iter()
                .filter_map(|(file, live, _)| (!live).then_some(file))
                .collect(),
        };
    }

    let mut sessions = SessionGroups::default();
    let groups = discovered
        .into_iter()
        .map(|(file, live, recorded_session)| {
            let recorded_session =
                recorded_session.unwrap_or_else(|| first_recorded_session_id(&file));
            let group = sessions.file(&file, recorded_session.as_deref(), projects_roots);
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

// NOTE(2026-11-21): Directory reads and stale-file probes are both blocking syscalls on one thread. Walking on its own thread overlaps them. Paths cross in batches because one wake-up per path cost macOS more than the overlap saved.
fn walk_beside<R>(
    single_thread: bool,
    mut discover: impl FnMut(&mut dyn FnMut(PathBuf)) + Send,
    consume: impl FnOnce(&mut dyn FnMut(&mut dyn FnMut(PathBuf))) -> R,
) -> R {
    if single_thread {
        return consume(&mut discover);
    }
    thread::scope(|scope| {
        let (sender, batches) = mpsc::sync_channel::<Vec<PathBuf>>(WALK_BACKLOG / WALK_BATCH);
        spawn_scoped_worker(scope, move || {
            let mut batch = Vec::with_capacity(WALK_BATCH);
            discover(&mut |file| {
                batch.push(file);
                if batch.len() == WALK_BATCH {
                    let _ = sender.send(std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(WALK_BATCH),
                    ));
                }
            });
            let _ = sender.send(batch);
        });
        consume(&mut |visit| batches.iter().flatten().for_each(&mut *visit))
    })
}

#[derive(Default)]
struct SessionGroups {
    ids: FxHashMap<String, usize>,
    parents: Vec<usize>,
}

impl SessionGroups {
    fn file(&mut self, file: &Path, recorded: Option<&str>, projects_roots: &[PathBuf]) -> usize {
        let (path_session, _) =
            extract_session_parts(file, containing_projects_root(file, projects_roots));
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

struct StaleProbe {
    threshold: i64,
    session_marker: LinePrefilter,
    timestamp_key: Finder<'static>,
    escape: Finder<'static>,
    tail: Vec<u8>,
    partial_line: Vec<u8>,
    escaped_lines: Vec<(usize, usize)>,
    older_lines: Vec<(usize, usize)>,
}

impl StaleProbe {
    fn new(threshold: i64) -> Self {
        Self {
            threshold,
            session_marker: session_marker(),
            timestamp_key: Finder::new(br#""timestamp""#),
            escape: Finder::new(br"\u00"),
            tail: Vec::new(),
            partial_line: Vec::new(),
            escaped_lines: Vec::new(),
            older_lines: Vec::new(),
        }
    }

    // NOTE(2026-11-21, #2431): On a recent `--since` most files are stale, and each stale file is opened for its tail anyway. Taking the mtime from the open handle saves a path lookup per stale file, and session grouping's first recorded session is read from the same handle rather than from a second open.
    fn file(&mut self, path: &Path) -> (bool, Option<Option<String>>) {
        let Ok(file) = File::open(path) else {
            return (true, None);
        };
        let Some((modified, len)) = file
            .metadata()
            .ok()
            .and_then(|metadata| Some((modified_millis(&metadata)?, metadata.len())))
        else {
            return (true, None);
        };
        if modified >= self.threshold {
            return (true, None);
        }
        let in_one_window = len <= TAIL_PROBE_BYTES + 1;
        let recorded_session = if in_one_window {
            None
        } else {
            head_recorded_session_id(&self.session_marker, &file, len)
        };
        // NOTE(2026-11-20, #2376): A restored or imported transcript can keep an mtime older than its records. Pruning on mtime alone silently drops in-window usage, so the latest timestamp in the tail must also be stale.
        let Some(live) = self.tail_reaches(&file, len) else {
            return (true, None);
        };
        let recorded_session = if in_one_window {
            Some(lines(&self.tail).find_map(|line| recorded_session_id(&self.session_marker, line)))
        } else {
            recorded_session
        };
        (live, recorded_session)
    }

    // NOTE(2026-11-21, #2431): A tail without record timestamps proves nothing, and treating it as live revives the file's whole session group. Read further back until a record timestamp turns up; a file without any holds no dated usage. Reads stop growing at `PROBE_READ_LIMIT` so a large undated file doesn't pin its size in memory, and a line longer than that keeps the file unjudged.
    fn tail_reaches(&mut self, file: &File, len: u64) -> Option<bool> {
        let mut end = len;
        let mut window = TAIL_PROBE_BYTES + 1;
        self.partial_line.clear();
        loop {
            let start = end.saturating_sub(window);
            let read = usize::try_from(end - start).ok()?;
            self.tail.resize(read, 0);
            read_exact_at(file, &mut self.tail, start).ok()?;
            self.tail.extend_from_slice(&self.partial_line);
            let (partial_end, whole_lines) = match memchr::memchr(b'\n', &self.tail) {
                _ if start == 0 => (0, 0),
                Some(newline) => (newline, newline + 1),
                None => (self.tail.len(), self.tail.len()),
            };
            match self.judge_records(whole_lines) {
                Records::Reach => return Some(true),
                Records::Older => return Some(false),
                Records::Absent if start == 0 => return Some(false),
                Records::Absent => {}
            }
            if partial_end as u64 > PROBE_READ_LIMIT {
                return None;
            }
            self.partial_line.clear();
            self.partial_line
                .extend_from_slice(&self.tail[..partial_end]);
            end = start;
            window = window.saturating_mul(2).min(PROBE_READ_LIMIT);
        }
    }

    // NOTE(2026-11-21, #2431): Parsing every tail line dominated `--since` on logs where most files are stale. Only a `\u006x` or `\u007x` escape can spell a letter of `timestamp`; outside one, every `timestamp` key at any depth is spelled out. So only lines with an escape, or with a spelled value at or after the threshold or one that can't be read off the bytes, need parsing; otherwise the shortest line with a top-level timestamp is enough to call the tail older.
    fn judge_records(&mut self, from: usize) -> Records {
        let Self {
            threshold,
            timestamp_key,
            escape,
            tail,
            escaped_lines,
            older_lines,
            ..
        } = self;
        let window = &tail[from..];
        let mut older = false;
        let mut judge = |line: &[u8]| match record_timestamp(line) {
            Some(timestamp) if timestamp >= *threshold => true,
            Some(_) => {
                older = true;
                false
            }
            None => false,
        };
        escaped_lines.clear();
        for at in escape.find_iter(window) {
            if escaped_lines.last().is_some_and(|&(_, end)| at < end)
                || !matches!(window.get(at + escape.needle().len()), Some(b'6' | b'7'))
            {
                continue;
            }
            let (start, end) = line_around(window, at);
            escaped_lines.push((start, end));
            if judge(&window[start..end]) {
                return Records::Reach;
            }
        }
        older_lines.clear();
        let mut escaped = escaped_lines.iter().peekable();
        let mut parsed_through = 0;
        for key in timestamp_key.find_iter(window) {
            while escaped.next_if(|&&(_, end)| end < key).is_some() {}
            if key < parsed_through || escaped.peek().is_some_and(|&&(start, _)| start <= key) {
                continue;
            }
            match spelled_value(&window[key + timestamp_key.needle().len()..]) {
                SpelledValue::NotKey => {}
                SpelledValue::At(timestamp) if timestamp < *threshold => {
                    if older_lines.last().is_none_or(|&(_, end)| end < key) {
                        older_lines.push(line_around(window, key));
                    }
                }
                SpelledValue::At(_) | SpelledValue::Unreadable => {
                    let (start, end) = line_around(window, key);
                    parsed_through = end;
                    if judge(&window[start..end]) {
                        return Records::Reach;
                    }
                }
            }
        }
        if older {
            return Records::Older;
        }
        older_lines.sort_unstable_by_key(|(start, end)| end - start);
        if older_lines
            .iter()
            .any(|&(start, end)| record_timestamp(&window[start..end]).is_some())
        {
            Records::Older
        } else {
            Records::Absent
        }
    }
}

enum SpelledValue {
    NotKey,
    At(i64),
    Unreadable,
}

fn spelled_value(after_key: &[u8]) -> SpelledValue {
    let Some(value) = after_key.trim_ascii_start().strip_prefix(b":") else {
        return SpelledValue::NotKey;
    };
    value
        .trim_ascii_start()
        .strip_prefix(b"\"")
        .and_then(|value| Some(&value[..memchr::memchr(b'"', value)?]))
        .filter(|value| !value.contains(&b'\\'))
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(parse_ts_timestamp)
        .map_or(SpelledValue::Unreadable, |timestamp| {
            SpelledValue::At(timestamp.as_millis())
        })
}

fn line_around(bytes: &[u8], at: usize) -> (usize, usize) {
    let start = memchr::memrchr(b'\n', &bytes[..at]).map_or(0, |newline| newline + 1);
    let end = memchr::memchr(b'\n', &bytes[at..]).map_or(bytes.len(), |newline| at + newline);
    (start, end)
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', bytes)
        .chain([bytes.len()])
        .map(move |end| {
            let line = &bytes[start..end];
            start = end + 1;
            line
        })
}

enum Records {
    Reach,
    Older,
    Absent,
}

#[cfg(unix)]
fn read_exact_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buffer, offset)
}

#[cfg(not(unix))]
fn read_exact_at(mut file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    io::Seek::seek(&mut file, io::SeekFrom::Start(offset))?;
    io::Read::read_exact(&mut file, buffer)
}

fn record_timestamp(line: &[u8]) -> Option<i64> {
    #[derive(Deserialize)]
    struct TimestampProbe<'a> {
        #[serde(borrow)]
        timestamp: Option<Text<'a>>,
    }

    serde_json::from_slice::<TimestampProbe<'_>>(line)
        .ok()?
        .timestamp
        .and_then(|timestamp| parse_ts_timestamp(&timestamp.0))
        .map(|timestamp| timestamp.as_millis())
}

fn session_marker() -> LinePrefilter {
    LinePrefilter::all(&[br#""sessionId""#])
}

fn first_recorded_session_id(path: &Path) -> Option<String> {
    recorded_session_id_in(&session_marker(), &File::open(path).ok()?)
}

// NOTE(2026-11-21, #2431): Grouping needs the first recorded session only when some file is live, and a file without one would be read to its end. Read a bounded head here; past that, leave it to the lazy `first_recorded_session_id`.
fn head_recorded_session_id(
    marker: &LinePrefilter,
    file: &File,
    len: u64,
) -> Option<Option<String>> {
    let mut lines = BufReader::new(io::Read::take(file, HEAD_PROBE_BYTES));
    let mut line = Vec::new();
    while lines.read_until(b'\n', &mut line).ok()? > 0 {
        if let Some(session_id) = recorded_session_id(marker, &line) {
            return Some(Some(session_id));
        }
        line.clear();
    }
    (len <= HEAD_PROBE_BYTES).then_some(None)
}

fn recorded_session_id_in(marker: &LinePrefilter, file: &File) -> Option<String> {
    let mut lines = BufReader::new(file);
    let mut line = Vec::new();
    while lines.read_until(b'\n', &mut line).ok()? > 0 {
        if let Some(session_id) = recorded_session_id(marker, &line) {
            return Some(session_id);
        }
        line.clear();
    }
    None
}

fn recorded_session_id(marker: &LinePrefilter, line: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct SessionIdProbe {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
    }

    if !marker.matches(line) {
        return None;
    }
    serde_json::from_slice::<SessionIdProbe>(line)
        .ok()?
        .session_id
}

fn modified_millis(metadata: &fs::Metadata) -> Option<i64> {
    metadata
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

fn below_projects<'a>(path: &'a Path, projects_root: Option<&Path>) -> Option<Vec<&'a str>> {
    let relative = path.strip_prefix(projects_root?).ok()?;
    Some(
        relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect(),
    )
}

pub(crate) fn extract_project(path: &Path, projects_root: Option<&Path>) -> String {
    below_projects(path, projects_root)
        .and_then(|parts| parts.first().copied())
        .map_or_else(
            || "unknown".to_string(),
            |part| {
                if part.trim().is_empty() {
                    "unknown"
                } else {
                    part
                }
                .to_string()
            },
        )
}

pub(crate) fn extract_session_parts(path: &Path, projects_root: Option<&Path>) -> (String, String) {
    let Some(relative) = below_projects(path, projects_root) else {
        return ("unknown".to_string(), "Unknown Project".to_string());
    };
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
