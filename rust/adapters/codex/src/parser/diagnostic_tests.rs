use super::*;
use ccusage_test_support::Fixture;
use std::path::PathBuf;

#[test]
fn a_rollout_removed_after_discovery_is_noted_not_skipped() {
    let fixture = Fixture::new();
    let path = fixture.path("session.jsonl");
    let diagnostics = Diagnostics::new("codex");

    visit_codex_session_file_with_diagnostics(fixture.root(), &path, None, |_| {}, &diagnostics);

    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
    let debug = crate::cli::SharedArgs {
        debug: true,
        ..Default::default()
    };
    assert_eq!(
        diagnostics.warnings(&debug),
        [format!(
            "codex: {}: removed after discovery, read as absent",
            path.display()
        )]
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_sibling_encoding_of_a_removed_rollout_is_a_skip() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let fixture = Fixture::new();
    let path = fixture.path("session.jsonl");
    let compressed = fixture.write_file("session.jsonl.zst", "rollout");
    fs::set_permissions(&compressed, fs::Permissions::from_mode(0o000)).unwrap();
    let diagnostics = Diagnostics::new("codex");

    visit_codex_session_file_with_diagnostics(fixture.root(), &path, None, |_| {}, &diagnostics);
    fs::set_permissions(&compressed, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    let warnings = diagnostics
        .warnings(&crate::cli::SharedArgs {
            debug: true,
            ..Default::default()
        })
        .join("\n");
    assert!(
        warnings.contains(&format!("{}: rollout open failed", compressed.display())),
        "{warnings}"
    );
}

const ROLLOUT_CUT_SHORT: &str = concat!(
    r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
    "\n",
    r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_tok"#,
);

fn visit_rollout_cut_short(on_event: impl Fn(&Path)) -> (usize, Diagnostics) {
    let fixture = Fixture::new();
    let path = fixture.write_file("session.jsonl", ROLLOUT_CUT_SHORT);
    let diagnostics = Diagnostics::new("codex");
    let mut events = 0;
    visit_codex_session_file_with_diagnostics(
        fixture.root(),
        &path,
        None,
        |_| {
            events += 1;
            on_event(&path);
        },
        &diagnostics,
    );
    (events, diagnostics)
}

#[test]
fn a_rollout_line_cut_short_while_the_rollout_grows_is_not_a_skip() {
    let (events, diagnostics) = visit_rollout_cut_short(|path| {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"al")
            .unwrap();
    });

    assert_eq!(events, 1);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
}

#[test]
fn a_rollout_line_left_cut_short_is_a_skip() {
    let (events, diagnostics) = visit_rollout_cut_short(|_| {});

    assert_eq!(events, 1);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
}

const USAGE_LINE: &str = r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#;
const MODIFIED: &str = "2026-09-01T00:00:00Z";

fn torn_compressed_rollout(fixture: &Fixture, complete_frame: bool) -> PathBuf {
    let mut compressed = if complete_frame {
        zstd::stream::encode_all(format!("{USAGE_LINE}\n").as_bytes(), 3).unwrap()
    } else {
        Vec::new()
    };
    let torn = zstd::stream::encode_all(format!("{USAGE_LINE}\n").as_bytes(), 3).unwrap();
    compressed.extend(&torn[..torn.len() / 2]);
    let path = fixture.write_file("session.jsonl.zst", "");
    std::fs::write(&path, compressed).unwrap();
    ccusage_test_support::set_modified(&path, ccusage_test_support::instant(MODIFIED));
    path
}

fn visit_torn_compressed_rollout(
    complete_frame: bool,
    on_event: impl Fn(&Path),
) -> (PathBuf, u64, Vec<String>, Diagnostics) {
    let fixture = Fixture::new();
    let path = torn_compressed_rollout(&fixture, complete_frame);
    let diagnostics = Diagnostics::new("codex");
    let mut tokens = 0;
    visit_codex_session_file_with_diagnostics(
        fixture.root(),
        &path,
        None,
        |event| {
            tokens += event.total_tokens;
            on_event(&path);
        },
        &diagnostics,
    );
    let warnings = diagnostics.warnings(&crate::cli::SharedArgs {
        debug: true,
        ..Default::default()
    });
    (path, tokens, warnings, diagnostics)
}

#[test]
fn a_compressed_rollout_cut_inside_a_frame_while_it_changes_is_noted_not_skipped() {
    let (path, tokens, warnings, diagnostics) = visit_torn_compressed_rollout(true, |path| {
        ccusage_test_support::set_modified(
            path,
            ccusage_test_support::instant(MODIFIED).plus(std::time::Duration::from_secs(1)),
        );
    });

    assert_eq!(tokens, 120);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 0);
    assert_eq!(
        warnings,
        [format!(
            "codex: {}: incomplete, read as still being written",
            path.display()
        )]
    );
}

