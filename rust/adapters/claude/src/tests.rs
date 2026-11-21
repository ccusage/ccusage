use std::assert_matches;
use std::{path::Path, sync::Arc};

use super::{
    extract_session_parts,
    load::{load_deduped_entries, usage_limit_reset_time_from_line_bytes},
    paths::is_project_path_segment,
};
use crate::ClaudeEntry;
use ccusage_adapter_common::json::JsonText;
use ccusage_core::{
    Diagnostics, LoadedEntry, ModelBreakdown, PricingMap, Speed, TimestampMs, TokenUsageRaw,
    cli::{CostMode, SharedArgs},
    format_rfc3339_millis,
};
use ccusage_test_support::{Environment, fs_fixture};
use jiff::tz::TimeZone as JiffTimeZone;

struct LoadedFile {
    entries: Vec<ClaudeEntry>,
}

fn read_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> LoadedFile {
    LoadedFile {
        entries: super::load::read_usage_file_with_diagnostics(
            path,
            crate::paths::fixture_projects_root(path).as_deref(),
            tz,
            pricing.map_or(ccusage_core::cost::PriceTable::Display, |map| {
                ccusage_core::cost::PriceTable::for_map(mode, map)
            }),
            diagnostics,
        ),
    }
}

use super::dedupe::push_deduped_entry;

fn read_usage_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> LoadedFile {
    read_usage_file_with_diagnostics(path, tz, mode, pricing, &Diagnostics::new("claude"))
}

#[test]
fn windowed_calculate_mode_prices_entries_with_shared_display_mode() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "projects/project-a/pricing.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"pricing","requestId":"known","message":{"id":"known","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
            r#"{"timestamp":"2026-01-02T10:01:00Z","sessionId":"pricing","requestId":"unknown","message":{"id":"unknown","model":"unlisted-fixture-model","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
        ].join("\n"),
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            offline: true,
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let loaded = super::load_windowed_entries(
            &shared,
            None,
            &Diagnostics::new("claude"),
            CostMode::Calculate,
        )
        .unwrap();
        assert_eq!(loaded.entries.len(), 2);
        let known = loaded
            .entries
            .iter()
            .find(|entry| entry.model.as_deref() == Some("claude-sonnet-4-20250514"))
            .unwrap();
        assert!((known.cost - 0.018).abs() < 1e-12, "{}", known.cost);
        assert_eq!(known.missing_pricing_model, None);
        let unknown = loaded
            .entries
            .iter()
            .find(|entry| entry.model.as_deref() == Some("unlisted-fixture-model"))
            .unwrap();
        assert_eq!(unknown.cost, 0.0);
        assert_eq!(
            unknown.missing_pricing_model.as_deref(),
            Some("unlisted-fixture-model")
        );
    })
}

#[test]
fn all_report_loaders_drop_timestamp_different_sidechain_replays() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"session-a","requestId":"parent","message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-02T10:00:01Z","sessionId":"session-a","requestId":"replay","isSidechain":true,"message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        ].join("\n"),
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 10);
    })
}

#[test]
fn all_report_loaders_count_agent_progress_usage() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"sessionId":"session-a","data":{"message":{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-progress","message":{"id":"msg-progress","model":"claude-sonnet-4-20250514","usage":{"input_tokens":7,"output_tokens":2}}}}}"#,
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 7);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 7);
    })
}

#[test]
fn all_report_loaders_prefer_usage_detail_over_stored_cost() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","costUSD":20,"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","costUSD":1,"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1,"speed":"fast"}}}"#,
        ].join("\n"),
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 1.0);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].total_cost, 1.0);
    })
}

#[test]
fn counts_usage_with_nulls_in_unrelated_nested_members() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","cwd":null,"toolUseResult":{"iterations":[{"model":null,"id":null,"cwd":null,"speed":null}]},"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","content":[{"id":null,"model":null}],"usage":{"input_tokens":10,"output_tokens":1,"metadata":{"cache_read_input_tokens":null}}}}"#,
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 10);
    })
}

#[test]
fn extracts_usage_limit_reset_time_from_raw_line() {
    let line = br#"{"timestamp":"2025-01-10T10:00:00.000Z","isApiErrorMessage":true,"message":{"content":[{"text":"Claude AI usage limit reached|1736503200 remaining"}],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
    let reset_time = usage_limit_reset_time_from_line_bytes(line, Some(true)).unwrap();

    assert_eq!(
        format_rfc3339_millis(reset_time),
        "2025-01-10T10:00:00.000Z"
    );
    assert!(usage_limit_reset_time_from_line_bytes(line, Some(false)).is_none());
    assert!(
        usage_limit_reset_time_from_line_bytes(
            br#"{"message":{"content":[{"text":"Claude AI usage limit reached|0"}]}}"#,
            Some(true)
        )
        .is_none()
    );
}

pub(crate) const SYNTHETIC_NULL_SPEED_LINE: &str = r#"{"timestamp":"2026-01-02T10:00:01.000Z","sessionId":"session-a","isApiErrorMessage":true,"message":{"id":"msg-synthetic","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"service_tier":null,"speed":null}}}"#;

#[test]
fn null_contract_rejections_claude_code_writes_stay_quiet() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            SYNTHETIC_NULL_SPEED_LINE,
        ]
        .join("\n"),
    });
    let diagnostics = ccusage_core::Diagnostics::new("claude");

    let loaded = read_usage_file_with_diagnostics(
        &fixture.path("projects/project-a/session-a.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        None,
        &diagnostics,
    );

    assert_eq!(loaded.entries.len(), 1);
    assert!(
        diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .is_empty()
    );
}

