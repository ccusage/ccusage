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
    visit_usage_files as visit_usage_files_in,
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

pub(crate) fn usage_file_order(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const MTIME_PRUNE_MARGIN_MS: i64 = MILLIS_PER_DAY;

pub(super) struct SinceFiles {
    pub(super) kept: Vec<PathBuf>,
    pub(super) pruned: Vec<PathBuf>,
}

// NOTE(2026-09-25, #1794): Claude Code appends session JSONL, and a transcript can record a session ID different from its directory. Keep related transcripts together when pruning by mtime so a replay still has its parent.
pub(super) fn usage_files_before_since(
    paths: &[PathBuf],
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
    shared: &SharedArgs,
    now: TimestampMs,
) -> SinceFiles {
    let files = usage_files(paths, project_filter, diagnostics);
    let Some(threshold) = prune_threshold(shared, now) else {
        return SinceFiles {
            kept: files,
            pruned: Vec::new(),
        };
    };
    split_files(files, threshold)
}

fn prune_threshold(shared: &SharedArgs, now: TimestampMs) -> Option<i64> {
    let timezone = resolve_tz(shared.timezone.as_ref());
    let (since, _) = date_range_bounds_ms(shared.since.as_deref(), None, &timezone);
    since
        .filter(|since| *since <= now.as_millis())
        .map(|since| since.saturating_sub(MTIME_PRUNE_MARGIN_MS))
}

fn split_files(files: Vec<PathBuf>, threshold: i64) -> SinceFiles {
    let live = files
        .iter()
        .map(|file| file_modified_millis(file).is_none_or(|modified| modified >= threshold))
        .collect::<Vec<_>>();
    if live.iter().all(|&live| live) {
        return SinceFiles {
            kept: files,
            pruned: Vec::new(),
        };
    }
    if live.iter().all(|&live| !live) {
        return SinceFiles {
            kept: Vec::new(),
            pruned: files,
        };
    }

    let mut sessions = SessionGroups::default();
    let file_groups = files
        .iter()
        .map(|file| {
            let (path_session, _) = extract_session_parts(file);
            let group = sessions.id(&path_session);
            if let Some(recorded) = first_recorded_session_id(file) {
                let recorded = sessions.id(&recorded);
                sessions.union(group, recorded);
            }
            group
        })
        .collect::<Vec<_>>();
    let live_groups = live
        .into_iter()
        .zip(&file_groups)
        .filter(|(live, _)| *live)
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
mod tests {
    use crate::{MILLIS_PER_DAY, TimestampMs, cli::SharedArgs};
    use ccusage_test_support::{fs_fixture, instant, set_modified};
    use std::path::{Path, PathBuf};

    fn split_files_before_since(
        files: Vec<PathBuf>,
        shared: &SharedArgs,
        now: TimestampMs,
    ) -> super::SinceFiles {
        let Some(threshold) = super::prune_threshold(shared, now) else {
            return super::SinceFiles {
                kept: files,
                pruned: Vec::new(),
            };
        };
        super::split_files(files, threshold)
    }

    fn shared_since(since: Option<&str>) -> SharedArgs {
        SharedArgs {
            since: since.map(str::to_string),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
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
        set_modified(&old, instant("2026-09-18T12:00:00Z"));
        set_modified(&margin, instant("2026-09-19T12:00:00Z"));
        set_modified(&fresh, instant("2026-09-20T12:00:00Z"));
        vec![fresh, margin, old]
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_project_directory_is_reported_and_the_rest_still_found() {
        use std::{fs, os::unix::fs::PermissionsExt};

        use crate::{Diagnostics, SkipKind};

        let fixture = fs_fixture!({
            "projects/blocked/s.jsonl": "{}",
            "projects/healthy/s.jsonl": "{}",
        });
        let blocked = fixture.path("projects/blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::read_dir(&blocked).is_err();
        let diagnostics = Diagnostics::new("claude");
        let files = super::usage_files(&[fixture.root().to_path_buf()], None, &diagnostics);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            return;
        }

        assert_eq!(files, [fixture.path("projects/healthy/s.jsonl")]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_streaming_walk_reports_an_unreadable_project_directory() {
        use std::{fs, os::unix::fs::PermissionsExt};

        use crate::{Diagnostics, SkipKind};

        let fixture = fs_fixture!({
            "projects/blocked/s.jsonl": "{}",
            "projects/healthy/s.jsonl": "{}",
        });
        let blocked = fixture.path("projects/blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::read_dir(&blocked).is_err();
        let diagnostics = Diagnostics::new("claude");
        let mut files = Vec::new();
        super::visit_usage_files(
            &[fixture.root().to_path_buf()],
            None,
            &mut |path| files.push(path),
            &diagnostics,
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            return;
        }

        assert_eq!(files, [fixture.path("projects/healthy/s.jsonl")]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn lists_a_parent_transcript_before_its_subagent_transcripts() {
        let fixture = fs_fixture!({
            "projects/project1/session-a.jsonl": "",
            "projects/project1/session-a/subagents/agent-a.jsonl": "",
        });

        let files = super::usage_files(
            &[fixture.root().to_path_buf()],
            None,
            &crate::Diagnostics::new("claude"),
        );

        assert_eq!(
            files,
            [
                fixture.path("projects/project1/session-a.jsonl"),
                fixture.path("projects/project1/session-a/subagents/agent-a.jsonl"),
            ]
        );
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
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
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

        let kept = split_files_before_since(
            files,
            &shared_since(None),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
        )
        .kept;

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
        let now =
            TimestampMs::from_millis(instant("2026-09-30T00:00:00Z").millis() - MILLIS_PER_DAY);

        let kept = split_files_before_since(files, &shared_since(Some("20260930")), now).kept;

        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn keeps_files_whose_metadata_cannot_be_read() {
        let missing = PathBuf::from("/nonexistent/ccusage/projects/p/missing.jsonl");

        let kept = split_files_before_since(
            vec![missing.clone()],
            &shared_since(Some("20260920")),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
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
        set_modified(&live_parent, instant("2026-09-01T12:00:00Z"));
        set_modified(&live_agent, instant("2026-09-21T12:00:00Z"));
        set_modified(&stale_parent, instant("2026-09-01T12:00:00Z"));
        set_modified(&stale_agent, instant("2026-09-02T12:00:00Z"));

        let split = split_files_before_since(
            vec![
                live_parent.clone(),
                live_agent.clone(),
                stale_parent.clone(),
                stale_agent.clone(),
            ],
            &shared_since(Some("20260920")),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
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
        set_modified(&live, instant("2026-09-21T12:00:00Z"));
        for stale in [&live_agent, &stale_agent, &other_root_copy] {
            set_modified(stale, instant("2026-09-01T12:00:00Z"));
        }

        let split = split_files_before_since(
            vec![
                live_agent.clone(),
                stale_agent.clone(),
                live.clone(),
                other_root_copy.clone(),
            ],
            &shared_since(Some("20260920")),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
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
        set_modified(&parent, instant("2026-09-01T12:00:00Z"));
        set_modified(&workflow_agent, instant("2026-09-21T12:00:00Z"));

        let split = split_files_before_since(
            vec![parent.clone(), workflow_agent.clone()],
            &shared_since(Some("20260920")),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
        );

        assert_eq!(split.kept, [parent, workflow_agent]);
        assert!(split.pruned.is_empty());
    }

    #[test]
    fn reads_a_recorded_session_id_under_an_escaped_key() {
        let fixture = fs_fixture!({
            "projects/p/agent-1.jsonl": r#"{"sessionI\u0064":"parent"}"#,
        });

        assert_eq!(
            super::first_recorded_session_id(&fixture.root().join("projects/p/agent-1.jsonl"))
                .as_deref(),
            Some("parent")
        );
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
            super::first_recorded_session_id(&fixture.root().join("projects/p/agent-1.jsonl"))
                .as_deref(),
            Some("parent")
        );
    }
}
