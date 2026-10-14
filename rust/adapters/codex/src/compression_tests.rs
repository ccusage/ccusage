use std::{fs, path::PathBuf};

use ccusage_test_support::{EnvVarGuard, Fixture};
use serde_json::json;

use crate::{
    PricingMap,
    aggregate::load_groups,
    cli::{AgentReportKind, CodexSpeed, SharedArgs},
    load_codex_events_from_directory, load_codex_events_with_detection, paths, report_json,
};

fn write_compressed(fixture: &Fixture, path: &str, contents: &str) -> PathBuf {
    let path = fixture.write_file(path, "");
    fs::write(
        &path,
        zstd::stream::encode_all(contents.as_bytes(), 3).unwrap(),
    )
    .unwrap();
    path
}

fn usage_log(timestamp: &str, input_tokens: u64) -> String {
    [
        json!({"timestamp": timestamp, "type": "turn_context", "payload": {"model": "gpt-5"}}),
        json!({"timestamp": timestamp, "type": "event_msg", "payload": {
            "type": "token_count", "info": {"last_token_usage": {
                "input_tokens": input_tokens, "output_tokens": 20,
                "total_tokens": input_tokens + 20,
            }},
        }}),
    ]
    .map(|line| line.to_string())
    .join("\n")
}

fn cumulative_count(timestamp: &str, input: u64, cached: u64, output: u64) -> String {
    json!({"timestamp": timestamp, "type": "event_msg", "payload": {
        "type": "token_count", "info": {"total_token_usage": {
            "input_tokens": input, "cached_input_tokens": cached,
            "output_tokens": output, "total_tokens": input + output,
        }},
    }})
    .to_string()
}

fn load_from_codex_home(
    fixture: &Fixture,
    single_thread: bool,
) -> Vec<crate::CodexTokenUsageEvent> {
    let _guard = EnvVarGuard::set("CODEX_HOME", fixture.path("codex"));
    let (events, detected) = load_codex_events_with_detection(&SharedArgs {
        single_thread,
        ..SharedArgs::default()
    })
    .unwrap();
    assert!(detected);
    events
}

fn total_tokens(events: &[crate::CodexTokenUsageEvent]) -> u64 {
    events.iter().map(|event| event.total_tokens).sum()
}

#[test]
fn loads_compressed_rollouts_with_the_plain_session_id() {
    let fixture = Fixture::new();
    let _ = write_compressed(
        &fixture,
        "2026/10/14/rollout.jsonl.zst",
        &[
            json!({"timestamp": "2026-10-14T00:00:00Z", "type": "turn_context",
                "payload": {"model": "gpt-5"}})
            .to_string(),
            json!({"timestamp": "2026-10-14T00:00:01Z", "type": "event_msg", "payload": {
                "type": "token_count", "info": {"total_token_usage": {
                    "input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 20,
                    "reasoning_output_tokens": 5, "total_tokens": 120,
                }},
            }})
            .to_string(),
        ]
        .join("\n"),
    );

    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.session_id, "2026/10/14/rollout");
    assert_eq!(event.model.as_deref(), Some("gpt-5"));
    assert_eq!(event.input_tokens, 100);
    assert_eq!(event.cached_input_tokens, 80);
    assert_eq!(event.output_tokens, 20);
    assert_eq!(event.reasoning_output_tokens, 5);
    assert_eq!(event.total_tokens, 120);
}

#[test]
fn discovers_compressed_rollouts_in_sessions_and_archived_sessions() {
    let fixture = Fixture::new();
    let _ = write_compressed(
        &fixture,
        "codex/sessions/2026/10/14/active.jsonl.zst",
        &usage_log("2026-10-14T00:00:00Z", 100),
    );
    let _ = write_compressed(
        &fixture,
        "codex/archived_sessions/archived.jsonl.zst",
        &usage_log("2026-10-14T00:01:00Z", 200),
    );
    let _ = fixture.write_file(
        "codex/archived_sessions/plain.jsonl",
        usage_log("2026-10-14T00:02:00Z", 300),
    );
    let _ = fixture.write_file("codex/sessions/notes.zst", "not a rollout");

    for single_thread in [true, false] {
        let events = load_from_codex_home(&fixture, single_thread);

        let mut session_ids = events
            .iter()
            .map(|event| event.session_id.as_str())
            .collect::<Vec<_>>();
        session_ids.sort_unstable();
        assert_eq!(session_ids, ["2026/10/14/active", "archived", "plain"]);
        assert_eq!(total_tokens(&events), 660);
    }
}

