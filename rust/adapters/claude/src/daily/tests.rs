use std::{fmt::Write, path::Path, sync::Arc};

use super::{ClaudeEntry, read_daily_usage_file_with_diagnostics};
use crate::dedupe::push_deduped_entry;
use ccusage_adapter_common::json::JsonText;
use ccusage_core::cost::PriceTable;
use ccusage_core::{
    Diagnostics, PricingMap, TimestampMs, TokenUsageRaw,
    cli::{CostMode, SharedArgs},
};
use ccusage_test_support::{Environment, fs_fixture};
use jiff::tz::TimeZone as JiffTimeZone;

fn load_daily(shared: &SharedArgs, diagnostics: &Diagnostics) -> crate::load::DailySummaries {
    crate::load_daily_summaries_with_detection_progress(
        shared,
        None,
        None,
        ccusage_adapter_common::source::ProgressOwner::Caller,
        diagnostics,
    )
    .unwrap()
}

fn read_daily_usage_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Vec<ClaudeEntry> {
    read_daily_usage_file_with_diagnostics(
        path,
        crate::paths::fixture_projects_root(path).as_deref(),
        tz,
        pricing.map_or(PriceTable::Display, |map| PriceTable::for_map(mode, map)),
        &Diagnostics::new("claude"),
    )
}

#[test]
fn usage_prefilter_matches_the_original_marker_on_generated_lines() {
    let original = ccusage_core::fast::LinePrefilter::all(&[br#""usage":{"#]);
    for escaped_letters in 0_u8..32 {
        let mut key = String::new();
        for (index, &letter) in b"usage".iter().enumerate() {
            if escaped_letters & (1 << index) == 0 {
                key.push(char::from(letter));
            } else {
                write!(key, "\\u{letter:04x}").unwrap();
            }
        }
        for delimiter in [":{", ": {", " :{", ":\t{", ":null", ":"] {
            let fragment = format!(r#""{key}"{delimiter}"#);
            let lines = [
                format!(r#"{{"message":{{{fragment}"#) + "}}}",
                format!(r#"{{"decoy":{{{fragment}"#) + "}}}",
                format!(
                    r#"{{"content":{}}}"#,
                    serde_json::to_string(&fragment).unwrap()
                ),
                format!(r#"{{"content":"\u006e",{fragment}"#),
                format!(r#"{{"content":"\\u006e",{fragment}"#),
                format!(r#"{{"content":"\\\u006e",{fragment}"#),
            ];
            for line in lines {
                let bytes = line.as_bytes();
                for end in 0..=bytes.len() {
                    let prefix = &bytes[..end];
                    assert_eq!(
                        !matches!(
                            super::read::admit(&JsonText::new(prefix)),
                            Err(super::read::Refusal::NotUsage)
                        ),
                        original.matches(prefix),
                        "prefilter decision for {:?}",
                        &line[..end],
                    );
                }
            }
        }
    }
}

#[test]
fn usage_prefilter_preserves_typed_record_admission() {
    #[derive(Debug, PartialEq)]
    enum Admission {
        Accepted(u64),
        NoMarker,
        Malformed,
    }
    use Admission::{Accepted, Malformed, NoMarker};

    let cases = [
        (
            r#""timestamp":"2026-01-02T10:00:00Z","version":"2.0.0","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}"#,
            Accepted(10),
        ),
        (
            r#""message":{"\u0075s\u0061g\u0065":{"input_tokens":20,"output_tokens":1},"id":"msg_2"},"version":"2.0.0","timestamp":"2026-01-02T10:00:00Z""#,
            Accepted(20),
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"content":"{\"usage\":{}}"}"#,
            NoMarker,
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"decoy":{"usage":{"input_tokens":99,"output_tokens":1}}}"#,
            Malformed,
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1},"\u0075sage":{"input_tokens":99,"output_tokens":1}}"#,
            Malformed,
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"\u0075sage": {"input_tokens":30,"output_tokens":1}}"#,
            Accepted(30),
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"usage": {"input_tokens":30,"output_tokens":1}}"#,
            Accepted(30),
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","decoy":{"\u0075sage":{}},"message":{"usage": {"input_tokens":30,"output_tokens":1}}"#,
            Accepted(30),
        ),
        (
            r#""timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":40,"output_tokens":1}},"tail":"unfinished"#,
            Malformed,
        ),
    ];
    for (fields, expected) in cases {
        let line = format!("{{{fields}}}");
        let actual = match super::read::admit(&JsonText::new(line.as_bytes())) {
            Ok(admitted) => Accepted(admitted.record.message.usage.usage.input_tokens),
            Err(super::read::Refusal::NotUsage) => NoMarker,
            Err(super::read::Refusal::Rejected(crate::Rejection::Malformed)) => Malformed,
            Err(_) => panic!("unexpected refusal for {line}"),
        };
        assert_eq!(actual, expected, "{line}");
    }
}

#[test]
fn counts_usage_written_with_json_whitespace() {
    for (key, colon) in [
        ("usage", ":"),
        ("usage", ": "),
        ("usage", " : "),
        ("usage", ":\t"),
        (r"\u0075sage", ": "),
    ] {
        let fields = format!(
            r#""message":{{"id":"m1","model":"claude-sonnet-4-20250514","{key}"{colon}{{"input_tokens":100,"output_tokens":2}}}}"#
        );
        let line = format!(
            r#"{{"type":"assistant","timestamp":"2026-09-11T12:00:00.000Z","sessionId":"session-a",{fields}}}"#
        );
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": line,
        });
        let path = fixture.path("projects/project-a/session-a.jsonl");
        let loaded = read_daily_usage_file(&path, &JiffTimeZone::UTC, CostMode::Display, None);
        assert_eq!(loaded.len(), 1, "{key}{colon}");
        assert_eq!(loaded[0].usage.input_tokens, 100, "{key}{colon}");
        assert_eq!(loaded[0].usage.output_tokens, 2, "{key}{colon}");
        assert!(super::has_usage(&path), "{key}{colon}");
    }
}