#[test]
fn limits_usage_file_discovery_to_requested_project() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/a.jsonl": "{}",
        "projects/project-b/session-b/b.jsonl": "{}",
    });

    let files = super::paths::usage_files(
        &[fixture.root().to_path_buf()],
        Some("project-a"),
        &ccusage_core::Diagnostics::new("claude"),
    );

    assert_eq!(files.len(), 1);
    assert!(files[0].to_string_lossy().contains("project-a"));
}

#[test]
fn filters_non_segment_projects_before_reading_files() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/a.jsonl": "{}",
        "projects/project-b/session-b/b.jsonl": "{}",
    });

    let files = super::paths::usage_files(
        &[fixture.root().to_path_buf()],
        Some("project-a/session-a"),
        &ccusage_core::Diagnostics::new("claude"),
    );

    assert!(files.is_empty());
}

#[test]
fn rejects_dot_segments_as_project_path_segments() {
    assert!(!is_project_path_segment(""));
    assert!(!is_project_path_segment("."));
    assert!(!is_project_path_segment(".."));
    assert!(!is_project_path_segment("project-a/session-a"));
    assert!(!is_project_path_segment("project-a\\session-a"));
    assert!(is_project_path_segment("project-a"));
}

#[test]
fn extracts_file_session_from_modern_claude_project_path() {
    let (session_id, project_path) = extract_session_parts(
        Path::new("/home/me/.claude/projects/project-a/session-a.jsonl"),
        Some(Path::new("/home/me/.claude/projects")),
    );

    assert_eq!(session_id, "session-a");
    assert_eq!(project_path, "project-a");
}

#[test]
fn extracts_parent_session_from_nested_claude_project_path() {
    let (session_id, project_path) = extract_session_parts(
        Path::new("/home/me/.claude/projects/project-a/session-a/chat.jsonl"),
        Some(Path::new("/home/me/.claude/projects")),
    );

    assert_eq!(session_id, "session-a");
    assert_eq!(project_path, "project-a");
}

#[test]
fn extracts_parent_session_from_claude_subagent_path() {
    let (session_id, project_path) = extract_session_parts(
        Path::new("/home/me/.claude/projects/project-a/session-a/subagents/worker.jsonl"),
        Some(Path::new("/home/me/.claude/projects")),
    );

    assert_eq!(session_id, "session-a");
    assert_eq!(project_path, "project-a");
}

#[test]
fn extracts_parent_session_from_claude_workflow_run_path() {
    let (session, project) = extract_session_parts(
        Path::new(
            "/home/user/.claude/projects/project-a/session-a/subagents/workflows/wf_1/agent-a.jsonl",
        ),
        Some(Path::new("/home/user/.claude/projects")),
    );
    assert_eq!(
        (session.as_str(), project.as_str()),
        ("session-a", "project-a")
    );
}

#[test]
fn extracts_the_project_relative_to_a_config_directory_under_projects() {
    let root = Path::new("/tmp/x/projects/cfg/projects");
    let transcript = Path::new("/tmp/x/projects/cfg/projects/myproj/session-a.jsonl");

    assert_eq!(
        super::paths::extract_project(transcript, Some(root)),
        "myproj"
    );
    assert_eq!(
        extract_session_parts(transcript, Some(root)),
        ("session-a".to_string(), "myproj".to_string())
    );
}

#[test]
fn reports_unknown_parts_without_a_projects_root() {
    assert_eq!(
        super::paths::extract_project(Path::new("s.jsonl"), None),
        "unknown"
    );
    assert_eq!(
        extract_session_parts(Path::new("s.jsonl"), None),
        ("unknown".to_string(), "Unknown Project".to_string())
    );
}

#[test]
fn reports_the_project_and_session_below_a_config_directory_under_projects() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "projects/cfg/projects/myproj/session-1.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"session-1","requestId":"req-1","message":{"id":"msg-1","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.path("projects/cfg")).run(|| {
        let shared = SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        let loaded = crate::load::load_deduped_entries(&shared, None).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        let entry = &loaded.entries[0];
        assert_eq!(entry.project.as_ref(), "myproj");
        assert_eq!(entry.project_path.as_ref(), "myproj");
        assert_eq!(entry.session_id.as_ref(), "session-1");

        let daily = crate::load::load_daily_summaries(&shared, None, true).unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].project.as_deref(), Some("myproj"));
    })
}

