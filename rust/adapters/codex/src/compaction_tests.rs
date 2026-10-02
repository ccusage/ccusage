use ccusage_test_support::fs_fixture;
use serde_json::{Value, json};

use crate::{
    PricingMap,
    aggregate::{aggregate_events, load_groups_from_directory},
    cli::{AgentReportKind, CodexSpeed, CostMode, SharedArgs},
    load_codex_events_from_directory,
    parser::visit_codex_session_file,
    report::report_from_groups,
    types::CodexRawUsage,
};

fn compaction_log(timestamp: &str, model: &str, input_tokens: u64) -> String {
    [
        json!({"timestamp": timestamp, "type": "turn_context", "payload": {"model": model}}),
        json!({"timestamp": timestamp, "type": "token_usage_record", "payload": {
            "response_id": "response-1", "usage": {"input_tokens": input_tokens, "output_tokens": 20}
        }}),
        json!({"type": "compacted", "payload": {"compaction_response_id": "response-1"}}),
    ].map(|line| line.to_string()).join("\n")
}

fn token_count(timestamp: &str, input_tokens: u64) -> String {
    json!({"timestamp": timestamp, "type": "event_msg", "payload": {
        "type": "token_count", "info": {"model": "gpt-5", "last_token_usage": {
            "input_tokens": input_tokens, "output_tokens": 20
        }}
    }})
    .to_string()
}

#[test]
fn compaction_identity_does_not_consume_the_normal_replay_prefix() {
    let fixture = fs_fixture!({
        "child.jsonl": [
            compaction_log("2026-09-01T00:00:01Z", "gpt-5", 100),
            token_count("2026-09-01T00:00:02Z", 100),
            token_count("2026-09-01T00:01:00Z", 50),
        ].join("\n"),
    });
    let prefix = [CodexRawUsage {
        input_tokens: 100,
        output_tokens: 20,
        total_tokens: 120,
        ..CodexRawUsage::default()
    }];
    let mut events = Vec::new();
    visit_codex_session_file(
        fixture.root(),
        &fixture.path("child.jsonl"),
        Some(&prefix),
        |event| {
            events.push(event);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].response_id.as_deref(), Some("response-1"));
    assert_eq!(events[1].input_tokens, 50);
}

#[test]
fn parent_compaction_records_do_not_change_the_normal_replay_prefix() {
    let fixture = fs_fixture!({
        "a-parent.jsonl": [
            json!({"type": "session_meta", "payload": {"id": "parent"}}).to_string(),
            compaction_log("2026-09-01T00:00:01Z", "gpt-5", 300),
            token_count("2026-09-01T00:00:02Z", 100),
        ].join("\n"),
        "b-child.jsonl": [
            json!({"timestamp": "2026-09-01T00:01:00Z", "type": "session_meta", "payload": {"id": "child", "forked_from_id": "parent"}}).to_string(),
            token_count("2026-09-01T00:01:00Z", 100),
            token_count("2026-09-01T00:02:00Z", 50),
        ].join("\n"),
    });
    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events.iter().map(|event| event.total_tokens).sum::<u64>(),
        510
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.session_id == "b-child")
            .count(),
        1
    );
}

#[test]
fn bounded_reports_exclude_parent_compactions_but_keep_child_requests() {
    let fixture = fs_fixture!({
        "2026/09/01/parent.jsonl": [
            json!({"type": "session_meta", "payload": {"id": "parent"}}).to_string(),
            compaction_log("2026-09-01T00:00:01Z", "gpt-5", 300),
            token_count("2026-09-01T00:00:02Z", 100),
        ].join("\n"),
        "2026/09/02/child.jsonl": [
            json!({"timestamp": "2026-09-02T00:00:00Z", "type": "session_meta", "payload": {"id": "child", "forked_from_id": "parent"}}).to_string(),
            compaction_log("2026-09-02T00:00:01Z", "gpt-5", 300),
            token_count("2026-09-02T00:00:02Z", 100),
            compaction_log("2026-09-02T00:01:00Z", "gpt-5", 50).replace("response-1", "child-response"),
            token_count("2026-09-02T00:02:00Z", 20),
        ].join("\n"),
    });
    crate::paths::set_file_modified(
        &fixture.path("2026/09/01/parent.jsonl"),
        crate::parse_ts_timestamp("2026-09-01T00:03:00Z").unwrap(),
    );
    let _guard = ccusage_test_support::EnvVarGuard::set("CODEX_HOME", fixture.root());
    for single_thread in [true, false] {
        let shared = SharedArgs {
            since: Some("20260902".into()),
            timezone: Some("UTC".into()),
            single_thread,
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Daily).unwrap();
        assert_eq!(groups["2026-09-02"].total_tokens, 110);
        let (events, detected) = crate::load_codex_events_with_detection(&shared).unwrap();
        assert!(detected);
        assert_eq!(
            events.iter().map(|event| event.total_tokens).sum::<u64>(),
            110
        );
        assert_eq!(events[0].response_id.as_deref(), Some("child-response"));
    }
}