#[test]
fn counts_wrapped_usage_written_with_json_whitespace() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"type":"progress","sessionId":"session-a","data":{"message":{"timestamp":"2026-09-11T12:00:00.000Z","message":{"id":"m1","model":"claude-sonnet-4-20250514","usage": {"input_tokens":100,"output_tokens":2}}}}}"#,
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let loaded = read_daily_usage_file(&path, &JiffTimeZone::UTC, CostMode::Display, None);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].usage.input_tokens, 100);
    assert_eq!(loaded[0].usage.output_tokens, 2);
    assert!(super::has_usage(&path));
}

#[test]
fn usage_prefilter_reads_a_marker_across_the_buffer_boundary() {
    let prefix = r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"content":""#;
    let separator = r#"","#;
    let padding_len = super::read::READ_POLICY.capacity - prefix.len() - separator.len() - 4;
    let line = format!(
        "{prefix}{}{separator}{}",
        "x".repeat(padding_len),
        r#""usage":{"input_tokens":40,"output_tokens":1}}}"#,
    );
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": line,
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let loaded = read_daily_usage_file(&path, &JiffTimeZone::UTC, CostMode::Display, None);
    assert!(super::has_usage(&path));
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].usage.input_tokens, 40);
}

#[test]
fn detection_and_reports_keep_parent_usage_with_a_null_advisor() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":1,"output_tokens":2,"cache_read_input_tokens":null}]}}}"#,
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let diagnostics = Diagnostics::new("claude");
    let loaded = super::read_daily_usage_file_with_diagnostics(
        &path,
        crate::paths::fixture_projects_root(&path).as_deref(),
        &JiffTimeZone::UTC,
        PriceTable::Display,
        &diagnostics,
    );
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].usage.input_tokens, 10);
    assert!(super::has_usage(&path));
    assert!(diagnostics.warnings(&SharedArgs::default()).is_empty());
}

#[test]
fn guards_only_the_advisor_iterations_it_reads() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1,"iterations":[{"type":"message","input_tokens":1,"output_tokens":1,"cache_read_input_tokens":null}]}}}"#,
            r#"{"timestamp":"2026-01-02T10:00:01Z","message":{"usage":{"input_tokens":20,"output_tokens":1,"iterations":[{"type":"message","input_tokens":1,"output_tokens":1,"cache_read_input_tokens":null},{"type":"advisor_message","model":"advisor-model","input_tokens":3,"output_tokens":1}]}}}"#,
        ]
        .join("\n"),
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let diagnostics = Diagnostics::new("claude");
    let loaded = super::read_daily_usage_file_with_diagnostics(
        &path,
        crate::paths::fixture_projects_root(&path).as_deref(),
        &JiffTimeZone::UTC,
        PriceTable::Display,
        &diagnostics,
    );
    let input_tokens = loaded
        .iter()
        .map(|entry| entry.usage.input_tokens)
        .collect::<Vec<_>>();
    assert_eq!(input_tokens, [10, 20, 3]);
    assert!(diagnostics.warnings(&SharedArgs::default()).is_empty());
}