#[test]
fn filters_the_project_relative_to_the_discovered_projects_root() {
    let fixture = fs_fixture!({
        "projects/cfg/projects/myproj/session-1.jsonl": "{}",
        "projects/cfg/projects/other/session-2.jsonl": "{}",
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.path("projects/cfg")).run(|| {
        let paths = super::claude_paths().unwrap();
        let mut matching = Vec::new();
        super::visit_usage_files(
            &paths,
            Some("myproj"),
            &mut |path| matching.push(path),
            &Diagnostics::new("claude"),
        );

        assert_eq!(
            matching,
            [fixture.path("projects/cfg/projects/myproj/session-1.jsonl")]
        );
    })
}

#[test]
fn rejects_null_in_each_read_non_nullable_field() {
    for pointer in [
        "/sessionId",
        "/requestId",
        "/version",
        "/costUSD",
        "/isApiErrorMessage",
        "/message/id",
        "/message/model",
        "/message/usage/speed",
        "/message/usage/cache_read_input_tokens",
        "/message/usage/cache_creation_input_tokens",
    ] {
        let mut record = serde_json::json!({
            "timestamp": "2026-01-02T10:00:00Z", "sessionId": "session-a", "requestId": "request-a",
            "version": "1.2.3", "costUSD": 1, "isApiErrorMessage": false,
            "message": {"id": "msg-a", "model": "model-a", "usage": {"input_tokens": 10, "output_tokens": 1, "speed": "standard", "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}
        });
        *record.pointer_mut(pointer).unwrap() = serde_json::Value::Null;
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(
            super::daily::parse_usage_line(&JsonText::new(&bytes)).unwrap_err(),
            super::Rejection::NullContract,
            "{pointer}"
        );
        let wrapped =
            serde_json::json!({"sessionId":record["sessionId"], "data":{"message":record}});
        if pointer != "/version" {
            assert_eq!(
                super::daily::parse_usage_line(&JsonText::new(
                    &serde_json::to_vec(&wrapped).unwrap()
                ))
                .unwrap_err(),
                super::Rejection::NullContract,
                "wrapped {pointer}"
            );
        }
    }
}

#[test]
fn allows_missing_optional_fields_and_nullable_unread_members() {
    for extra in [
        r#","cwd":null,"content":[{"id":null,"model":null}],"toolUseResult":{"iterations":[{"model":null}]}"#,
        r#","isSidechain":null"#,
        "",
    ] {
        let line = format!(
            r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":1,"output_tokens":2,"iterations":[{{"type":"message","model":null}}]}}}}{extra}}}"#
        );
        assert!(
            super::daily::parse_usage_line(&JsonText::new(line.as_bytes())).is_ok(),
            "{line}"
        );
    }
}

#[test]
fn rejects_null_under_escaped_read_field_names() {
    for line in [
        br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"m\u006fdel":null,"usage":{"input_tokens":1,"output_tokens":2}}}"#.as_slice(),
        br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"sp\u0065ed":null}}}"#,
    ] {
        assert_eq!(
            super::daily::parse_usage_line(&JsonText::new(line)).unwrap_err(),
            super::Rejection::NullContract
        );
    }
    let line = JsonText::new(
        br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"cache_r\u0065ad_input_tokens":7,"sp\u0065ed":"fast"}}}"#,
    );
    let (record, _) = super::daily::parse_usage_line(&line).unwrap();
    assert_eq!(record.message.usage.usage.cache_read_input_tokens, 7);
    assert!(matches!(
        record.message.usage.usage.speed,
        Some(crate::Speed::Fast)
    ));
}

#[test]
fn malformed_required_usage_is_diagnosed_separately_from_null_contract() {
    for usage in [
        r#"{"input_tokens":null,"output_tokens":2}"#,
        r#"{"input_tokens":1,"output_tokens":null}"#,
        r#"{"input_tokens":1}"#,
    ] {
        let line =
            format!(r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{usage}}}}}"#);
        assert_eq!(
            super::daily::parse_usage_line(&JsonText::new(line.as_bytes())).unwrap_err(),
            super::Rejection::Malformed
        );
    }
}

#[test]
fn handles_duplicate_members_the_same_with_or_without_nulls() {
    for nulls in [
        "",
        r#","stop_sequence":null"#,
        r#","iterations":[{"type":"message","model":null}]"#,
    ] {
        let (message_nulls, usage_nulls) = if nulls.contains("iterations") {
            ("", nulls)
        } else {
            (nulls, "")
        };
        let duplicate_known = format!(
            r#"{{"timestamp":"2026-09-12T04:38:42.296Z","message":{{"id":"msg_1","model":"first","model":"second"{message_nulls},"usage":{{"input_tokens":2,"output_tokens":1{usage_nulls}}}}}}}"#
        );
        let duplicate_unknown = format!(
            r#"{{"timestamp":"2026-09-12T04:38:42.296Z","extra":1,"extra":2,"message":{{"id":"msg_1","model":"m"{message_nulls},"usage":{{"input_tokens":2,"output_tokens":1{usage_nulls}}}}}}}"#
        );

        assert!(
            super::daily::parse_usage_line(&JsonText::new(duplicate_known.as_bytes())).is_err(),
            "{duplicate_known}"
        );
        assert!(
            super::daily::parse_usage_line(&JsonText::new(duplicate_unknown.as_bytes())).is_ok(),
            "{duplicate_unknown}"
        );
    }
}

#[test]
fn rejects_duplicate_known_usage_fields_but_keeps_unknown_duplicates() {
    for (field, value) in [
        ("input_tokens", "1"),
        ("output_tokens", "2"),
        ("cache_read_input_tokens", "0"),
        ("cache_creation_input_tokens", "0"),
        ("speed", r#""fast""#),
        ("cache_creation", "{}"),
        ("iterations", "[]"),
    ] {
        let line = format!(
            r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":1,"output_tokens":2,"{field}":{value},"{field}":{value}}}}}}}"#
        );
        assert!(
            super::daily::parse_usage_line(&JsonText::new(line.as_bytes())).is_err(),
            "{field}"
        );
    }
    let line = br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"metadata":null,"metadata":{}}}}"#;
    assert!(super::daily::parse_usage_line(&JsonText::new(line)).is_ok());
}

#[test]
fn rejects_duplicate_members_before_typed_deserialization() {
    assert!(super::daily::parse_usage_line(&JsonText::new(
        br#"{"type":"assistant","timestamp":"2026-09-12T04:38:42.296Z","sessionId":"session-a","message":{"id":"msg_1","model":"first","model":"second","usage":{"input_tokens":2,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289}]}}}"#
))
    .is_err());
}

#[test]
fn counts_entries_whose_iteration_model_is_null() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"type":"assistant","timestamp":"2026-09-12T04:38:42.296Z","version":"2.1.268","sessionId":"session-a","requestId":"req_1","message":{"id":"msg_1","model":"claude-fable-5-1","role":"assistant","usage":{"input_tokens":2,"cache_creation_input_tokens":55866,"cache_read_input_tokens":0,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289,"cache_creation_input_tokens":55866,"cache_read_input_tokens":0}]}}}"#,
    });

    let loaded = read_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        Some(&PricingMap::default()),
    );

    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].model.as_deref(), Some("claude-fable-5-1"));
    assert_eq!(loaded.entries[0].usage.output_tokens, 289);
}

