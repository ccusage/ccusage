use crate::{MILLIS_PER_DAY, TimestampMs, cli::SharedArgs};
use ccusage_test_support::{fs_fixture, instant, read_fixture_dir, set_modified};
use std::path::{Path, PathBuf};

struct Split {
    kept: Vec<PathBuf>,
    pruned: Vec<PathBuf>,
}

fn split_files_before_since(files: Vec<PathBuf>, shared: &SharedArgs, now: TimestampMs) -> Split {
    let in_input_order = |mut split: Vec<PathBuf>| {
        split.sort_by_key(|file| files.iter().position(|input| input == file));
        split
    };
    let split = super::read_files_since(
        shared.single_thread,
        super::prune_threshold(shared, now),
        |visit| files.iter().cloned().for_each(visit),
        |file| file.clone(),
    );
    assert!(split.kept.iter().all(|(file, read)| file == read));
    Split {
        kept: in_input_order(split.kept.into_iter().map(|(file, _)| file).collect()),
        pruned: in_input_order(split.pruned),
    }
}

fn shared_since(since: Option<&str>) -> SharedArgs {
    SharedArgs {
        since: since.map(|date| date.parse().unwrap()),
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

    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = fs_fixture!({
        "projects/blocked/s.jsonl": "{}",
        "projects/healthy/s.jsonl": "{}",
    });
    let blocked = fixture.path("projects/blocked");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = read_fixture_dir(&blocked).is_err();
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

    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = fs_fixture!({
        "projects/blocked/s.jsonl": "{}",
        "projects/healthy/s.jsonl": "{}",
    });
    let blocked = fixture.path("projects/blocked");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = read_fixture_dir(&blocked).is_err();
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
    let now = TimestampMs::from_millis(instant("2026-09-30T00:00:00Z").millis() - MILLIS_PER_DAY);

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
    let workflow_agent = root.join("projects/p/session-x/subagents/workflows/wf_1/agent-a.jsonl");
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

#[test]
fn keeps_a_stale_transcript_whose_recorded_session_links_it_after_its_live_sibling() {
    let fixture = fs_fixture!({
        "a/projects/p/live.jsonl": "{}",
        "b/projects/p/live.jsonl": r#"{"sessionId":"resumed"}"#,
        "b/projects/p/resumed.jsonl": "{}",
    });
    let root = fixture.root();
    let live = root.join("a/projects/p/live.jsonl");
    let stale_copy = root.join("b/projects/p/live.jsonl");
    let stale_resumed = root.join("b/projects/p/resumed.jsonl");
    set_modified(&live, instant("2026-09-21T12:00:00Z"));
    set_modified(&stale_copy, instant("2026-09-01T12:00:00Z"));
    set_modified(&stale_resumed, instant("2026-09-01T12:00:00Z"));

    let split = split_files_before_since(
        vec![live.clone(), stale_copy.clone(), stale_resumed.clone()],
        &shared_since(Some("20260920")),
        TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
    );

    assert_eq!(split.kept, [live, stale_copy, stale_resumed]);
    assert!(split.pruned.is_empty());
}
