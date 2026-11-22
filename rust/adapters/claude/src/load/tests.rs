use ccusage_core::summary::{KeyTarget, summarize_by_day};
use ccusage_test_support::{Environment, fs_fixture};
use serde_json::Value;

use ccusage_core::{
    LoadedEntry, UsageSummary,
    cli::{CostMode, SharedArgs},
    summarize_by_key_target,
};

use crate::{load_daily_summaries, load_entries};

fn display_shared() -> SharedArgs {
    SharedArgs {
        mode: CostMode::Display,
        ..SharedArgs::default()
    }
}

fn display_shared_in_utc() -> SharedArgs {
    SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..display_shared()
    }
}

fn rows_json(rows: &[UsageSummary]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

fn daily_from_entries(entries: &[LoadedEntry]) -> Vec<UsageSummary> {
    summarize_by_day(entries).unwrap()
}

#[test]
fn bounded_loaders_filter_after_dedupe_and_keep_full_history_available() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": [
            r#"{"timestamp":"2026-01-01T23:00:00Z","sessionId":"s1","requestId":"r1","costUSD":1,"message":{"id":"msg_1","usage":{"input_tokens":20,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-02T01:00:00Z","sessionId":"s1","requestId":"r1","costUSD":1,"message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-02T23:00:00Z","sessionId":"s1","requestId":"r2","costUSD":2,"message":{"id":"msg_2","usage":{"input_tokens":30,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-03T01:00:00Z","sessionId":"s1","requestId":"r3","costUSD":3,"message":{"id":"msg_3","usage":{"input_tokens":40,"output_tokens":1}}}"#,
        ].join("\n"),
        "projects/project1/session2/chat.jsonl": r#"{"timestamp":"2026-01-02T23:00:00Z","sessionId":"s2","requestId":"r2","costUSD":2,"message":{"id":"msg_2","usage":{"input_tokens":30,"output_tokens":1}}}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = SharedArgs {
            since: Some("20260102".parse().unwrap()),
            until: Some("20260102".parse().unwrap()),
            ..display_shared_in_utc()
        };
        let entries = crate::load_entries_since(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 2.0);
        assert_eq!(load_entries(&shared, None).unwrap().len(), 3);
        let loaded = crate::load_daily_summaries_with_detection_progress(
            &shared,
            None,
            None,
            ccusage_adapter_common::source::ProgressOwner::Caller,
            &crate::Diagnostics::new("claude"),
            || true,
        )
        .unwrap();
        assert!(loaded.detected);
        assert_eq!(loaded.summaries.len(), 1);
        assert_eq!(loaded.summaries[0].input_tokens, 30);
        assert_eq!(loaded.sessions.len(), 2);
        assert!(
            loaded
                .sessions
                .iter()
                .all(|session| ["s1", "s2"].contains(&session.as_ref()))
        );
    })
}

#[test]
fn bounded_daily_detection_survives_an_empty_report_window() {
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": r#"{"timestamp":"2026-01-03T00:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1}}}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = SharedArgs {
            until: Some("20260102".parse().unwrap()),
            ..display_shared_in_utc()
        };
        let loaded = crate::load_daily_summaries_with_detection_progress(
            &shared,
            None,
            None,
            ccusage_adapter_common::source::ProgressOwner::Caller,
            &crate::Diagnostics::new("claude"),
            || true,
        )
        .unwrap();
        assert!(loaded.detected);
        assert!(loaded.summaries.is_empty());
        assert!(loaded.sessions.is_empty());
    })
}

#[test]
fn keeps_most_complete_duplicate_usage_entry() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": [
            r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"requestId":"req_456","costUSD":0.001}"#,
            r#"{"timestamp":"2025-01-10T10:00:01.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"requestId":"req_456","costUSD":0.01}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 100);
        assert_eq!(entries[0].usage.output_tokens, 250);
        assert_eq!(entries[0].cost, 0.01);
    })
}

#[test]
fn keeps_reused_message_id_from_distinct_sessions_at_same_timestamp() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
        "projects/project1/session2/chat.jsonl": r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"costUSD":0.01}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.usage.output_tokens)
                .sum::<u64>(),
            275
        );
    })
}