#[test]
fn calculates_advisor_cost_with_the_advisor_model() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg-parent","model":"main-model","usage":{"input_tokens":1,"output_tokens":2,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}]}},"requestId":"req-parent","costUSD":1.23}"#,
    });
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "main-model": {
                    "input_cost_per_token": 100,
                    "output_cost_per_token": 100
                },
                "advisor-model": {
                    "input_cost_per_token": 2,
                    "output_cost_per_token": 3
                }
            }"#,
    );

    let loaded = read_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        Some(&pricing),
    );

    assert_eq!(loaded.entries.len(), 2);
    assert_eq!(loaded.entries[0].cost, 1.23);
    assert_eq!(loaded.entries[1].model.as_deref(), Some("advisor-model"));
    assert_eq!(loaded.entries[1].cost, 26.0);
    assert_eq!(
        loaded.entries[1].original_model.as_deref(),
        Some("advisor-model")
    );
    assert_eq!(loaded.entries[1].recorded_cost, None);
}

#[test]
fn reads_usage_and_advisor_iterations_under_escaped_keys() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg-parent","model":"main-model","us\u0061ge":{"input_tokens":1,"output_tokens":2,"iterations":[{"type":"advisor\u005fmessage","model":"advisor-model","input_tokens":10,"output_tokens":2}]}},"requestId":"req-parent"}"#,
    });

    let loaded = read_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        None,
    );

    assert_eq!(loaded.entries.len(), 2);
    assert_eq!(loaded.entries[0].usage.output_tokens, 2);
    assert_eq!(loaded.entries[1].model.as_deref(), Some("advisor-model"));
}

#[test]
fn keeps_gateway_usage_from_distinct_sessions_with_reused_message_id() {
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
        let loaded = read_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded.entries {
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
fn keeps_requestless_usage_from_same_session_at_distinct_timestamps() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:41.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":250,"speed":"standard"}}}"#,
        ]
        .join("\n"),
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let loaded = read_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        None,
    );
    for entry in loaded.entries {
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
    }

    assert_eq!(deduped.len(), 2);
    assert_eq!(
        deduped
            .iter()
            .map(|entry| entry.usage.output_tokens)
            .sum::<u64>(),
        275
    );
}

#[test]
fn keeps_the_cache_ttl_split_from_an_otherwise_equal_duplicate() {
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
                single_thread: true,
                ..SharedArgs::default()
            };

            let deduped = load_deduped_entries(&shared, None).unwrap();

            assert_eq!(deduped.entries.len(), 1);
            let mut breakdown = ModelBreakdown::default();
            breakdown.add_usage(&deduped.entries[0].usage);
            assert_eq!(breakdown.cache_creation_tokens, 100);
            assert_eq!(breakdown.cache_creation_1h_tokens, Some(60));
        })
    }
}

#[test]
fn keeps_the_fast_tier_from_an_otherwise_equal_duplicate() {
    for lines in [
        [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"standard","cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"fast"}}}"#,
        ],
        [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"fast"}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"standard","cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
        ],
    ] {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": lines.join("\n"),
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            None,
        );
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }

        assert_eq!(deduped.len(), 1);
        assert_matches!(deduped[0].usage.speed, Some(Speed::Fast));
    }
}