#[test]
fn counts_a_rollout_with_plain_and_compressed_copies_once() {
    let fixture = Fixture::new();
    let _ = fixture.write_file("session.jsonl", usage_log("2026-10-14T00:00:10Z", 100));
    let _ = write_compressed(
        &fixture,
        "session.jsonl.zst",
        &usage_log("2026-10-14T00:00:01Z", 400),
    );

    assert_eq!(
        paths::collect_codex_usage_files(fixture.root()),
        vec![fixture.path("session.jsonl")]
    );
    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "session");
    assert_eq!(events[0].total_tokens, 120);
}

#[test]
fn keeps_active_rollout_when_archive_uses_a_different_encoding() {
    let fixture = Fixture::new();
    let _ = write_compressed(
        &fixture,
        "codex/sessions/session.jsonl.zst",
        &usage_log("2026-10-14T00:00:10Z", 100),
    );
    let _ = fixture.write_file(
        "codex/archived_sessions/session.jsonl",
        usage_log("2026-10-14T00:00:01Z", 400),
    );

    let events = load_from_codex_home(&fixture, true);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 120);
}

#[test]
fn counts_cumulative_deltas_across_concatenated_zstd_frames() {
    let fixture = Fixture::new();
    let path = write_compressed(
        &fixture,
        "session.jsonl.zst",
        &[
            json!({"type": "turn_context", "payload": {"model": "gpt-5"}}).to_string(),
            cumulative_count("2026-10-14T00:00:01Z", 100, 80, 20),
            String::new(),
        ]
        .join("\n"),
    );
    let second_frame = [
        cumulative_count("2026-10-14T00:00:02Z", 160, 110, 40),
        cumulative_count("2026-10-14T00:00:03Z", 160, 110, 40),
    ]
    .join("\n");
    let mut compressed = fs::read(&path).unwrap();
    compressed.extend(zstd::stream::encode_all(second_frame.as_bytes(), 3).unwrap());
    fs::write(path, compressed).unwrap();

    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events[1].input_tokens, 60);
    assert_eq!(events[1].cached_input_tokens, 30);
    assert_eq!(events[1].output_tokens, 20);
    assert_eq!(total_tokens(&events), 200);
}

#[test]
fn keeps_usage_decoded_before_a_truncated_frame() {
    let fixture = Fixture::new();
    let complete = usage_log("2026-10-14T00:00:00Z", 100) + "\n";
    let mut compressed = zstd::stream::encode_all(complete.as_bytes(), 3).unwrap();
    let truncated = zstd::stream::encode_all(
        format!("{}\n", usage_log("2026-10-14T00:01:00Z", 200)).as_bytes(),
        3,
    )
    .unwrap();
    compressed.extend(&truncated[..truncated.len() / 2]);
    let path = fixture.write_file("session.jsonl.zst", "");
    fs::write(path, compressed).unwrap();

    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 120);
}

#[test]
fn reads_the_other_encoding_when_codex_migrates_after_discovery() {
    let fixture = Fixture::new();
    let plain = fixture.write_file("session.jsonl", usage_log("2026-10-14T00:00:00Z", 100));
    let files = paths::collect_codex_usage_files(fixture.root());
    fs::remove_file(&plain).unwrap();
    let _ = write_compressed(
        &fixture,
        "session.jsonl.zst",
        &usage_log("2026-10-14T00:00:00Z", 100),
    );

    assert_eq!(files, vec![plain]);
    let mut visited = Vec::new();
    crate::parser::visit_codex_session_file(fixture.root(), &files[0], None, |event| {
        visited.push(event);
        Ok(())
    })
    .unwrap();
    assert_eq!(visited.len(), 1);
    assert_eq!(visited[0].session_id, "session");
    assert_eq!(visited[0].total_tokens, 120);
}

#[test]
fn dates_headless_usage_from_the_encoding_opened_after_migration() {
    let fixture = Fixture::new();
    let headless = json!({"type": "turn.completed", "usage": {
        "input_tokens": 100, "output_tokens": 20, "total_tokens": 120,
    }})
    .to_string();
    let plain = fixture.write_file("run.jsonl", &headless);
    let files = paths::collect_codex_usage_files(fixture.root());
    fs::remove_file(&plain).unwrap();
    let compressed = write_compressed(&fixture, "run.jsonl.zst", &headless);
    paths::set_file_modified(
        &compressed,
        crate::parse_ts_timestamp("2026-10-14T12:00:00.000Z").unwrap(),
    );

    let mut visited = Vec::new();
    crate::parser::visit_codex_session_file(fixture.root(), &files[0], None, |event| {
        visited.push(event);
        Ok(())
    })
    .unwrap();

    assert_eq!(visited.len(), 1);
    assert_eq!(visited[0].timestamp, "2026-10-14T12:00:00.000Z");
}