#[test]
fn malformed_advisors_preserve_parent_and_valid_sibling_usage() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"bad","input_tokens":"x"},{"type":"advisor_message","model":"good","input_tokens":3,"output_tokens":2}]}}}"#,
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let diagnostics = Diagnostics::new("claude");
    let loaded = super::read_daily_usage_file_with_diagnostics(
        &path,
        crate::paths::fixture_projects_root(&path).as_deref(),
        &JiffTimeZone::UTC,
        PriceTable::Display,
        &diagnostics,
    );
    assert_eq!(
        loaded
            .iter()
            .map(|entry| entry.usage.input_tokens)
            .collect::<Vec<_>>(),
        [10, 3]
    );
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
}

#[test]
fn unrepresentable_advisor_usage_is_diagnosed_and_dropped() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":18446744073709551615,"output_tokens":50},{"type":"advisor_message","model":"advisor-model","input_tokens":3,"output_tokens":2}]}}}"#,
    });
    let diagnostics = Diagnostics::new("claude");
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let loaded = super::read_daily_usage_file_with_diagnostics(
        &path,
        crate::paths::fixture_projects_root(&path).as_deref(),
        &JiffTimeZone::UTC,
        PriceTable::Display,
        &diagnostics,
    );
    assert_eq!(
        loaded
            .iter()
            .map(|entry| entry.usage.input_tokens)
            .collect::<Vec<_>>(),
        [10, 3]
    );
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
}

#[test]
fn irrelevant_outer_advisor_fields_do_not_reject_progress_usage() {
    for message in [
        r#""advisor_message""#,
        r#"{"usage":{"iterations":null},"content":"advisor_message"}"#,
    ] {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": format!(r#"{{"message":{message},"data":{{"message":{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":10,"output_tokens":1}}}}}}}}}}"#),
        });
        let path = fixture.path("projects/project-a/session-a.jsonl");
        let diagnostics = Diagnostics::new("claude");
        let loaded = super::read_daily_usage_file_with_diagnostics(
            &path,
            crate::paths::fixture_projects_root(&path).as_deref(),
            &JiffTimeZone::UTC,
            PriceTable::Display,
            &diagnostics,
        );
        assert_eq!(loaded.len(), 1, "{message}");
        assert_eq!(loaded[0].usage.input_tokens, 10);
        assert!(super::has_usage(&path));
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 0);
    }
}

#[test]
fn records_a_malformed_advisor_iteration_as_invalid_data() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":"SECRET_CANARY","output_tokens":1}]}}}"#,
    });
    let path = fixture.path("projects/project-a/session-a.jsonl");
    let diagnostics = Diagnostics::new("claude");
    let loaded = super::read_daily_usage_file_with_diagnostics(
        &path,
        crate::paths::fixture_projects_root(&path).as_deref(),
        &JiffTimeZone::UTC,
        PriceTable::Display,
        &diagnostics,
    );
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].usage.input_tokens, 10);
    assert_eq!(loaded[0].usage.output_tokens, 1);
    assert!(super::has_usage(&path));
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
}

#[test]
fn null_contract_rejections_claude_code_writes_stay_quiet() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            crate::tests::SYNTHETIC_NULL_SPEED_LINE,
        ]
        .join("\n"),
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread: true,
            debug: true,
            ..SharedArgs::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("claude");

        let daily = load_daily(&shared, &diagnostics);

        assert_eq!(daily.summaries[0].input_tokens, 10);
        assert!(diagnostics.warnings(&shared).is_empty());
    })
}

