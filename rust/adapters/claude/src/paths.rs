use std::{
    env, fs,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem;
use serde::Deserialize;

use crate::{
    MILLIS_PER_DAY, Result,
    cli::SharedArgs,
    cli_error, date_range_bounds_ms,
    fast::{FxHashMap, FxHashSet},
    home, parse_tz,
    path_utils::expand_home_path,
};
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

/// Margin subtracted from the `--since` lower bound before comparing it with
/// file mtimes. It absorbs the gap between the timezone-resolved bound and
/// wall-clock mtimes, plus sessions flushed well after the entries they hold.
const MTIME_PRUNE_MARGIN_MS: i64 = MILLIS_PER_DAY;

/// Usage files split by whether they can hold entries inside a `--since`
/// window.
pub(super) struct SinceFiles {
    /// Files that may hold entries inside the window, in discovery order.
    pub(super) kept: Vec<PathBuf>,
    /// Files skipped because their whole session was last written before it.
    pub(super) pruned: Vec<PathBuf>,
}

/// Drops sessions whose files were all last written before the `--since`
/// window.
///
/// Claude session JSONL files are append-only, so a file last written before
/// the window opened cannot hold an entry inside it. Deduplication matches
/// sidechain replays against their parent messages by session ID, which spans
/// files (subagent and workflow transcripts, legacy flat `agent-*.jsonl`
/// files, copies in another config root), so files are kept or dropped per
/// connected group of session IDs: dropping a parent file alone would let its
/// replay count twice. Copies of a message in other sessions, such as resumed
/// transcripts, keep the original timestamp, so they fall on the same side of
/// the window as the original.
///
/// Only the file list is narrowed; entries parsed from a surviving file are
/// untouched. Every file is kept when `since` is unset or unparsable, when the
/// window opens ahead of the current clock, or when a file's mtime cannot be
/// read.
///
/// @param files Discovered usage files.
/// @param shared Report arguments providing `since` and `timezone`.
/// @param now Current wall-clock time.
/// @returns The kept files and the pruned ones, each in discovery order.
pub(super) fn split_files_before_since(
    files: Vec<PathBuf>,
    shared: &SharedArgs,
    now: TimestampMs,
) -> SinceFiles {
    let keep_all = |files| SinceFiles {
        kept: files,
        pruned: Vec::new(),
    };
    let timezone = parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    let (Some(since_ms), _) =
        date_range_bounds_ms(shared.since.as_deref(), None, timezone.as_ref())
    else {
        return keep_all(files);
    };
    // Entry timestamps can outrun any mtime the filesystem reports when the
    // window starts in the future, so an mtime cannot rule a file out.
    if since_ms > now.as_millis() {
        return keep_all(files);
    }
    let threshold = since_ms.saturating_sub(MTIME_PRUNE_MARGIN_MS);

    // Union every file's session IDs so files sharing any ID share a group.
    let mut sessions = SessionGroups::default();
    let file_groups = files
        .iter()
        .map(|file| {
            let (path_session, _) = extract_session_parts(file);
            let group = sessions.id(&path_session);
            if let Some(recorded) = recorded_session_id(file) {
                let recorded = sessions.id(&recorded);
                sessions.union(group, recorded);
            }
            group
        })
        .collect::<Vec<_>>();
    let live_groups = files
        .iter()
        .zip(&file_groups)
        .filter(|(file, _)| file_modified_millis(file).is_none_or(|modified| modified >= threshold))
        .map(|(_, &group)| sessions.root(group))
        .collect::<FxHashSet<_>>();

    let mut split = SinceFiles {
        kept: Vec::new(),
        pruned: Vec::new(),
    };
    for (file, group) in files.into_iter().zip(file_groups) {
        if live_groups.contains(&sessions.root(group)) {
            split.kept.push(file);
        } else {
            split.pruned.push(file);
        }
    }
    split
}

/// Union-find over session IDs.
#[derive(Default)]
struct SessionGroups {
    ids: FxHashMap<String, usize>,
    parents: Vec<usize>,
}

impl SessionGroups {
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
            // Path halving: point each visited node at its grandparent.
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

/// Returns the first `sessionId` a transcript records, which is the session
/// the loaders attribute its entries to. Transcripts record a single session,
/// so reading stops at the first line that names one.
fn recorded_session_id(path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct SessionIdProbe {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
    }

    let mut reader = BufReader::new(fs::File::open(path).ok()?);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).ok()? == 0 {
            return None;
        }
        if memmem::find(&line, b"\"sessionId\"").is_none() {
            continue;
        }
        if let Ok(SessionIdProbe {
            session_id: Some(session_id),
        }) = serde_json::from_slice(&line)
        {
            return Some(session_id);
        }
    }
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

#[cfg(test)]
mod tests {
    use std::{
        fs::{File, FileTimes},
        path::{Path, PathBuf},
        time::{Duration, UNIX_EPOCH},
    };

    use ccusage_test_support::fs_fixture;

    use super::split_files_before_since;
    use crate::{MILLIS_PER_DAY, TimestampMs, cli::SharedArgs, parse_ts_timestamp};

    fn set_file_modified(path: &Path, timestamp: TimestampMs) {
        let milliseconds = u64::try_from(timestamp.as_millis()).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(milliseconds)),
            )
            .unwrap();
    }

    fn ts(value: &str) -> TimestampMs {
        parse_ts_timestamp(value).unwrap()
    }

    fn shared_since(since: Option<&str>) -> SharedArgs {
        SharedArgs {
            since: since.map(str::to_string),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        }
    }

    fn file_names(files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn fixture_files(root: &Path) -> Vec<PathBuf> {
        let old = root.join("projects/p/old.jsonl");
        let margin = root.join("projects/p/margin.jsonl");
        let fresh = root.join("projects/p/fresh.jsonl");
        set_file_modified(&old, ts("2026-09-18T12:00:00Z"));
        set_file_modified(&margin, ts("2026-09-19T12:00:00Z"));
        set_file_modified(&fresh, ts("2026-09-20T12:00:00Z"));
        vec![fresh, margin, old]
    }

    #[test]
    fn prunes_files_last_written_before_the_since_margin() {
        let fixture = fs_fixture!({
            "projects/p/old.jsonl": "{}",
            "projects/p/margin.jsonl": "{}",
            "projects/p/fresh.jsonl": "{}",
        });
        let files = fixture_files(fixture.root());

        let kept = split_files_before_since(
            files,
            &shared_since(Some("20260920")),
            ts("2026-09-24T00:00:00Z"),
        )
        .kept;

        assert_eq!(file_names(&kept), ["fresh.jsonl", "margin.jsonl"]);
    }

    #[test]
    fn keeps_every_file_without_a_since_bound() {
        let fixture = fs_fixture!({
            "projects/p/old.jsonl": "{}",
            "projects/p/margin.jsonl": "{}",
            "projects/p/fresh.jsonl": "{}",
        });
        let files = fixture_files(fixture.root());

        let kept =
            split_files_before_since(files, &shared_since(None), ts("2026-09-24T00:00:00Z")).kept;

        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn keeps_every_file_when_the_window_opens_in_the_future() {
        let fixture = fs_fixture!({
            "projects/p/old.jsonl": "{}",
            "projects/p/margin.jsonl": "{}",
            "projects/p/fresh.jsonl": "{}",
        });
        let files = fixture_files(fixture.root());
        let now = TimestampMs::from_millis(ts("2026-09-30T00:00:00Z").as_millis() - MILLIS_PER_DAY);

        let kept = split_files_before_since(files, &shared_since(Some("20260930")), now).kept;

        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn keeps_files_whose_metadata_cannot_be_read() {
        let missing = PathBuf::from("/nonexistent/ccusage/projects/p/missing.jsonl");

        let kept = split_files_before_since(
            vec![missing.clone()],
            &shared_since(Some("20260920")),
            ts("2026-09-24T00:00:00Z"),
        )
        .kept;

        assert_eq!(kept, [missing]);
    }

    #[test]
    fn keeps_or_prunes_a_session_together_with_its_subagents() {
        let fixture = fs_fixture!({
            "projects/p/live.jsonl": "{}",
            "projects/p/live/subagents/agent-a.jsonl": "{}",
            "projects/p/stale.jsonl": "{}",
            "projects/p/stale/subagents/agent-b.jsonl": "{}",
        });
        let root = fixture.root();
        let live_parent = root.join("projects/p/live.jsonl");
        let live_agent = root.join("projects/p/live/subagents/agent-a.jsonl");
        let stale_parent = root.join("projects/p/stale.jsonl");
        let stale_agent = root.join("projects/p/stale/subagents/agent-b.jsonl");
        // Only the live session's subagent was written inside the window.
        set_file_modified(&live_parent, ts("2026-09-01T12:00:00Z"));
        set_file_modified(&live_agent, ts("2026-09-21T12:00:00Z"));
        set_file_modified(&stale_parent, ts("2026-09-01T12:00:00Z"));
        set_file_modified(&stale_agent, ts("2026-09-02T12:00:00Z"));

        let split = split_files_before_since(
            vec![
                live_parent.clone(),
                live_agent.clone(),
                stale_parent.clone(),
                stale_agent.clone(),
            ],
            &shared_since(Some("20260920")),
            ts("2026-09-24T00:00:00Z"),
        );

        assert_eq!(split.kept, [live_parent, live_agent]);
        assert_eq!(split.pruned, [stale_parent, stale_agent]);
    }

    #[test]
    fn keeps_legacy_flat_agent_transcripts_and_config_root_copies_with_their_session() {
        let fixture = fs_fixture!({
            "a/projects/p/live.jsonl": "{}",
            "a/projects/p/agent-1234.jsonl": r#"{"sessionId":"live","timestamp":"2026-09-01T12:00:00Z"}"#,
            "a/projects/p/agent-5678.jsonl": r#"{"sessionId":"stale","timestamp":"2026-09-01T12:00:00Z"}"#,
            "b/projects/p/live.jsonl": "{}",
        });
        let root = fixture.root();
        let live = root.join("a/projects/p/live.jsonl");
        let live_agent = root.join("a/projects/p/agent-1234.jsonl");
        let stale_agent = root.join("a/projects/p/agent-5678.jsonl");
        let other_root_copy = root.join("b/projects/p/live.jsonl");
        set_file_modified(&live, ts("2026-09-21T12:00:00Z"));
        for stale in [&live_agent, &stale_agent, &other_root_copy] {
            set_file_modified(stale, ts("2026-09-01T12:00:00Z"));
        }

        let split = split_files_before_since(
            vec![
                live_agent.clone(),
                stale_agent.clone(),
                live.clone(),
                other_root_copy.clone(),
            ],
            &shared_since(Some("20260920")),
            ts("2026-09-24T00:00:00Z"),
        );

        assert_eq!(split.kept, [live_agent, live, other_root_copy]);
        assert_eq!(split.pruned, [stale_agent]);
    }

    #[test]
    fn keeps_sessions_linked_by_a_recorded_session_id_in_another_directory() {
        let fixture = fs_fixture!({
            "projects/p/session-y.jsonl": r#"{"sessionId":"session-y"}"#,
            "projects/p/session-x/subagents/workflows/wf_1/agent-a.jsonl": "{\"type\":\"summary\"}\n{\"sessionId\" : \"session-y\"}",
        });
        let root = fixture.root();
        let parent = root.join("projects/p/session-y.jsonl");
        let workflow_agent =
            root.join("projects/p/session-x/subagents/workflows/wf_1/agent-a.jsonl");
        set_file_modified(&parent, ts("2026-09-01T12:00:00Z"));
        set_file_modified(&workflow_agent, ts("2026-09-21T12:00:00Z"));

        let split = split_files_before_since(
            vec![parent.clone(), workflow_agent.clone()],
            &shared_since(Some("20260920")),
            ts("2026-09-24T00:00:00Z"),
        );

        assert_eq!(split.kept, [parent, workflow_agent]);
        assert!(split.pruned.is_empty());
    }

    #[test]
    fn reads_the_recorded_session_id_past_long_leading_lines() {
        let padding = format!(
            r#"{{"type":"summary","text":"{}"}}"#,
            "x".repeat(100 * 1024)
        );
        let fixture = fs_fixture!({
            "projects/p/agent-1.jsonl": format!("{padding}\n{{\"sessionId\":  \"parent\"}}"),
        });

        assert_eq!(
            super::recorded_session_id(&fixture.root().join("projects/p/agent-1.jsonl")).as_deref(),
            Some("parent")
        );
    }
}