#[test]
fn dedupes_chained_anthropic_message_id_from_same_session_without_request_id() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": [
            r#"{"uuid":"a1","parentUuid":null,"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
            r#"{"uuid":"a2","parentUuid":"a1","timestamp":"2025-01-10T10:00:01.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"costUSD":0.01}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.output_tokens, 250);
        assert_eq!(entries[0].cost, 0.01);
    })
}

#[test]
fn keeps_gateway_message_id_from_same_session_without_request_id() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": [
            r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"gateway_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
            r#"{"timestamp":"2025-01-10T10:00:01.000Z","message":{"id":"gateway_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"costUSD":0.01}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.usage.output_tokens)
                .sum::<u64>(),
            275
        );
        assert_eq!(entries.iter().map(|entry| entry.cost).sum::<f64>(), 0.011);
    })
}

#[test]
fn dedupes_repeated_requestless_writes_from_same_session_at_the_same_timestamp() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": [
            r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"gateway_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
            r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"gateway_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"costUSD":0.01}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.output_tokens, 250);
        assert_eq!(entries[0].cost, 0.01);
    })
}

#[test]
fn accepts_projects_directory_in_claude_config_dir() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": r#"{"timestamp":"2025-01-10T10:00:00.000Z","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"costUSD":0.001}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.path("projects")).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project.as_ref(), "project1");
    })
}

const ADVISOR_LINE: &str = r#"{"timestamp":"2026-05-22T02:34:40.000Z","version":"1.2.3","sessionId":"session1","advisorModel":"claude-opus-4-20250514","message":{"id":"msg_123","model":"claude-sonnet-4-20250514","usage":{"input_tokens":2,"output_tokens":491,"cache_creation_input_tokens":7853,"cache_read_input_tokens":226584,"iterations":[{"type":"message","input_tokens":1,"output_tokens":45,"cache_creation_input_tokens":7192,"cache_read_input_tokens":109696},{"type":"advisor_message","model":"claude-opus-4-20250514","input_tokens":159419,"output_tokens":7805,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},{"type":"message","input_tokens":1,"output_tokens":446,"cache_creation_input_tokens":661,"cache_read_input_tokens":116888}]}},"requestId":"req_456","costUSD":1.23}"#;

#[test]
fn loads_advisor_usage_as_a_separate_model_entry() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": ADVISOR_LINE,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = load_entries(&display_shared(), None).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].usage.input_tokens, 2);
        assert_eq!(entries[0].usage.output_tokens, 491);
        assert_eq!(entries[1].model.as_deref(), Some("claude-opus-4-20250514"));
        assert_eq!(entries[1].usage.input_tokens, 159_419);
        assert_eq!(entries[1].usage.output_tokens, 7_805);
    })
}

#[test]
fn includes_advisor_usage_in_daily_summaries() {
    let fixture = fs_fixture!({
        "projects/project1/session1/chat.jsonl": ADVISOR_LINE,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let daily = load_daily_summaries(&display_shared_in_utc(), None, false).unwrap();

        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].input_tokens, 159_421);
        assert_eq!(daily[0].output_tokens, 8_296);
        assert_eq!(
            daily[0].models_used,
            ["claude-sonnet-4-20250514", "claude-opus-4-20250514"]
        );
    })
}

#[test]
fn daily_summaries_match_entry_aggregation_for_requestless_rewrites() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": [
            r#"{"timestamp":"2026-09-03T12:00:00.000Z","sessionId":"session-a","message":{"id":"msg-gateway","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
            r#"{"timestamp":"2026-09-04T12:00:00.000Z","sessionId":"session-a","message":{"id":"msg-gateway","model":"claude-sonnet-4-20250514","usage":{"input_tokens":30,"output_tokens":4}}}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = display_shared_in_utc();

        let entries = load_entries(&shared, None).unwrap();
        let daily = load_daily_summaries(&shared, None, false).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(rows_json(&daily), rows_json(&daily_from_entries(&entries)));
        assert_eq!(daily.iter().map(|row| row.total_tokens()).sum::<u64>(), 46);
    })
}