fn chained_assistant(
    uuid: &str,
    parent: Option<&str>,
    timestamp: &str,
    message_id: &str,
    input_tokens: u64,
    tool_use: Option<&str>,
) -> String {
    let parent = parent.map_or_else(|| "null".to_string(), |parent| format!(r#""{parent}""#));
    let content = tool_use.map_or_else(
        || r#"[{"type":"text","text":"ok"}]"#.to_string(),
        |id| format!(r#"[{{"type":"tool_use","id":"{id}","name":"Read","input":{{}}}}]"#),
    );
    format!(
        r#"{{"parentUuid":{parent},"type":"assistant","uuid":"{uuid}","timestamp":"{timestamp}","sessionId":"session-a","message":{{"id":"{message_id}","model":"claude-sonnet-4-20250514","role":"assistant","content":{content},"usage":{{"input_tokens":{input_tokens},"output_tokens":25,"cache_read_input_tokens":0}}}}}}"#
    )
}

fn chained_user(uuid: &str, parent: &str, content: &str) -> String {
    format!(
        r#"{{"parentUuid":"{parent}","type":"user","uuid":"{uuid}","timestamp":"2026-05-22T02:34:40.250Z","sessionId":"session-a","message":{{"role":"user","content":{content}}}}}"#
    )
}

fn tool_result(tool_use_id: &str) -> String {
    format!(r#"[{{"type":"tool_result","tool_use_id":"{tool_use_id}","content":"ok"}}]"#)
}

fn message_first(
    uuid: &str,
    parent: Option<&str>,
    timestamp: &str,
    input: &str,
    trailing: &str,
) -> String {
    let parent = parent.map_or_else(|| "null".to_string(), |parent| format!(r#""{parent}""#));
    format!(
        r#"{{"parentUuid":{parent},"isSidechain":false,"message":{{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid}","name":"Read","input":{input}}}],"usage":{{"input_tokens":100,"output_tokens":25,"cache_read_input_tokens":0}}}},"type":"assistant","uuid":"{uuid}","timestamp":"{timestamp}","sessionId":"session-a"{trailing}}}"#
    )
}

fn linkless_assistant(timestamp: &str, message_id: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","sessionId":"session-a","message":{{"id":"{message_id}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":100,"output_tokens":25,"cache_read_input_tokens":0}}}}}}"#
    )
}

fn with_tool_input(line: String, input: &str) -> String {
    line.replace(r#""input":{}"#, &format!(r#""input":{input}"#))
}

const FIRST_BLOCK: &str = "2026-05-22T02:34:40.000Z";
const LATER_BLOCK: &str = "2026-05-22T02:34:40.500Z";

pub(crate) fn requestless_response_cases() -> Vec<(&'static str, Vec<String>, usize)> {
    let two_blocks = |message_id: &str| {
        vec![
            chained_assistant("a1", None, FIRST_BLOCK, message_id, 100, None),
            chained_assistant("a2", Some("a1"), LATER_BLOCK, message_id, 100, None),
        ]
    };
    let spaced_usage = |line: String| line.replace(r#""usage":{"#, r#""usage": {"#);
    vec![
        ("chained anthropic message id", two_blocks("msg_01AbC2DeF3"), 1),
        ("chained response message id", two_blocks("resp_0123456789abcdef"), 1),
        ("chained proxy message id", two_blocks("gateway-7f3a"), 1),
        (
            "chained blocks written with JSON whitespace",
            two_blocks("gateway-7f3a")
                .into_iter()
                .map(spaced_usage)
                .collect(),
            1,
        ),
        (
            "a block written with JSON whitespace does not bridge a response chain",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-1", 100, None),
                spaced_usage(chained_assistant(
                    "b1",
                    Some("a1"),
                    LATER_BLOCK,
                    "gateway-2",
                    200,
                    None,
                )),
                chained_assistant(
                    "a2",
                    Some("b1"),
                    "2026-05-22T02:34:41.000Z",
                    "gateway-1",
                    100,
                    None,
                ),
            ],
            3,
        ),
        (
            "blocks around their own tool result",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                chained_user("u1", "a1", &tool_result("toolu_1")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "blocks around a tool use on a later block",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("a2", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                chained_user("u1", "a2", &tool_result("toolu_1")),
                chained_assistant("a3", Some("u1"), "2026-05-22T02:34:41.000Z", "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "blocks around a tool result whose text holds quotes and brackets",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                chained_user(
                    "u1",
                    "a1",
                    r#"[{"tool_use_id":"toolu_1","type":"tool_result","content":"a \"}]} {\"type\":\"text\"} \\"}]"#,
                ),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "blocks around their own agent tool result",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                r#"{"parentUuid":"a1","type":"user","uuid":"u1","timestamp":"2026-05-22T02:34:40.250Z","sessionId":"session-a","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"done"}]},"toolUseResult":{"usage":{"input_tokens":7777,"output_tokens":7777}}}"#.to_string(),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "blocks through a non-usage line",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                r#"{"parentUuid":"a1","type":"system","uuid":"s1","timestamp":"2026-05-22T02:34:40.250Z","sessionId":"session-a"}"#.to_string(),
                chained_assistant("a2", Some("s1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "reused id after a tool result with new input",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "msg_01AbC2DeF3", 100, Some("toolu_1")),
                chained_user("u1", "a1", &tool_result("toolu_1")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "msg_01AbC2DeF3", 150, None),
            ],
            2,
        ),
        (
            "reused id after a user prompt",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "msg_01AbC2DeF3", 100, None),
                chained_user("u1", "a1", r#""next question""#),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "msg_01AbC2DeF3", 100, None),
            ],
            2,
        ),
        (
            "reused id after a tool result for another response",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                chained_user("u1", "a1", &tool_result("toolu_9")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "blocks whose tool input holds a uuid",
            vec![
                with_tool_input(
                    chained_assistant("a1", None, FIRST_BLOCK, "msg_01AbC2DeF3", 100, Some("toolu_1")),
                    r#"{"path":"a","uuid":"x"}"#,
                ),
                chained_assistant("a2", Some("a1"), LATER_BLOCK, "msg_01AbC2DeF3", 100, None),
            ],
            1,
        ),
        (
            "message-first blocks whose tool input holds a uuid",
            vec![
                message_first("a1", None, FIRST_BLOCK, r#"{"uuid":"x"}"#, ""),
                message_first("a2", Some("a1"), LATER_BLOCK, "{}", ""),
            ],
            1,
        ),
        (
            "message-first blocks with a trailing object that holds a uuid",
            vec![
                message_first("a1", None, FIRST_BLOCK, "{}", r#","toolUseResult":{"a":1,"uuid":"x1"}"#),
                message_first("a2", Some("a1"), LATER_BLOCK, "{}", r#","toolUseResult":{"a":1,"uuid":"x2"}"#),
            ],
            1,
        ),
        (
            "distinct responses whose tool inputs share a uuid",
            vec![
                with_tool_input(
                    chained_assistant("a1", None, FIRST_BLOCK, "msg_01AbC2DeF3", 100, Some("toolu_1")),
                    r#"{"uuid":"shared"}"#,
                ),
                chained_user("u1", "a1", r#""next question""#),
                with_tool_input(
                    chained_assistant("a2", Some("u1"), LATER_BLOCK, "msg_01AbC2DeF3", 100, Some("toolu_2")),
                    r#"{"uuid":"shared"}"#,
                ),
            ],
            2,
        ),
        (
            "reused id after a tool result for an id named only in tool input",
            vec![
                with_tool_input(
                    chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_own")),
                    r#"{"id":"toolu_foreign"}"#,
                ),
                chained_user("u1", "a1", &tool_result("toolu_foreign")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "reused id after a user prompt whose type follows its message",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                r#"{"parentUuid":"a1","uuid":"u1","message":{"role":"user","content":"next question"},"type":"user","timestamp":"2026-05-22T02:34:40.250Z","sessionId":"session-a"}"#.to_string(),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "later block of a discarded same-timestamp rewrite",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("a1-rewrite", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("a2", Some("a1-rewrite"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "later block of the original after a surviving same-timestamp rewrite",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("a1-rewrite", None, FIRST_BLOCK, "gateway-7f3a", 100, None)
                    .replace(r#""output_tokens":25"#, r#""output_tokens":250"#),
                chained_assistant("a2", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "reused id after a user prompt whose type is escaped",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_user("u1", "a1", r#""next question""#).replace(r#""type":"user""#, r#""type":"\u0075ser""#),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "blocks around a tool result for a tool whose input holds usage",
            vec![
                with_tool_input(
                    chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                    r#"{"usage":{"window":"7d"}}"#,
                ),
                chained_user("u1", "a1", &tool_result("toolu_1")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "linkless record at the timestamp of a replaced survivor",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("a2", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, None)
                    .replace(r#""output_tokens":25"#, r#""output_tokens":250"#),
                linkless_assistant(FIRST_BLOCK, "gateway-7f3a"),
            ],
            1,
        ),
        (
            "linkless record at the timestamp of a discarded block",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None)
                    .replace(r#""output_tokens":25"#, r#""output_tokens":250"#),
                chained_assistant("a2", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, None),
                linkless_assistant(LATER_BLOCK, "gateway-7f3a"),
            ],
            1,
        ),
        (
            "block that connects an earlier response and a same-timestamp record",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("b1", None, LATER_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("c1", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            1,
        ),
        (
            "blocks whose parent belongs to another session",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                chained_assistant("b1", Some("a1"), LATER_BLOCK, "gateway-7f3a", 100, None)
                    .replace(r#""sessionId":"session-a""#, r#""sessionId":"session-b""#),
                chained_assistant("b2", Some("a1"), "2026-05-22T02:34:41.000Z", "gateway-7f3a", 100, None)
                    .replace(r#""sessionId":"session-a""#, r#""sessionId":"session-b""#),
            ],
            3,
        ),
        (
            "blocks around a tool result after another session reused the tool id",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, Some("toolu_1")),
                chained_assistant("b1", None, FIRST_BLOCK, "gateway-copy", 100, Some("toolu_1"))
                    .replace(r#""sessionId":"session-a""#, r#""sessionId":"session-b""#),
                chained_user("u1", "a1", &tool_result("toolu_1")),
                chained_assistant("a2", Some("u1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "requestless record at the timestamp of a sidechain replay its parent absorbed",
            vec![
                linkless_assistant(FIRST_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","isSidechain":true,"#,
                ),
                linkless_assistant(LATER_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","requestId":"req_parent","#,
                ),
                linkless_assistant(FIRST_BLOCK, "msg_parent"),
            ],
            1,
        ),
        (
            "sidechain replay of a response whose first block came from another session",
            vec![
                linkless_assistant(FIRST_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","isSidechain":true,"#,
                ),
                linkless_assistant(LATER_BLOCK, "msg_parent")
                    .replace(r#""type":"assistant","#, r#""type":"assistant","requestId":"req_parent","#)
                    .replace(r#""sessionId":"session-a""#, r#""sessionId":"session-b""#),
                linkless_assistant("2026-05-22T02:34:41.000Z", "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","requestId":"req_parent","#,
                ),
            ],
            1,
        ),
        (
            "explicit parent written again after a requestless rewrite won its slot",
            vec![
                linkless_assistant(FIRST_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","isSidechain":true,"#,
                ),
                linkless_assistant(LATER_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","requestId":"req_parent","#,
                ),
                linkless_assistant(FIRST_BLOCK, "msg_parent")
                    .replace(r#""output_tokens":25"#, r#""output_tokens":500"#),
                linkless_assistant("2026-05-22T02:34:41.000Z", "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","requestId":"req_parent","#,
                ),
            ],
            1,
        ),
        (
            "explicit parent after a requestless record took its replay's slot",
            vec![
                linkless_assistant(FIRST_BLOCK, "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","isSidechain":true,"#,
                ),
                linkless_assistant(LATER_BLOCK, "msg_parent")
                    .replace(r#""type":"assistant","#, r#""type":"assistant","requestId":"req_parent","#)
                    .replace(r#""sessionId":"session-a""#, r#""sessionId":"session-b""#),
                linkless_assistant(FIRST_BLOCK, "msg_parent")
                    .replace(r#""output_tokens":25"#, r#""output_tokens":500"#),
                linkless_assistant("2026-05-22T02:34:41.000Z", "msg_parent").replace(
                    r#""type":"assistant","#,
                    r#""type":"assistant","requestId":"req_parent","#,
                ),
            ],
            1,
        ),
        (
            "blocks around an interrupted write that the next record continued",
            vec![
                chained_assistant("a1", None, FIRST_BLOCK, "gateway-7f3a", 100, None),
                [
                    r#"{"parentUuid":"a1","isSidechain":false,"sessionId":"session-a","type":"attachment","uuid":"x1","times"#,
                    r#"{"parentUuid":"a1","isSidechain":false,"sessionId":"session-a","type":"system","uuid":"s1","timestamp":"2026-05-22T02:34:40.250Z"}"#,
                ]
                .concat(),
                chained_assistant("a2", Some("x1"), LATER_BLOCK, "gateway-7f3a", 100, None),
            ],
            2,
        ),
        (
            "blocks without transcript links",
            vec![
                format!(
                    r#"{{"timestamp":"{FIRST_BLOCK}","message":{{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":100,"output_tokens":25}}}}}}"#
                ),
                format!(
                    r#"{{"timestamp":"{LATER_BLOCK}","message":{{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":100,"output_tokens":25}}}}}}"#
                ),
            ],
            2,
        ),
    ]
}

#[test]
fn requestless_content_blocks_collapse_only_along_their_transcript_chain() {
    let mut failures = Vec::new();
    for (case, lines, responses) in requestless_response_cases() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": lines.join("\n"),
        });
        let path = fixture.path("projects/project-a/session-a/chat.jsonl");
        let loaded = read_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        let (deduped, _) = crate::dedupe::dedupe_files(vec![(path, loaded.entries)]);

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
fn dedupes_repeated_requestless_writes_at_the_same_timestamp() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": [
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
            r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":250,"speed":"standard"}}}"#,
        ]
        .join("\n"),
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let loaded = read_usage_file(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        None,
    );
    for entry in loaded.entries {
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
    }

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.output_tokens, 250);
}

#[test]
fn dedupes_copied_transcripts_with_the_same_serialized_session_id() {
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
        let loaded = read_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }
    }

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.input_tokens, 200);
}

#[test]
fn skips_and_diagnoses_copied_transcripts_whose_token_buckets_exceed_a_u64() {
    let line = r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","message":{"id":"huge","model":"claude-sonnet-4-20250514","usage":{"input_tokens":18446744073709551615,"output_tokens":50}}}"#;
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": [line, line].join("\n"),
    });
    let diagnostics = Diagnostics::new("claude");
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let loaded = read_usage_file_with_diagnostics(
        &fixture.path("projects/project-a/session-a/chat.jsonl"),
        &JiffTimeZone::UTC,
        CostMode::Display,
        None,
        &diagnostics,
    );
    for entry in loaded.entries {
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
    }

    assert!(deduped.is_empty());
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 2);
}

#[test]
fn dedupes_copied_transcripts_with_the_same_request_id_across_sessions() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    for path in [
        fixture.path("projects/project-a/session-a/chat.jsonl"),
        fixture.path("projects/project-a/session-b/chat.jsonl"),
    ] {
        let loaded = read_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None);
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }
    }

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.input_tokens, 200);
}

#[test]
fn returns_the_discarded_copy_from_another_session() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
        "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        "projects/project-a/session-c/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-c","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":50,"output_tokens":1}}}"#,
    });
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let discarded = ["a", "b", "c"].map(|session| {
        let path = fixture.path(format!("projects/project-a/session-{session}/chat.jsonl"));
        let entry = read_usage_file(&path, &jiff::tz::TimeZone::UTC, CostMode::Display, None)
            .entries
            .into_iter()
            .next()
            .unwrap();
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
            .map(|(index, discarded)| (index, discarded.session_id.to_string()))
    });

    assert_eq!(
        discarded,
        [
            None,
            Some((0, "session-a".to_string())),
            Some((0, "session-c".to_string())),
        ]
    );
    assert_eq!(deduped[0].session_id.as_ref(), "session-b");
}

#[test]
fn attributes_a_cross_session_copy_to_the_survivor_day() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        "projects/project-a/session-b.jsonl": [
            r#"{"timestamp":"2026-01-01T10:00:00.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-03T10:00:00.000Z","sessionId":"session-b","requestId":"req-own","message":{"id":"msg-own","model":"claude-sonnet-4-20250514","usage":{"input_tokens":7,"output_tokens":1}}}"#,
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
            json: true,
            ..SharedArgs::default()
        };
        let deduped = load_deduped_entries(&shared, None).unwrap();
        let sorted_days = |deduped: &crate::load::DedupedEntries| {
            let mut days: Vec<_> = deduped
                .main_thread_sessions()
                .into_iter()
                .map(|session| session.to_string())
                .collect();
            days.sort();
            days
        };

        assert_eq!(deduped.entries.len(), 2);
        assert_eq!(sorted_days(&deduped), ["session-a", "session-b"]);
        let filtered = crate::load::DedupedEntries {
            entries: deduped
                .entries
                .into_iter()
                .filter(|entry| entry.date != crate::cli::IsoDate::from_ymd(2026, 1, 2).unwrap())
                .collect(),
            ..Default::default()
        };
        assert_eq!(sorted_days(&filtered), ["session-b"]);
    })
}

#[test]
fn dedupes_sidechain_replay_after_an_equal_copy_from_another_session() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let mut copied_parent = loaded_usage_entry(UsageEntryFixture {
        message_id: "msg-parent",
        request_id: "req-parent",
        is_sidechain: false,
        cache_read_tokens: 20,
        output_tokens: 10,
    });
    copied_parent.recorded_session_id = Some(Arc::from("session-b"));
    copied_parent.entry.session_id = Arc::from("session-b");
    push_deduped_entry(copied_parent, &mut deduped_indexes, &mut deduped);

    push_deduped_entry(
        loaded_usage_entry(UsageEntryFixture {
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
        loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-sidechain-replay",
            is_sidechain: true,
            cache_read_tokens: 50_000,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
}

#[test]
fn dedupes_sidechain_replay_after_cross_session_survivor_replacement() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    let mut copied_parent = loaded_usage_entry(UsageEntryFixture {
        message_id: "msg-parent",
        request_id: "req-parent",
        is_sidechain: false,
        cache_read_tokens: 20,
        output_tokens: 10,
    });
    copied_parent.recorded_session_id = Some(Arc::from("session-b"));
    copied_parent.entry.session_id = Arc::from("session-b");
    push_deduped_entry(copied_parent, &mut deduped_indexes, &mut deduped);

    push_deduped_entry(
        loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 30,
            output_tokens: 10,
        }),
        &mut deduped_indexes,
        &mut deduped,
    );

    let mut copied_session_replay = loaded_usage_entry(UsageEntryFixture {
        message_id: "msg-parent",
        request_id: "req-sidechain-replay",
        is_sidechain: true,
        cache_read_tokens: 50_000,
        output_tokens: 10,
    });
    copied_session_replay.recorded_session_id = Some(Arc::from("session-b"));
    copied_session_replay.entry.session_id = Arc::from("session-b");
    push_deduped_entry(copied_session_replay, &mut deduped_indexes, &mut deduped);

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].request.id(), Some("req-parent"));
    assert_eq!(deduped[0].session_id.as_ref(), "session-a");
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 30);
}

#[test]
fn keeps_parent_usage_when_sidechain_replays_message_with_new_request_id() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        loaded_usage_entry(UsageEntryFixture {
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
        loaded_usage_entry(UsageEntryFixture {
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
        loaded_usage_entry(UsageEntryFixture {
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
    assert_eq!(deduped[0].message_id.as_deref(), Some("msg-parent"));
    assert_eq!(deduped[0].request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
    assert_eq!(
        deduped[1].message_id.as_deref(),
        Some("msg-sidechain-answer")
    );
    assert_eq!(deduped[1].usage.cache_read_input_tokens, 700);
}

#[test]
fn refreshes_dedupe_indexes_when_parent_replaces_sidechain_replay() {
    let mut deduped_indexes = Default::default();
    let mut deduped = Vec::new();

    push_deduped_entry(
        loaded_usage_entry(UsageEntryFixture {
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
        loaded_usage_entry(UsageEntryFixture {
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
        loaded_usage_entry(UsageEntryFixture {
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
    assert_eq!(deduped[0].request.id(), Some("req-parent"));
    assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
}

struct UsageEntryFixture {
    message_id: &'static str,
    request_id: &'static str,
    is_sidechain: bool,
    cache_read_tokens: u64,
    output_tokens: u64,
}

fn loaded_usage_entry(fixture: UsageEntryFixture) -> ClaudeEntry {
    ClaudeEntry {
        source_timestamp: "2026-03-29T07:00:00.000Z".to_string(),
        recorded_session_id: Some(Arc::from("session-a")),
        original_model: Some("claude-sonnet-4-20250514".to_string()),
        request: crate::dedupe::Request::Id(fixture.request_id.into()),
        is_sidechain: fixture.is_sidechain,
        entry: LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 0,
                output_tokens: fixture.output_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: fixture.cache_read_tokens,
                speed: None,
                cache_creation: None,
            },
            message_id: Some(fixture.message_id.to_string()),
            version: Some("1.0.0".to_string()),
            recorded_cost: None,
            timestamp: TimestampMs::from_millis(1_775_000_000_000),
            date: ccusage_core::cli::IsoDate::parse("2026-03-29").unwrap(),
            project: Arc::from("project-a"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("project-a"),
            cost: 0.0,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            stored_cost: None,
            estimated_pricing: false,
        },
    }
}

#[cfg(unix)]
const INVALID_UTF8_PROJECTS: [(&[u8], u64); 2] = [(b"p\xfe", 100), (b"p\xff", 1)];

#[cfg(unix)]
fn shared_copy_line(input_tokens: u64) -> String {
    let output_tokens = 101 - input_tokens;
    format!(
        r#"{{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session","requestId":"req-shared","message":{{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":{input_tokens},"output_tokens":{output_tokens}}}}}}}"#
    )
}

#[cfg(unix)]
#[test]
fn dedupe_keeps_the_same_copy_from_project_dirs_that_differ_only_in_invalid_utf8() {
    use std::os::unix::ffi::OsStrExt;

    let fixture = ccusage_test_support::Fixture::new();
    let mut files = INVALID_UTF8_PROJECTS
        .iter()
        .enumerate()
        .map(|(index, &(name, input_tokens))| {
            let readable = fixture.write_file(
                format!("{index}/session.jsonl"),
                shared_copy_line(input_tokens),
            );
            let loaded = read_usage_file(&readable, &JiffTimeZone::UTC, CostMode::Display, None);
            let path = Path::new("/projects")
                .join(std::ffi::OsStr::from_bytes(name))
                .join("session.jsonl");
            (path, loaded.entries)
        })
        .collect::<Vec<_>>();
    let survivor = |files: Vec<(std::path::PathBuf, Vec<ClaudeEntry>)>| {
        let (entries, _) = super::dedupe::dedupe_files(files);
        assert_eq!(entries.len(), 1);
        entries[0].usage.input_tokens
    };

    let forward = survivor(files.clone());
    files.reverse();

    assert_eq!(forward, survivor(files));
}

#[cfg(unix)]
#[test]
fn single_and_multi_thread_keep_the_same_copy_from_invalid_utf8_project_dirs() {
    use std::os::unix::ffi::OsStrExt;

    let fixture = ccusage_test_support::Fixture::new();
    for (name, input_tokens) in INVALID_UTF8_PROJECTS {
        let dir = fixture
            .root()
            .join("projects")
            .join(std::ffi::OsStr::from_bytes(name));
        if std::fs::create_dir_all(&dir).is_err() {
            // NOTE(2026-11-20, #2359): APFS rejects invalid UTF-8 file names with EILSEQ, so this fixture cannot exist on macOS. The test runs on Linux CI instead.
            return;
        }
        std::fs::write(dir.join("session.jsonl"), shared_copy_line(input_tokens)).unwrap();
    }
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let load = |single_thread| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..Default::default()
            };
            let entries = super::load_entries(&shared, None).unwrap();
            let daily = super::load_daily_summaries(&shared, None, false).unwrap();
            (
                entries
                    .iter()
                    .map(|entry| (entry.usage.input_tokens, entry.project.to_string()))
                    .collect::<Vec<_>>(),
                daily.iter().map(|day| day.input_tokens).collect::<Vec<_>>(),
            )
        };
        let single = load(true);
        for _ in 0..20 {
            assert_eq!(load(false), single);
        }
    });
}