#[test]
fn collects_sessions_of_counted_main_thread_usage_only() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-empty.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-empty","type":"user","message":{"role":"user","content":"hi"}}"#,
        "projects/project-a/session-sub/subagents/agent-1.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-sub","isSidechain":true,"requestId":"req-sub","message":{"id":"msg-sub","model":"claude-sonnet-4-20250514","usage":{"input_tokens":30,"output_tokens":1}}}"#,
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread: true,
            ..SharedArgs::default()
        };

        let daily = load_daily(&shared, &ccusage_core::Diagnostics::new("claude"));

        assert_eq!(
            daily.sessions.into_iter().collect::<Vec<_>>(),
            [Arc::from("session-a")]
        );
        assert_eq!(daily.summaries[0].input_tokens, 130);
    })
}

#[test]
fn detecting_usage_in_files_outside_the_window_records_no_skips() {
    let fixture = fs_fixture!({
        "projects/project-a/old.jsonl": [
            r#"{"timestamp":"2001-01-01T00:00:00.000Z","message":{"usage":{"input_tokens":"bad"}}}"#,
            r#"{"timestamp":"2001-01-01T00:00:00.000Z","sessionId":"old","requestId":"req-old","message":{"id":"msg-old","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        ]
        .join("\n"),
    });
    std::fs::File::options()
        .write(true)
        .open(fixture.path("projects/project-a/old.jsonl"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200))
        .unwrap();
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            since: Some("20260101".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread: true,
            ..SharedArgs::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("claude");

        let daily = load_daily(&shared, &diagnostics);

        assert!(daily.summaries.is_empty());
        assert!(daily.detected);
        assert!(diagnostics.warnings(&shared).is_empty());
    })
}

#[test]
fn keeps_parent_daily_usage_when_sidechain_replays_message_with_new_request_id() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 20,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-parent",
            request_id: "req-sidechain-replay",
            is_sidechain: true,
            cache_read_tokens: 50_000,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-sidechain-answer",
            request_id: "req-sidechain-answer",
            is_sidechain: true,
            cache_read_tokens: 700,
            output_tokens: 30,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].key.message_id.as_deref(), Some("msg-parent"));
    assert_eq!(deduped[0].key.request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
    assert_eq!(
        deduped[1].key.message_id.as_deref(),
        Some("msg-sidechain-answer")
    );
    assert_eq!(deduped[1].usage.cache_read_input_tokens, 700);
}

#[test]
fn dedupes_timestamp_different_daily_sidechain_replay_with_parent() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_001_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].key.request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
}

#[test]
fn refreshes_daily_dedupe_indexes_when_parent_replaces_sidechain_replay() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-parent",
            request_id: "req-sidechain-replay",
            is_sidechain: true,
            cache_read_tokens: 50_000,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 20,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 5,
            output_tokens: 5,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].key.request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
}

#[test]
fn propagates_sidechain_metadata_from_agent_progress_lines() {
    let line = JsonText::new(
        br#"{"sessionId":"session-a","data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","requestId":"req-sidechain","isSidechain":true,"message":{"usage":{"input_tokens":0,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":20},"model":"claude-sonnet-4-20250514","id":"msg-sidechain"}}}}"#,
    );
    let (data, _) = super::parse_usage_line(&line).unwrap();

    assert_eq!(data.session_id.as_deref(), Some("session-a"));
    assert_eq!(data.is_sidechain, Some(true));
}

#[test]
fn reads_the_agent_progress_envelope_when_the_top_level_timestamp_is_null() {
    let line = JsonText::new(
        br#"{"timestamp":null,"sessionId":"session-a","data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","message":{"usage":{"input_tokens":1,"output_tokens":2}}}}}"#,
    );
    let (data, _) = super::parse_usage_line(&line).unwrap();

    assert_eq!(data.timestamp, "2026-03-29T07:00:00.000Z");
    assert_eq!(data.session_id.as_deref(), Some("session-a"));
}

