use super::*;
use crate::paths::{self, CodexUsageFileGroup};

const BEFORE_SINCE: &str = "2025-01-01T08:00:00.000Z";
const IN_WINDOW: &str = "2026-03-15T09:00:00.000Z";

#[test]
fn bounded_loading_skips_an_archived_rollout_last_modified_before_since() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "archived_sessions/stale.jsonl": [
            replay_metadata(BEFORE_SINCE, "stale", None),
            replay_token_count(BEFORE_SINCE, 999),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
        ]
        .join("\n"),
        "sessions/2026/03/15/current.jsonl": [
            replay_metadata(IN_WINDOW, "current", None),
            replay_token_count(IN_WINDOW, 50),
        ]
        .join("\n"),
    });
    set_modified(&fixture, "archived_sessions/stale.jsonl", BEFORE_SINCE);
    set_modified(&fixture, "sessions/2026/03/15/current.jsonl", IN_WINDOW);

    for single_thread in [true, false] {
        let diagnostics = Diagnostics::new("codex");
        let (sessions, _) = session_input_tokens(
            &home_sources(&fixture),
            &bounded_to_march_15(single_thread),
            &diagnostics,
        );

        assert_eq!(
            sessions,
            [("2026/03/15/current".to_string(), 50)],
            "single_thread={single_thread}"
        );
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
            0,
            "the stale archived rollout was parsed, single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_keeps_an_archived_rollout_resumed_inside_the_window() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "archived_sessions/resumed.jsonl": [
            replay_metadata(BEFORE_SINCE, "resumed", None),
            replay_token_count(BEFORE_SINCE, 999),
            replay_token_count(IN_WINDOW, 40),
        ]
        .join("\n"),
        "sessions/.keep": "",
    });
    set_modified(&fixture, "archived_sessions/resumed.jsonl", IN_WINDOW);

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &home_sources(&fixture),
            &bounded_to_march_15(single_thread),
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("resumed".to_string(), 40)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_replays_an_archived_parent_last_modified_before_since() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "archived_sessions/parent.jsonl": [
            replay_metadata(BEFORE_SINCE, "parent", None),
            replay_token_count(BEFORE_SINCE, 100),
        ]
        .join("\n"),
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata(IN_WINDOW, "child", Some("parent")),
            replay_token_count(IN_WINDOW, 100),
            replay_token_count(IN_WINDOW, 50),
        ]
        .join("\n"),
    });
    set_modified(&fixture, "archived_sessions/parent.jsonl", BEFORE_SINCE);
    set_modified(&fixture, "sessions/2026/03/15/child.jsonl", IN_WINDOW);
    let expected = [("2026/03/15/child".to_string(), 50)];

    for single_thread in [true, false] {
        let shared = bounded_to_march_15(single_thread);
        let sources = home_sources(&fixture);
        let (discovered, _) = session_input_tokens(&sources, &shared, &Diagnostics::new("codex"));
        let selected = selected_session_input_tokens(&sources, &shared);

        assert_eq!(discovered, expected, "single_thread={single_thread}");
        assert_eq!(selected, expected, "single_thread={single_thread}");
    }
}

fn home_sources(fixture: &ccusage_test_support::Fixture) -> [CodexUsageSource; 2] {
    let home = fixture.root().to_path_buf();
    [
        CodexUsageSource::new(fixture.path("sessions"), home.clone()),
        CodexUsageSource::new(fixture.path("archived_sessions"), home),
    ]
}

fn bounded_to_march_15(single_thread: bool) -> SharedArgs {
    SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        single_thread,
        ..SharedArgs::default()
    }
}

fn set_modified(fixture: &ccusage_test_support::Fixture, path: &str, timestamp: &str) {
    crate::test_support::set_file_modified(
        &fixture.path(path),
        crate::parse_ts_timestamp(timestamp).unwrap(),
    );
}

fn selected_session_input_tokens(
    sources: &[CodexUsageSource],
    shared: &SharedArgs,
) -> Vec<(String, u64)> {
    let diagnostics = Diagnostics::new("codex");
    let groups = paths::collect_deduped_codex_usage_files(sources, &diagnostics);
    let selected = groups
        .iter()
        .map(|CodexUsageFileGroup { source, files }| {
            paths::filter_codex_usage_files(source.dir(), files, shared)
        })
        .collect::<Vec<_>>();
    crate::aggregate::load_groups_from_file_selection(
        &groups,
        &selected,
        shared,
        AgentReportKind::Session,
        Some(&crate::PricingMap::load_embedded()),
        &diagnostics,
    )
    .unwrap()
    .into_iter()
    .map(|(session, group)| (session, group.input_tokens))
    .collect()
}