#[test]
fn loads_daily_summaries_like_loaded_entry_aggregation() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a/chat.jsonl": [
            r#"{"timestamp":"2025-01-10T09:59:00.000Z","version":"not-semver","message":{"id":"bad","model":"claude-opus-4-6","usage":{"input_tokens":999,"output_tokens":999,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"bad","costUSD":9}"#,
            r#"{"timestamp":"2025-01-10T10:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}},"requestId":"req_456","costUSD":0.001}"#,
            r#"{"timestamp":"2025-01-10T10:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_123","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":250,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"speed":"standard"}},"requestId":"req_456","costUSD":0.01}"#,
            r#"{"timestamp":"2025-01-11T11:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_789","model":"<synthetic>","usage":{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_789","costUSD":0.02}"#,
        ]
        .join("\n"),
        "projects/project-b/session-b/chat.jsonl": r#"{"timestamp":"2025-01-10T12:00:00.000Z","version":"1.2.3","sessionId":"session-b","message":{"id":"msg_b","model":"claude-sonnet-4-20250514","usage":{"input_tokens":20,"output_tokens":30,"cache_creation_input_tokens":4,"cache_read_input_tokens":2}},"requestId":"req_b","costUSD":0.04}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = display_shared_in_utc();

        let entries = load_entries(&shared, None).unwrap();
        let daily = load_daily_summaries(&shared, None, false).unwrap();
        let grouped_daily = load_daily_summaries(&shared, None, true).unwrap();

        assert_eq!(rows_json(&daily), rows_json(&daily_from_entries(&entries)));
        let expected_grouped_daily = summarize_by_key_target(
            &entries,
            |entry| format!("{}\0{}", entry.date, entry.project),
            |key| {
                let mut parts = key.split('\0');
                (
                    parts.next().unwrap_or_default().to_string(),
                    parts.next().map(str::to_string),
                )
            },
            KeyTarget::Date,
        )
        .unwrap();
        assert_eq!(
            rows_json(&grouped_daily),
            rows_json(&expected_grouped_daily)
        );
    })
}

#[test]
fn keeps_nested_agent_progress_in_daily_model_order() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2026-03-10T06:00:00.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_fast","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":10,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"speed":"fast"}},"requestId":"req_fast","costUSD":0.03}"#,
            r#"{"sessionId":"session-a","version":"1.2.3","type":"progress","data":{"message":{"type":"assistant","timestamp":"2026-03-10T06:00:01.000Z","message":{"model":"claude-haiku-4-5-20251001","id":"msg_haiku","type":"message","role":"assistant","content":[],"usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","uuid":"nested"}},"timestamp":"2026-03-10T06:00:01.001Z"}"#,
            r#"{"timestamp":"2026-03-10T06:00:02.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_opus","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":30,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_opus","costUSD":0.09}"#,
        ]
        .join("\n"),
        "projects/project-a/session-a/subagents/agent-a.jsonl": r#"{"timestamp":"2026-03-10T06:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_haiku","model":"claude-haiku-4-5-20251001","role":"assistant","usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","costUSD":0.06}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let daily = load_daily_summaries(&display_shared_in_utc(), None, false).unwrap();

        assert_eq!(daily.len(), 1);
        assert_eq!(
            daily[0].models_used,
            [
                "claude-opus-4-6-fast",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-6",
            ]
        );
        assert_eq!(daily[0].total_cost, 0.18);
    })
}

#[test]
fn uses_direct_subagent_cost_for_duplicate_daily_agent_progress() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": r#"{"sessionId":"session-a","version":"1.2.3","type":"progress","data":{"message":{"type":"assistant","timestamp":"2026-03-10T06:00:01.000Z","message":{"model":"claude-haiku-4-5-20251001","id":"msg_haiku","type":"message","role":"assistant","content":[],"usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","uuid":"nested"}},"timestamp":"2026-03-10T06:00:01.001Z"}"#,
        "projects/project-a/session-a/subagents/agent-a.jsonl": r#"{"timestamp":"2026-03-10T06:00:01.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg_haiku","model":"claude-haiku-4-5-20251001","role":"assistant","usage":{"input_tokens":20,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"requestId":"req_haiku","costUSD":0.06}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = display_shared_in_utc();

        let entries = load_entries(&shared, None).unwrap();
        let daily = load_daily_summaries(&shared, None, false).unwrap();

        assert_eq!(daily[0].total_cost, 0.06);
        assert_eq!(rows_json(&daily), rows_json(&daily_from_entries(&entries)));
    })
}

