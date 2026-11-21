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
    let roots = files
        .iter()
        .filter_map(|file| super::fixture_projects_root(file))
        .collect::<Vec<_>>();
    let split = super::read_files_since(
        shared.single_thread,
        super::prune_threshold(shared, now),
        &roots,
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
    for (file, written) in [
        (&old, "2026-09-18T12:00:00Z"),
        (&margin, "2026-09-19T12:00:00Z"),
        (&fresh, "2026-09-20T12:00:00Z"),
    ] {
        std::fs::write(file, record_at(written)).unwrap();
        set_modified(file, instant(written));
    }
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
fn picks_the_deepest_projects_root_for_a_nested_config_directory() {
    let roots = [
        PathBuf::from("/tmp/x/projects"),
        PathBuf::from("/tmp/x/projects/cfg/projects"),
    ];

    assert_eq!(
        super::containing_projects_root(
            Path::new("/tmp/x/projects/cfg/projects/myproj/s.jsonl"),
            &roots,
        ),
        Some(Path::new("/tmp/x/projects/cfg/projects"))
    );
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
        "projects/p/live.jsonl": record_at("2026-09-01T12:00:00Z"),
        "projects/p/live/subagents/agent-a.jsonl": "{}",
        "projects/p/stale.jsonl": record_at("2026-09-01T12:00:00Z"),
        "projects/p/stale/subagents/agent-b.jsonl": record_at("2026-09-02T12:00:00Z"),
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
        "b/projects/p/live.jsonl": record_at("2026-09-01T12:00:00Z"),
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
        "projects/p/session-y.jsonl": r#"{"sessionId":"session-y","timestamp":"2026-09-01T12:00:00Z"}"#,
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
        "b/projects/p/live.jsonl": r#"{"sessionId":"resumed","timestamp":"2026-09-01T12:00:00Z"}"#,
        "b/projects/p/resumed.jsonl": record_at("2026-09-01T12:00:00Z"),
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

#[test]
fn revives_a_stale_transcript_longer_than_its_tail_by_the_session_it_records() {
    let old = record_at("2026-09-01T12:00:00Z");
    let fixture = fs_fixture!({
        "projects/p/live.jsonl": record_at("2026-09-21T12:00:00Z"),
        "projects/p/agent-1.jsonl": format!(
            "{{\"sessionId\":\"live\",\"timestamp\":\"2026-09-01T12:00:00Z\"}}\n{}",
            vec![old; 32 * 1024 / 60].join("\n")
        ),
    });
    let live = fixture.path("projects/p/live.jsonl");
    let agent = fixture.path("projects/p/agent-1.jsonl");
    set_modified(&live, instant("2026-09-21T12:00:00Z"));
    set_modified(&agent, instant("2026-09-01T12:00:00Z"));

    let split = split_files_before_since(
        vec![live.clone(), agent.clone()],
        &shared_since(Some("20260920")),
        TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
    );

    assert_eq!(split.kept, [live, agent]);
}

#[test]
fn splits_a_window_that_keeps_or_prunes_every_file() {
    let fixture = fs_fixture!({
        "projects/p/old.jsonl": "{}",
        "projects/p/margin.jsonl": "{}",
        "projects/p/fresh.jsonl": "{}",
    });
    let files = fixture_files(fixture.root());
    let now = TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis());

    let keep_all = split_files_before_since(files.clone(), &shared_since(Some("20260918")), now);
    let prune_all = split_files_before_since(files.clone(), &shared_since(Some("20260923")), now);

    assert_eq!(keep_all.kept, files);
    assert!(keep_all.pruned.is_empty());
    assert!(prune_all.kept.is_empty());
    assert_eq!(prune_all.pruned, files);
}

fn stale_since_split(files: Vec<PathBuf>) -> Split {
    for file in &files {
        set_modified(file, instant("2026-09-01T12:00:00Z"));
    }
    split_files_before_since(
        files,
        &shared_since(Some("20260920")),
        TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
    )
}

fn record_at(timestamp: &str) -> String {
    format!(r#"{{"timestamp":"{timestamp}","message":{{"id":"m","usage":{{"input_tokens":1}}}}}}"#)
}

#[test]
fn keeps_a_stale_transcript_whose_tail_records_fall_inside_the_window() {
    let fixture = fs_fixture!({
        "projects/p/restored.jsonl": format!(
            "{}\n{}\n",
            record_at("2026-09-01T12:00:00Z"),
            record_at("2026-09-21T12:00:00Z")
        ),
    });
    let restored = fixture.path("projects/p/restored.jsonl");

    let split = stale_since_split(vec![restored.clone()]);

    assert_eq!(split.kept, [restored]);
    assert!(split.pruned.is_empty());
}

#[test]
fn keeps_stale_transcripts_with_in_window_tails_beside_live_and_pruned_ones() {
    let fixture = fs_fixture!({
        "projects/p/fresh.jsonl": record_at("2026-09-21T12:00:00Z"),
        "projects/p/restored.jsonl": record_at("2026-09-21T12:00:00Z"),
        "projects/p/old.jsonl": record_at("2026-09-01T12:00:00Z"),
    });
    let fresh = fixture.path("projects/p/fresh.jsonl");
    let restored = fixture.path("projects/p/restored.jsonl");
    let old = fixture.path("projects/p/old.jsonl");

    set_modified(&fresh, instant("2026-09-21T12:00:00Z"));
    set_modified(&restored, instant("2026-09-01T12:00:00Z"));
    set_modified(&old, instant("2026-09-01T12:00:00Z"));

    let split = split_files_before_since(
        vec![fresh.clone(), restored.clone(), old.clone()],
        &shared_since(Some("20260920")),
        TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
    );

    assert_eq!(split.kept, [fresh, restored]);
    assert_eq!(split.pruned, [old]);
}

#[test]
fn judges_a_stale_transcript_by_the_records_in_its_last_16_kib_only() {
    let padding = record_at("2026-09-01T12:00:00Z");
    let lines = std::iter::once(record_at("2026-09-21T12:00:00Z"))
        .chain(std::iter::repeat_n(padding, 32 * 1024 / 60))
        .collect::<Vec<_>>();
    let fixture = fs_fixture!({
        "projects/p/old.jsonl": lines.join("\n"),
    });
    let old = fixture.path("projects/p/old.jsonl");

    let split = stale_since_split(vec![old.clone()]);

    assert!(split.kept.is_empty());
    assert_eq!(split.pruned, [old]);
}

#[test]
fn reads_the_latest_record_after_a_tail_that_starts_mid_line() {
    let fixture = fs_fixture!({
        "projects/p/restored.jsonl": format!(
            r#"{{"timestamp":"2026-09-01T12:00:00Z","text":"{}"}}{}{}"#,
            "x".repeat(32 * 1024),
            "\n",
            record_at("2026-09-21T12:00:00Z")
        ),
    });
    let restored = fixture.path("projects/p/restored.jsonl");

    let split = stale_since_split(vec![restored.clone()]);

    assert_eq!(split.kept, [restored]);
}

fn records_then_undated(last_record: &str, undated_bytes: usize) -> String {
    let undated = format!(r#"{{"type":"progress","text":"{}"}}"#, "x".repeat(1000));
    std::iter::repeat_n(record_at("2026-09-01T12:00:00Z"), 200 * 1024 / 60)
        .chain([record_at(last_record)])
        .chain(std::iter::repeat_n(undated, undated_bytes / 1024 + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn nested_time_line(timestamp: &str) -> String {
    format!(
        r#"{{"type":"file-history-snapshot","snapshot":{{"timestamp":"{timestamp}","text":"{}"}}}}"#,
        "x".repeat(1024)
    )
}

fn trailer_without_timestamps() -> String {
    format!(
        r#"{{"type":"file-history-snapshot","snapshot":{{"timestamp":"2026-09-21T12:00:00Z","text":"{}"}}}}"#,
        "x".repeat(32 * 1024)
    )
}

#[test]
fn judges_a_stale_transcript_by_its_latest_record_before_a_tail_without_timestamps() {
    let long_line = |timestamp: &str| {
        format!(
            r#"{{"timestamp":"{timestamp}","text":"{}"}}"#,
            "x".repeat(32 * 1024)
        )
    };
    let cases = [
        (
            "in-window long line",
            long_line("2026-09-21T12:00:00Z"),
            true,
        ),
        ("old long line", long_line("2026-09-01T12:00:00Z"), false),
        (
            "in-window record, then a trailer",
            format!(
                "{}\n{}\n",
                record_at("2026-09-21T12:00:00Z"),
                trailer_without_timestamps()
            ),
            true,
        ),
        (
            "old record, then a trailer",
            format!(
                "{}\n{}\n",
                record_at("2026-09-01T12:00:00Z"),
                trailer_without_timestamps()
            ),
            false,
        ),
        (
            "in-window record under an escaped key",
            r#"{"\u0074imestamp":"2026-09-21T12:00:00Z"}"#.to_string(),
            true,
        ),
        (
            "in-window record under an escaped key after older records",
            format!(
                "{}\n{}",
                record_at("2026-09-01T12:00:00Z"),
                r#"{"\u0074imestamp":"2026-09-21T12:00:00Z"}"#
            ),
            true,
        ),
        (
            "in-window record beside control-character escapes",
            format!(
                "{}\n{}",
                r#"{"timestamp":"2026-09-01T12:00:00Z","text":"\u001b[0m"}"#,
                record_at("2026-09-21T12:00:00Z")
            ),
            true,
        ),
        (
            "in-window record before 16 KiB of older nested times",
            format!(
                "{}\n{}",
                record_at("2026-09-21T12:00:00Z"),
                vec![nested_time_line("2026-09-01T12:00:00Z"); 20].join("\n")
            ),
            true,
        ),
        (
            "older record before 16 KiB of older nested times",
            format!(
                "{}\n{}",
                record_at("2026-09-01T12:00:00Z"),
                vec![nested_time_line("2026-09-01T12:00:00Z"); 20].join("\n")
            ),
            false,
        ),
        (
            "in-window record whose timestamp carries an escape",
            r#"{"timestamp":"2026-09-21T12:00:00\u005a","message":{"id":"m","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#.to_string(),
            true,
        ),
        (
            "in-window record three reads back",
            records_then_undated("2026-09-21T12:00:00Z", 64 * 1024),
            true,
        ),
        (
            "older record three reads back",
            records_then_undated("2026-09-01T12:00:00Z", 64 * 1024),
            false,
        ),
        (
            "in-window record behind more than the read limit of undated lines",
            records_then_undated("2026-09-21T12:00:00Z", 3 * 1024 * 1024),
            true,
        ),
        (
            "older record behind more than the read limit of undated lines",
            records_then_undated("2026-09-01T12:00:00Z", 3 * 1024 * 1024),
            false,
        ),
        (
            "undated last line longer than the read limit",
            format!(
                "{}\n{{\"text\":\"{}\"}}",
                record_at("2026-09-01T12:00:00Z"),
                "x".repeat(2 * 1024 * 1024)
            ),
            true,
        ),
        (
            "no record timestamps",
            r#"{"type":"summary"}"#.to_string(),
            false,
        ),
    ];
    let fixture = fs_fixture!({ "projects/p/.keep": "" });
    for (index, (name, content, kept)) in cases.into_iter().enumerate() {
        let file = fixture.path(format!("projects/p/{index}.jsonl"));
        std::fs::write(&file, content).unwrap();

        let split = stale_since_split(vec![file.clone()]);

        assert_eq!(split.kept.is_empty(), !kept, "{name}");
    }
}

#[test]
fn keeps_the_probe_buffer_bounded_on_a_large_undated_file() {
    let fixture = fs_fixture!({
        "projects/p/undated.jsonl": records_then_undated("2026-09-01T12:00:00Z", 8 * 1024 * 1024),
    });
    let undated = fixture.path("projects/p/undated.jsonl");
    set_modified(&undated, instant("2026-09-01T12:00:00Z"));
    let mut probe = super::StaleProbe::new(instant("2026-09-19T00:00:00Z").millis());

    let (live, _) = probe.file(&undated);

    assert!(!live);
    let limit = usize::try_from(super::PROBE_READ_LIMIT).unwrap();
    assert!(
        probe.tail.capacity() < 3 * limit,
        "{}",
        probe.tail.capacity()
    );
}

#[test]
fn revives_a_stale_transcript_whose_recorded_session_follows_a_head_longer_than_the_probe() {
    let fixture = fs_fixture!({
        "projects/p/live.jsonl": record_at("2026-09-21T12:00:00Z"),
        "projects/p/agent-1.jsonl": format!(
            "{{\"type\":\"summary\",\"text\":\"{}\"}}\n{{\"sessionId\":\"live\",\"timestamp\":\"2026-09-01T12:00:00Z\"}}\n{}",
            "x".repeat(100 * 1024),
            record_at("2026-09-01T12:00:00Z")
        ),
    });
    let live = fixture.path("projects/p/live.jsonl");
    let agent = fixture.path("projects/p/agent-1.jsonl");
    set_modified(&live, instant("2026-09-21T12:00:00Z"));
    set_modified(&agent, instant("2026-09-01T12:00:00Z"));
    let mut probe = super::StaleProbe::new(instant("2026-09-19T00:00:00Z").millis());
    assert_eq!(probe.file(&agent), (false, None));

    let split = split_files_before_since(
        vec![live.clone(), agent.clone()],
        &shared_since(Some("20260920")),
        TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
    );

    assert_eq!(split.kept, [live, agent]);
}

#[test]
fn reads_no_stale_session_group_for_a_transcript_whose_tail_has_no_timestamps() {
    let old = record_at("2026-09-01T12:00:00Z");
    let fixture = fs_fixture!({
        "projects/p/live.jsonl": record_at("2026-09-21T12:00:00Z"),
        "projects/p/stale.jsonl": format!("{old}\n{}\n", trailer_without_timestamps()),
        "projects/p/stale/subagents/agent-a.jsonl": old.clone(),
        "projects/p/stale/subagents/agent-b.jsonl": old.clone(),
        "projects/p/stale/subagents/agent-c.jsonl": old,
    });
    let files = [
        "live.jsonl",
        "stale.jsonl",
        "stale/subagents/agent-a.jsonl",
        "stale/subagents/agent-b.jsonl",
        "stale/subagents/agent-c.jsonl",
    ]
    .map(|name| fixture.path(format!("projects/p/{name}")));
    set_modified(&files[0], instant("2026-09-21T12:00:00Z"));
    for stale in &files[1..] {
        set_modified(stale, instant("2026-09-01T12:00:00Z"));
    }
    let parsed = std::sync::Mutex::new(Vec::new());

    super::read_files_since(
        false,
        super::prune_threshold(
            &shared_since(Some("20260920")),
            TimestampMs::from_millis(instant("2026-09-24T00:00:00Z").millis()),
        ),
        &[fixture.path("projects")],
        |visit| files.iter().cloned().for_each(visit),
        |file| parsed.lock().unwrap().push(file.clone()),
    );

    assert_eq!(parsed.into_inner().unwrap(), [files[0].clone()]);
}

#[test]
fn keeps_the_record_a_tail_starts_on_exactly() {
    let in_window = format!("{}\n", record_at("2026-09-21T12:00:00Z"));
    let old_line =
        |text: &str| format!(r#"{{"timestamp":"2026-09-01T12:00:00Z","text":"{text}"}}"#) + "\n";
    let padding = 16 * 1024 - in_window.len() - old_line("").len();
    let tail = in_window + &old_line(&"x".repeat(padding));
    assert_eq!(tail.len(), 16 * 1024);
    let fixture = fs_fixture!({
        "projects/p/restored.jsonl": old_line("") + &tail,
    });
    let restored = fixture.path("projects/p/restored.jsonl");

    let split = stale_since_split(vec![restored.clone()]);

    assert_eq!(split.kept, [restored]);
}

#[test]
fn reads_a_file_from_its_first_byte_as_whole_lines() {
    let fixture = fs_fixture!({ "projects/p/.keep": "" });
    let in_window = format!("{}\n", record_at("2026-09-21T12:00:00Z"));
    let old_line =
        |text: &str| format!(r#"{{"timestamp":"2026-09-01T12:00:00Z","text":"{text}"}}"#) + "\n";
    for (len, kept) in [
        (16 * 1024, true),
        (16 * 1024 + 1, true),
        (16 * 1024 + 2, false),
    ] {
        let padding = len - in_window.len() - old_line("").len();
        let content = in_window.clone() + &old_line(&"x".repeat(padding));
        assert_eq!(content.len(), len);
        let file = fixture.path(format!("projects/p/{len}.jsonl"));
        std::fs::write(&file, content).unwrap();

        let split = stale_since_split(vec![file.clone()]);

        assert_eq!(split.kept.is_empty(), !kept, "{len} bytes");
    }
}