#[test]
fn bounded_reports_exclude_counted_parent_compactions_without_a_copied_snapshot() {
    let fixture = fs_fixture!({
        "2026/09/01/parent.jsonl": [
            r#"{"type":"session_meta","payload":{"id":"parent"}}"#,
            r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"token_usage_record","payload":{"response_id":"response-1","usage":{"input_tokens":300,"output_tokens":20}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":400,"output_tokens":40},"last_token_usage":{"input_tokens":300,"output_tokens":20}}}}"#,
            r#"{"type":"compacted","payload":{"compaction_response_id":"response-1"}}"#,
        ].join("\n"),
        "2026/09/02/child.jsonl": [
            json!({"timestamp": "2026-09-02T00:00:00Z", "type": "session_meta", "payload": {"id": "child", "forked_from_id": "parent"}}).to_string(),
            token_count("2026-09-02T00:00:01Z", 100),
            compaction_log("2026-09-02T00:00:02Z", "gpt-5", 300),
            compaction_log("2026-09-02T00:01:00Z", "gpt-5", 50).replace("response-1", "child-response"),
            token_count("2026-09-02T00:02:00Z", 30),
        ].join("\n"),
    });
    crate::paths::set_file_modified(
        &fixture.path("2026/09/01/parent.jsonl"),
        crate::parse_ts_timestamp("2026-09-01T00:03:00Z").unwrap(),
    );
    let _guard = ccusage_test_support::EnvVarGuard::set("CODEX_HOME", fixture.root());
    for single_thread in [true, false] {
        let shared = SharedArgs {
            since: Some("20260902".into()),
            timezone: Some("UTC".into()),
            single_thread,
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Daily).unwrap();
        assert_eq!(groups["2026-09-02"].total_tokens, 120);
        let (events, _) = crate::load_codex_events_with_detection(&shared).unwrap();
        assert_eq!(
            events.iter().map(|event| event.total_tokens).sum::<u64>(),
            120
        );
        assert_eq!(events[0].response_id.as_deref(), Some("child-response"));
    }
}

#[test]
fn all_report_modes_keep_the_first_compaction_copy_in_serial_and_parallel() {
    let fixture = fs_fixture!({
        "a-first.jsonl": compaction_log("2026-09-01T00:00:01Z", "gpt-5", 300),
        "b-copy.jsonl": [
            r#"{"timestamp":"2026-09-02T00:00:00Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#.to_string(),
            compaction_log("2026-09-02T00:00:01Z", "gpt-5-mini", 500),
        ].join("\n"),
    });
    let pricing = PricingMap::load_embedded();
    for kind in [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
        AgentReportKind::Session,
    ] {
        let events = load_codex_events_from_directory(fixture.root(), true).unwrap();
        let expected_groups = aggregate_events(&events, kind, Some("UTC")).unwrap();
        let expected = report_from_groups(
            &expected_groups,
            kind,
            &pricing,
            CodexSpeed::Auto.into(),
            CostMode::Auto,
        );
        assert_eq!(expected["totals"]["totalTokens"], 320);
        for single_thread in [true, false] {
            let shared = SharedArgs {
                timezone: Some("UTC".into()),
                single_thread,
                ..SharedArgs::default()
            };
            let groups = load_groups_from_directory(fixture.root(), &shared, kind).unwrap();
            let actual = report_from_groups(
                &groups,
                kind,
                &pricing,
                CodexSpeed::Auto.into(),
                CostMode::Auto,
            );
            assert_eq!(actual, expected);
            assert_eq!(groups.len(), 1);
            assert_eq!(
                groups.values().next().unwrap().models["gpt-5"]
                    .recorded_fast_usage
                    .input_tokens,
                300
            );
        }
    }
}

#[test]
fn direct_reports_merge_recorded_tiers_of_duplicate_compactions() {
    let fixture = fs_fixture!({
        "first.jsonl": compaction_log("2026-09-01T00:00:01Z", "gpt-5", 300),
    });
    let first = load_codex_events_from_directory(fixture.root(), true)
        .unwrap()
        .remove(0);
    let mut duplicate = first.clone();
    duplicate.session_id = "copied-session".into();
    duplicate.service_tier = Some(crate::CodexServiceTier::Fast);
    let groups =
        aggregate_events(&[first, duplicate], AgentReportKind::Session, Some("UTC")).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups["first"].models["gpt-5"]
            .recorded_fast_usage
            .input_tokens,
        300
    );
}