#[test]
fn decodes_escaped_line_scoped_fields_like_plain_ones() {
    for (direct, progress, escaped) in [
        (
            r#"{"timestamp":"2026-03-29T07:00:00.000Z","version":"1.0.0","sessionId":"session-a","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            r#"{"sessionId":"session-a","data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","message":{"usage":{"input_tokens":1,"output_tokens":2}}}}}"#,
            false,
        ),
        (
            r#"{"timestamp":"2026-03-29T07:00:00.000\u005a","version":"1.0\u002e0","sessionId":"session\u002da","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            r#"{"sessionId":"session\u002da","data":{"message":{"timestamp":"2026-03-29T07:00:00.000\u005a","message":{"usage":{"input_tokens":1,"output_tokens":2}}}}}"#,
            true,
        ),
    ] {
        let direct = JsonText::new(direct.as_bytes());
        let (record, _) = super::parse_usage_line(&direct).unwrap();
        assert_eq!(
            matches!(record.timestamp, std::borrow::Cow::Owned(_)),
            escaped
        );
        assert_eq!(record.timestamp, "2026-03-29T07:00:00.000Z");
        assert_eq!(record.version.as_deref(), Some("1.0.0"));
        assert_eq!(record.session_id.as_deref(), Some("session-a"));

        let progress = JsonText::new(progress.as_bytes());
        let (record, _) = super::parse_usage_line(&progress).unwrap();
        assert_eq!(record.timestamp, "2026-03-29T07:00:00.000Z");
        assert_eq!(record.session_id.as_deref(), Some("session-a"));
    }
}

#[test]
fn prefers_the_direct_entry_over_an_agent_progress_wrapper() {
    let line = JsonText::new(
        br#"{"timestamp":"2026-03-29T08:00:00.000Z","requestId":"req-direct","message":{"usage":{"input_tokens":1,"output_tokens":2},"model":"direct-model","id":"msg-direct"},"data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","requestId":"req-wrapped","message":{"usage":{"input_tokens":3,"output_tokens":4},"model":"wrapped-model","id":"msg-wrapped"}}}}"#,
    );
    let (data, _) = super::parse_usage_line(&line).unwrap();

    assert_eq!(data.request_id.as_deref(), Some("req-direct"));
    assert_eq!(data.message.model.as_deref(), Some("direct-model"));
}

#[test]
fn keeps_gateway_daily_usage_from_distinct_sessions_with_reused_message_id() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":300,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    for path in [
        fixture.path("projects/project-a/session-a/chat.jsonl"),
        fixture.path("projects/project-a/session-b/chat.jsonl"),
    ] {
        let loaded =
            read_daily_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }
    }

    assert_eq!(deduped.len(), 2);
    assert_eq!(
        deduped
            .iter()
            .map(|entry| entry.usage.input_tokens)
            .sum::<u64>(),
        400
    );
}

#[test]
fn keeps_timestamp_different_requestless_daily_entries_without_sidechain() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let mut first = daily_loaded_entry_at(
        DailyEntryFixture {
            message_id: "ocgo",
            request_id: "unused",
            is_sidechain: false,
            cache_read_tokens: 0,
            output_tokens: 10,
        },
        "session-a",
        TimestampMs::from_millis(1_774_000_000_000),
    );
    first.key.request = crate::dedupe::Request::Missing(None);
    push_deduped_entry(first, &mut deduped_indexes, &mut deduped);

    let mut second = daily_loaded_entry_at(
        DailyEntryFixture {
            message_id: "ocgo",
            request_id: "unused",
            is_sidechain: false,
            cache_read_tokens: 0,
            output_tokens: 20,
        },
        "session-a",
        TimestampMs::from_millis(1_774_000_001_000),
    );
    second.key.request = crate::dedupe::Request::Missing(None);
    push_deduped_entry(second, &mut deduped_indexes, &mut deduped);

    assert_eq!(deduped.len(), 2);
    assert_eq!(
        deduped
            .iter()
            .map(|entry| entry.usage.output_tokens)
            .sum::<u64>(),
        30
    );
}

#[test]
fn keeps_the_cache_ttl_split_from_an_otherwise_equal_daily_duplicate() {
    for lines in [
        [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
        ],
        [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100}}}"#,
        ],
    ] {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": lines.join("\n"),
        });
        Environment::with_vars([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )])
        .run(|| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                single_thread: true,
                ..SharedArgs::default()
            };

            let daily = load_daily(&shared, &ccusage_core::Diagnostics::new("claude"));

            let breakdown = &daily.summaries[0].model_breakdowns[0];
            assert_eq!(breakdown.cache_creation_tokens, 100);
            assert_eq!(breakdown.cache_creation_1h_tokens, Some(60));
        })
    }
}