#[test]
fn wrapped_advisors_share_daily_and_full_parsing_and_dedupe() {
    ccusage_test_support::install_pricing_snapshots();
    let wrapped = r#"{"type":"progress","sessionId":"session-a","data":{"type":"agent_progress","message":{"type":"assistant","timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"claude-opus-5","input_tokens":10,"output_tokens":2}]}}}}}"#;
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": [wrapped, wrapped].join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        for mode in [CostMode::Auto, CostMode::Calculate, CostMode::Display] {
            let shared = SharedArgs {
                offline: true,
                mode,
                ..display_shared_in_utc()
            };
            let entries = load_entries(&shared, None).unwrap();
            assert_eq!(entries.len(), 2, "{mode:?}");
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.usage.input_tokens)
                    .sum::<u64>(),
                110
            );
            let daily = load_daily_summaries(&shared, None, false).unwrap();
            assert_eq!(daily.len(), 1);
            assert_eq!(daily[0].input_tokens, 110);
            assert_eq!(daily[0].models_used.len(), 2);
            assert!(
                (daily[0].total_cost - entries.iter().map(|entry| entry.cost).sum::<f64>()).abs()
                    < 1e-12
            );
        }
    })
}
#[test]
fn wrapped_advisors_dedupe_with_a_top_level_twin_and_ignore_outer_iterations() {
    ccusage_test_support::install_pricing_snapshots();
    let wrapped = r#"{"type":"progress","sessionId":"session-a","data":{"type":"agent_progress","message":{"type":"assistant","timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"claude-opus-5","input_tokens":10,"output_tokens":2}]}}}}}"#;
    let mut wrapper: serde_json::Value = serde_json::from_str(wrapped).unwrap();
    let mut twin = wrapper["data"]["message"].clone();
    twin["sessionId"] = serde_json::json!("session-a");
    twin["isSidechain"] = serde_json::json!(true);
    wrapper["message"] = serde_json::json!({"usage":{"iterations":[{"type":"advisor_message","model":"claude-opus-5","input_tokens":999,"output_tokens":9}]}});
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": wrapper.to_string(),
        "projects/p/s/subagents/agent-a.jsonl": twin.to_string(),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        for mode in [CostMode::Auto, CostMode::Calculate, CostMode::Display] {
            let shared = SharedArgs {
                offline: true,
                mode,
                ..display_shared_in_utc()
            };
            let entries = load_entries(&shared, None).unwrap();
            assert_eq!(entries.len(), 2, "{mode:?}");
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.usage.input_tokens)
                    .sum::<u64>(),
                110
            );
            let daily = load_daily_summaries(&shared, None, false).unwrap();
            assert_eq!(daily.len(), 1);
            assert_eq!(daily[0].input_tokens, 110);
            assert_eq!(daily[0].models_used.len(), 2);
            assert!(
                (daily[0].total_cost - entries.iter().map(|entry| entry.cost).sum::<f64>()).abs()
                    < 1e-12
            );
        }
    })
}

#[test]
fn malformed_wrapped_advisors_keep_parent_and_valid_siblings() {
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": r#"{"type":"progress","sessionId":"session-a","data":{"type":"agent_progress","message":{"type":"assistant","timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"bad","input_tokens":"invalid"},{"type":"advisor_message","model":"good","input_tokens":10,"output_tokens":2},{"type":"advisor_message","model":"null","input_tokens":5,"cache_read_input_tokens":null}]}}}}}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let shared = display_shared_in_utc();
        let diagnostics = crate::Diagnostics::new("claude");
        let entries =
            crate::load_entries_since_with_diagnostics(&shared, None, &diagnostics).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.usage.input_tokens)
                .sum::<u64>(),
            110
        );
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        let daily_diagnostics = crate::Diagnostics::new("claude");
        let daily = crate::load_daily_summaries_with_detection_progress(
            &shared,
            None,
            None,
            ccusage_adapter_common::source::ProgressOwner::Caller,
            &daily_diagnostics,
            || true,
        )
        .unwrap();
        assert_eq!(daily.summaries[0].input_tokens, 110);
        assert_eq!(
            daily_diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
            1
        );
    })
}