#[test]
fn compaction_records_without_a_valid_pair_are_ignored() {
    let record = json!({"timestamp": "2026-09-01T00:00:01Z", "type": "token_usage_record", "payload": {
        "response_id": "response-1", "usage": {"input_tokens": 300, "output_tokens": 20}
    }});
    let marker = json!({"type": "compacted", "payload": {"compaction_response_id": "response-1"}});
    let mut missing_timestamp = record.clone();
    missing_timestamp["timestamp"] = Value::Null;
    let mut blank_id = record.clone();
    blank_id["payload"]["response_id"] = json!(" ");
    let mut zero_usage = record.clone();
    zero_usage["payload"]["usage"] = json!({"input_tokens": 0, "output_tokens": 0});
    let mut missing_usage = record.clone();
    missing_usage["payload"]["usage"] = Value::Null;
    for lines in [
        vec![record],
        vec![marker.clone(), missing_timestamp],
        vec![marker.clone(), blank_id],
        vec![marker.clone(), zero_usage],
        vec![marker.clone(), missing_usage],
        vec![marker],
    ] {
        let fixture = fs_fixture!({"session.jsonl": lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")});
        assert!(
            load_codex_events_from_directory(fixture.root(), true)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn local_compaction_usage_already_in_cumulative_counts_is_not_added_again() {
    let fixture = fs_fixture!({
        "session.jsonl": [
            r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"token_usage_record","payload":{"response_id":"response-1","usage":{"input_tokens":300,"output_tokens":30}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":400,"output_tokens":50},"last_token_usage":{"input_tokens":300,"output_tokens":30}}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:04Z","type":"compacted","payload":{"compaction_response_id":"response-1"}}"#,
            r#"{"timestamp":"2026-09-01T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":450,"output_tokens":60}}}}"#,
        ].join("\n"),
    });
    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();
    assert_eq!(
        events.iter().map(|event| event.total_tokens).sum::<u64>(),
        510
    );
    assert!(events.iter().all(|event| event.response_id.is_none()));
    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(fixture.root(), &shared, AgentReportKind::Daily).unwrap();
        assert_eq!(groups["2026-09-01"].total_tokens, 510);
    }
}

#[test]
fn thread_totals_identify_already_counted_compaction_without_an_earlier_snapshot() {
    let fixture = fs_fixture!({
        "session.jsonl": [
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"token_usage_record","payload":{"response_id":"response-1","usage":{"input_tokens":300,"output_tokens":30},"thread_token_usage":{"input_tokens":400,"output_tokens":50}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":400,"output_tokens":50}}}}"#,
            r#"{"timestamp":"2026-09-01T00:00:03Z","type":"compacted","payload":{"compaction_response_id":"response-1"}}"#,
        ].join("\n"),
    });
    let events = load_codex_events_from_directory(fixture.root(), true).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].total_tokens, 450);
}