#[test]
fn a_compressed_rollout_left_cut_inside_a_frame_keeps_earlier_usage_as_invalid_data() {
    let (_, tokens, _, diagnostics) = visit_torn_compressed_rollout(true, |_| {});

    assert_eq!(tokens, 120);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 0);
}

#[test]
fn a_compressed_rollout_left_cut_inside_its_first_frame_is_invalid_data() {
    let (_, tokens, _, diagnostics) = visit_torn_compressed_rollout(false, |_| {});

    assert_eq!(tokens, 0);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 0);
}

#[test]
fn malformed_usage_lines_skip_the_same_whether_or_not_a_marker_is_escaped() {
    for malformed in [
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#,
        r#"{"type":"event_msg","payload":{"type":"token_count","note":"compact\u0065d","info":{"secret":"#,
    ] {
        let fixture = Fixture::new();
        let path = fixture.write_file(
            "session.jsonl",
            [
                malformed,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            ]
            .join("\n"),
        );
        let diagnostics = Diagnostics::new("codex");
        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |_| {},
            &diagnostics,
        );

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1, "{malformed}");
    }
}

#[test]
fn nonusage_and_suppressed_session_records_stay_quiet() {
    let fixture = Fixture::new();
    let path = fixture.write_file("session.jsonl", [
        r#"{"type":"turn_context","timestamp":{},"payload":{"model":"SECRET_CANARY"}}"#,
        r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0}}}}"#,
    ].join("\n"));
    let diagnostics = Diagnostics::new("codex");
    let mut events = Vec::new();
    visit_codex_session_file_with_diagnostics(
        fixture.root(),
        &path,
        None,
        |event| {
            events.push(event);
        },
        &diagnostics,
    );
    assert!(events.is_empty());
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    ccusage_test_support::Environment::with_var("LOG_LEVEL", "1").run(|| {
        assert!(
            diagnostics
                .warnings(&crate::cli::SharedArgs::default())
                .is_empty()
        );
    })
}

fn session_events(lines: &[&str]) -> (Vec<CodexTokenUsageEvent>, Diagnostics) {
    let fixture = Fixture::new();
    let path = fixture.write_file("session.jsonl", lines.join("\n"));
    let diagnostics = Diagnostics::new("codex");
    let mut events = Vec::new();
    visit_codex_session_file_with_diagnostics(
        fixture.root(),
        &path,
        None,
        |event| events.push(event),
        &diagnostics,
    );
    (events, diagnostics)
}

#[test]
fn malformed_descriptive_session_fields_keep_the_usage() {
    let (events, diagnostics) = session_events(&[
        r#"{"type":"turn_context","timestamp":{},"payload":{"model":"gpt-5.4","metadata":[]}}"#,
        r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","model":{},"model_name":7,"metadata":"x","info":{"model":[],"last_token_usage":{"input_tokens":10,"output_tokens":2},"total_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
        r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:01Z","payload":{"type":"thread_settings_applied","thread_settings":"fast"}}"#,
    ]);
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].input_tokens, events[0].output_tokens), (10, 2));
    assert_eq!(events[0].model, "gpt-5.4");
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
}

#[test]
fn session_usage_objects_keep_the_last_value_of_a_repeated_key() {
    let (events, diagnostics) = session_events(&[
        r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":999,"input_tokens":10,"output_tokens":2}}}}"#,
        r#"{"type":"token_usage_record","timestamp":"2026-01-02T00:00:01Z","payload":{"response_id":"r","usage":{"input_tokens":999,"input_tokens":20,"output_tokens":3}}}"#,
        r#"{"type":"compacted","timestamp":"2026-01-02T00:00:02Z","payload":{"compaction_response_id":"r"}}"#,
    ]);
    let usage: Vec<_> = events
        .iter()
        .map(|event| (event.input_tokens, event.output_tokens))
        .collect();
    assert_eq!(usage, [(10, 2), (20, 3)]);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
}