#[test]
fn requestless_daily_content_blocks_collapse_only_along_their_transcript_chain() {
    let mut failures = Vec::new();
    for (case, lines, responses) in crate::tests::requestless_response_cases() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": lines.join("\n"),
        });
        let path = fixture.path("projects/project-a/session-a/chat.jsonl");
        let loaded =
            read_daily_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        let (deduped, _) = crate::dedupe::dedupe_files(vec![(path, loaded)]);

        if deduped.len() != responses {
            failures.push(format!(
                "{case}: {} entries, expected {responses}",
                deduped.len()
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn keeps_daily_sidechain_replay_from_distinct_session() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            },
            "session-b",
            TimestampMs::from_millis(1_774_000_001_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 2);
    assert_eq!(
        deduped
            .iter()
            .map(|entry| entry.usage.cache_read_input_tokens)
            .sum::<u64>(),
        50_020
    );
}

#[test]
fn dedupes_copied_daily_transcripts_with_the_same_serialized_session_id() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    for path in [
        fixture.path("projects/project-a/session-a/chat.jsonl"),
        fixture.path("projects/project-a/session-b/chat.jsonl"),
    ] {
        let loaded =
            read_daily_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }
    }

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.input_tokens, 200);
}

#[test]
fn reads_daily_usage_and_advisor_iterations_under_escaped_keys() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"main-model","us\u0061ge":{"input_tokens":100,"output_tokens":1,"iterations":[{"type":"advisor\u005fmessage","model":"advisor-model","input_tokens":10,"output_tokens":2}]}}}"#,
    });

    let loaded = read_daily_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        None,
    );

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].usage.input_tokens, 100);
    assert_eq!(loaded[1].usage.input_tokens, 10);
}

#[test]
fn daily_advisor_gate_preserves_the_previous_extraction() {
    let iterations = [
        r#"{"type":"advisor_message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
        r#"{"type":"advisor\u005fmessage","model":"advisor","input_tokens":10,"output_tokens":2}"#,
        r#"{"type":"advisor\\u005fmessage","model":"advisor","input_tokens":10,"output_tokens":2}"#,
        r#"{"type":"message","cache_creation":{"ephemeral_5m_input_tokens":7}}"#,
        r#"{"type":"advisor_message","type":"message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
        r#"{"type":"message","type":"advisor_message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
        r#"{"type":null}"#,
        r#"{"type":["advisor_message"]}"#,
        r#"{"kind":"advisor_message"}"#,
        r#"null"#,
        r#"[]"#,
    ];
    let mut state = 0x3c6e_f372_fe94_f82b_u64;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).unwrap_or_default()
    };
    let mut skipped = 0;
    let mut retained = 0;
    for _ in 0..5_000 {
        let entries = (0..next(6))
            .map(|_| iterations[next(iterations.len())])
            .collect::<Vec<_>>()
            .join(",");
        let usage = format!(r#"{{"input_tokens":1,"output_tokens":2,"iterations":[{entries}]}}"#);
        let message = format!(r#"{{"model":"parent","usage":{usage}}}"#);
        for line in [
            format!(r#"{{"timestamp":"2026-05-22T02:34:40Z","message":{message}}}"#),
            format!(
                r#"{{"data":{{"message":{{"timestamp":"2026-05-22T02:34:40Z","message":{message}}}}}}}"#
            ),
        ] {
            let (_, lookup) = super::parse_usage_line(&JsonText::new(line.as_bytes())).unwrap();
            let reference = format!(r#"{{"message":{message}}}"#);
            let previous = crate::advisor::advisor_usages_from_line(reference.as_bytes()).unwrap();
            let actual = lookup.usages(line.as_bytes()).unwrap();
            match lookup {
                crate::advisor::AdvisorLookup::AskLine => {
                    retained += 1;
                    assert_eq!(actual.usages.len(), previous.usages.len(), "{line}");
                    assert_eq!(actual.malformed, previous.malformed, "{line}");
                }
                crate::advisor::AdvisorLookup::AskProgress => {
                    retained += 1;
                    assert_eq!(actual.usages.len(), previous.usages.len(), "{line}");
                    assert_eq!(actual.malformed, previous.malformed, "{line}");
                }
                crate::advisor::AdvisorLookup::RuledOut => {
                    skipped += 1;
                    assert!(previous.usages.is_empty(), "{line}");
                    assert!(actual.usages.is_empty(), "{line}");
                }
            }
        }
    }
    assert!(skipped > 0);
    assert!(retained > 0);
}

#[test]
fn dedupes_copied_daily_transcripts_with_the_same_request_id_across_sessions() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:41.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    for path in [
        fixture.path("projects/project-a/session-a/chat.jsonl"),
        fixture.path("projects/project-a/session-b/chat.jsonl"),
    ] {
        let loaded =
            read_daily_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }
    }

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.input_tokens, 200);
}