#[test]
fn removes_inherited_usage_when_parent_and_child_are_compressed() {
    let fixture = Fixture::new();
    let _ = write_compressed(
        &fixture,
        "a-parent.jsonl.zst",
        &[
            json!({"type": "session_meta", "payload": {"id": "parent"}}).to_string(),
            usage_log("2026-10-14T00:00:00Z", 100),
        ]
        .join("\n"),
    );
    let _ = write_compressed(
        &fixture,
        "b-child.jsonl.zst",
        &[
            json!({"timestamp": "2026-10-14T00:01:00Z", "type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "parent"}})
            .to_string(),
            usage_log("2026-10-14T00:01:00Z", 100),
            usage_log("2026-10-14T00:02:00Z", 50),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let events = load_codex_events_from_directory(fixture.root(), single_thread).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(total_tokens(&events), 190);
        let child = events
            .iter()
            .find(|event| event.session_id == "b-child")
            .unwrap();
        assert_eq!(child.total_tokens, 70);
    }
}

#[test]
fn uses_a_compressed_archived_parent_outside_the_selected_date_range() {
    let fixture = Fixture::new();
    let parent = write_compressed(
        &fixture,
        "codex/archived_sessions/2026/09/01/parent.jsonl.zst",
        &[
            json!({"type": "session_meta", "payload": {"id": "parent"}}).to_string(),
            usage_log("2026-09-01T00:00:00Z", 100),
        ]
        .join("\n"),
    );
    paths::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2026-09-01T00:00:01Z").unwrap(),
    );
    let _ = fixture.write_file(
        "codex/sessions/2026/10/14/child.jsonl",
        [
            json!({"timestamp": "2026-10-14T00:01:00Z", "type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "parent"}})
            .to_string(),
            usage_log("2026-10-14T00:01:00Z", 100),
            usage_log("2026-10-14T00:02:00Z", 50),
        ]
        .join("\n"),
    );
    let _guard = EnvVarGuard::set("CODEX_HOME", fixture.path("codex"));

    for single_thread in [true, false] {
        let shared = SharedArgs {
            since: Some("20261014".into()),
            timezone: Some("UTC".into()),
            single_thread,
            ..SharedArgs::default()
        };
        let (events, detected) = load_codex_events_with_detection(&shared).unwrap();
        assert!(detected);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 70);
        let groups = load_groups(&shared, AgentReportKind::Monthly).unwrap();
        assert_eq!(groups["2026-10"].total_tokens, 70);
    }
}

#[test]
fn detects_a_rewritten_burst_in_a_compressed_child_without_its_parent() {
    let fixture = Fixture::new();
    let _ = write_compressed(
        &fixture,
        "child.jsonl.zst",
        &[
            json!({"timestamp": "2026-10-14T00:00:10Z", "type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "missing-parent"}})
            .to_string(),
            usage_log("2026-10-14T00:00:10.000Z", 100),
            usage_log("2026-10-14T00:00:10.010Z", 200),
            usage_log("2026-10-14T00:00:12Z", 50),
        ]
        .join("\n"),
    );

    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 70);
}

#[test]
fn counts_compaction_records_in_a_compressed_archive_once() {
    let fixture = Fixture::new();
    let record = json!({"timestamp": "2026-10-14T00:00:02Z", "type": "token_usage_record",
    "payload": {
        "response_id": "compaction-1",
        "usage": {"input_tokens": 500, "output_tokens": 20},
    }})
    .to_string();
    let marker = json!({"type": "compacted", "payload": {
        "compaction_response_id": "compaction-1",
    }})
    .to_string();
    let _ = write_compressed(
        &fixture,
        "codex/archived_sessions/session.jsonl.zst",
        &[
            usage_log("2026-10-14T00:00:01Z", 100),
            record.clone(),
            marker.clone(),
            record,
            marker,
        ]
        .join("\n"),
    );

    let events = load_from_codex_home(&fixture, true);

    assert_eq!(events.len(), 2);
    assert_eq!(total_tokens(&events), 640);
}

#[test]
fn compressed_rollouts_match_plain_json_reports_for_every_report_mode() {
    let fixture = Fixture::new();
    let log = [
        usage_log("2026-02-01T00:00:00Z", 100).replace("gpt-5", "gpt-5.3-codex"),
        usage_log("2026-10-14T00:00:00Z", 50),
    ]
    .join("\n");
    let _ = fixture.write_file("plain/session.jsonl", &log);
    let _ = write_compressed(&fixture, "compressed/session.jsonl.zst", &log);
    let plain = load_codex_events_from_directory(&fixture.path("plain"), true).unwrap();
    let compressed = load_codex_events_from_directory(&fixture.path("compressed"), true).unwrap();
    let pricing = PricingMap::load_embedded();

    for kind in [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
        AgentReportKind::Session,
    ] {
        let expected =
            report_json(&plain, kind, Some("UTC"), &pricing, CodexSpeed::Standard).unwrap();
        let actual = report_json(
            &compressed,
            kind,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();
        assert_eq!(actual["totals"]["totalTokens"], 190);
        assert_eq!(actual, expected);
    }
}