#[test]
fn a_malformed_usage_field_is_named_by_its_static_key_path() {
    let (events, diagnostics) = session_events(&[
        r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":{"x":"SECRET_CANARY"}}}}}"#,
        r#"{"type":"event_msg","timestamp":{"x":"SECRET_CANARY"},"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10}}}}"#,
        r#"{"type":"token_usage_record","timestamp":"2026-01-02T00:00:01Z","payload":{"response_id":"r","usage":{"input_tokens":10},"thread_token_usage":{"input_tokens":[]}}}"#,
    ]);
    assert!(events.is_empty());
    let warnings = diagnostics
        .warnings(&crate::cli::SharedArgs {
            debug: true,
            ..Default::default()
        })
        .join("\n");
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 3, "{warnings}");
    for field in [
        "payload.info.last_token_usage",
        "timestamp",
        "payload.thread_token_usage",
    ] {
        assert!(
            warnings.contains(&format!("field `{field}` did not decode")),
            "{warnings}"
        );
    }
    assert!(!warnings.contains("SECRET_CANARY"), "{warnings}");
}

#[test]
fn a_damaged_field_never_hides_the_cumulative_total_from_the_next_delta() {
    use serde_json::json;

    let pointers = [
        "/timestamp",
        "/created_at",
        "/createdAt",
        "/model",
        "/model_name",
        "/metadata",
        "/usage",
        "/data",
        "/result",
        "/response",
        "/payload/model",
        "/payload/model_name",
        "/payload/metadata",
        "/payload/usage",
        "/payload/thread_token_usage",
        "/payload/thread_settings",
        "/payload/response_id",
        "/payload/compaction_response_id",
        "/payload/info/model",
        "/payload/info/model_name",
        "/payload/info/metadata",
        "/payload/info/last_token_usage",
    ];
    let damaged_values = [
        json!(null),
        json!(true),
        json!(-1),
        json!(1.5),
        json!("SECRET_CANARY"),
        json!([]),
        json!([{"input_tokens": 1}]),
        json!({}),
        json!({"input_tokens": {"SECRET_CANARY": true}}),
    ];
    let total = |timestamp: &str, input_tokens: u64| {
        json!({"timestamp": timestamp, "type": "event_msg", "payload": {
            "type": "token_count",
            "info": {"total_token_usage": {"input_tokens": input_tokens, "output_tokens": 1}},
        }})
    };
    for pointer in pointers {
        for damaged_value in &damaged_values {
            let mut damaged = total("2026-01-02T00:00:01Z", 110);
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            damaged
                .pointer_mut(parent)
                .and_then(serde_json::Value::as_object_mut)
                .unwrap()
                .insert(key.to_string(), damaged_value.clone());
            let lines = [
                total("2026-01-02T00:00:00Z", 10),
                damaged,
                total("2026-01-02T00:00:02Z", 115),
            ]
            .map(|line| line.to_string());
            let (events, _) = session_events(&lines.each_ref().map(String::as_str));
            assert_eq!(
                events.last().map(|event| event.input_tokens),
                Some(5),
                "{pointer} = {damaged_value}"
            );
        }
    }
}

#[test]
fn numeric_timestamps_beyond_the_supported_range_do_not_date_usage() {
    let (events, diagnostics) = session_events(&[
        r#"{"type":"event_msg","timestamp":1767312000000000,"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
    ]);
    assert!(events.is_empty());
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);

    let (events, _) = session_events(&[
        r#"{"type":"turn.completed","timestamp":1767312000000000,"created_at":"2026-01-02T00:00:00Z","usage":{"input_tokens":10,"output_tokens":2}}"#,
    ]);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].timestamp,
        parse_ts_timestamp("2026-01-02T00:00:00Z").unwrap()
    );
}

#[test]
fn attributes_a_record_only_to_the_snapshot_that_follows_it() {
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    let usage = |input_tokens: u64| json!({"input_tokens": input_tokens, "cached_input_tokens": 0, "output_tokens": 1, "total_tokens": input_tokens + 1});
    let token_count = |timestamp: &str, last: u64, total: u64| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "model": "gpt-5.2",
                "last_token_usage": usage(last),
                "total_token_usage": usage(total),
            }},
        })
        .to_string()
    };
    let fixture = fs_fixture!({
        "session.jsonl": [
            json!({
                "timestamp": "2026-11-01T09:00:00.000Z",
                "type": "token_usage_record",
                "payload": {"response_id": "resp_unmatched", "usage": usage(10)},
            })
            .to_string(),
            token_count("2026-11-01T09:00:01.000Z", 20, 20),
            token_count("2026-11-01T09:00:02.000Z", 10, 30),
        ]
        .join("\n"),
    });

    let events = crate::test_support::rollout_events(fixture.root());

    assert_eq!(
        events
            .iter()
            .map(|event| (event.input_tokens, event.record_response_id.as_deref()))
            .collect::<Vec<_>>(),
        [(20, None), (10, None)]
    );
}