#[test]
fn returns_the_discarded_daily_copy_from_another_session() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        "projects/project-a/session-c/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-c","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":50,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let discarded = ["a", "b", "c"].map(|session| {
        let path = fixture.path(format!("projects/project-a/session-{session}/chat.jsonl"));
        let entry = read_daily_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None)
            .into_iter()
            .next()
            .unwrap();
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
            .map(|(index, discarded)| (index, discarded.key.session_id.to_string()))
    });

    assert_eq!(
        discarded,
        [
            None,
            Some((0, "session-a".to_string())),
            Some((0, "session-c".to_string())),
        ]
    );
    assert_eq!(deduped[0].key.session_id.as_ref(), "session-b");
}

#[test]
fn dedupes_daily_sidechain_replay_after_an_equal_copy_from_another_session() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            },
            "session-b",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].key.request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
}

#[test]
fn dedupes_daily_sidechain_replay_after_cross_session_survivor_replacement() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            },
            "session-b",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 30,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );
    push_deduped_entry(
        daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            },
            "session-b",
            TimestampMs::from_millis(1_774_000_000_000),
        ),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].key.request.id(), Some("req-parent"));
    assert_eq!(deduped[0].key.session_id.as_ref(), "session-a");
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 30);
}

#[test]
fn daily_summaries_keep_label_order_for_extended_years() {
    let entries = [(10000, 1, 1), (2026, 1, 1), (-2, 1, 1), (-1, 12, 31)]
        .into_iter()
        .map(|(y, m, d)| {
            let mut entry = daily_loaded_entry(DailyEntryFixture {
                message_id: "msg",
                request_id: "req",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 1,
            });
            entry.date = ccusage_core::cli::IsoDate::from_ymd(y, m, d).unwrap();
            entry
        })
        .collect();
    let daily = super::DailyEntries {
        deduped: super::DedupedDailyEntries {
            entries,
            copies: Vec::new(),
        },
        detected: true,
    };
    for group_by_project in [false, true] {
        let rows = daily.summaries(group_by_project, None);
        let dates = rows
            .iter()
            .map(|row| row.date.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            dates,
            ["-001-12-31", "-002-01-01", "10000-01-01", "2026-01-01"]
        );
    }
}

struct DailyEntryFixture {
    message_id: &'static str,
    request_id: &'static str,
    is_sidechain: bool,
    cache_read_tokens: u64,
    output_tokens: u64,
}

fn daily_loaded_entry(fixture: DailyEntryFixture) -> ClaudeEntry {
    ClaudeEntry {
        key: super::DedupKey {
            timestamp: TimestampMs::from_millis(1_774_000_000_000),
            session_id: Arc::from("session-a"),
            message_id: Some(fixture.message_id.into()),
            request: crate::dedupe::Request::Id(fixture.request_id.into()),
        },
        is_sidechain: fixture.is_sidechain,
        date: ccusage_core::cli::IsoDate::parse("2026-03-29").unwrap(),
        project: Arc::from("project-a"),
        usage: TokenUsageRaw {
            input_tokens: 0,
            output_tokens: fixture.output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: fixture.cache_read_tokens,
            speed: None,
            cache_creation: None,
        },
        cost: 0.0,
        model: Some(Arc::from("claude-sonnet-4-20250514")),
        missing_pricing: false,
        estimated_pricing: false,
        recorded_cost: super::RecordedCost::Absent,
    }
}

fn daily_loaded_entry_at(
    fixture: DailyEntryFixture,
    session_id: &str,
    timestamp: TimestampMs,
) -> ClaudeEntry {
    let mut entry = daily_loaded_entry(fixture);
    entry.key.session_id = Arc::from(session_id);
    entry.key.timestamp = timestamp;
    entry
}
