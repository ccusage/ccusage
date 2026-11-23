use super::*;
use crate::cli::{CostMode, SharedArgs};
use ccusage_adapter_common::source::SourceOptions;
use ccusage_core::PricingMap;
use ccusage_test_support::Fixture;
use serde_json::json;
use std::path::Path;

fn load_default(
    shared: &SharedArgs,
    path: PathBuf,
    pricing: Option<&PricingMap>,
) -> Vec<LoadedEntry> {
    ccusage_test_support::install_pricing_snapshots();
    ccusage_adapter_common::testing::load_entries(
        &crate::SOURCE,
        shared,
        SourceOptions {
            pi_path: path.to_str(),
            ..SourceOptions::default()
        },
        pricing,
    )
    .unwrap()
    .0
}

fn load_store(shared: &SharedArgs, paths: Vec<PathBuf>) -> Vec<LoadedEntry> {
    ccusage_test_support::install_pricing_snapshots();
    ccusage_adapter_common::testing::load_entries(
        &crate::NamedStore::new("omp", paths),
        shared,
        SourceOptions::default(),
        None,
    )
    .unwrap()
    .0
}

#[test]
fn default_store_uses_its_root_for_project_names() {
    let fixture = ccusage_test_support::fs_fixture!({});
    fixture.write_file(
        "sessions/outer/pi-data/project-a/agent_session-a.jsonl",
        usage_line("2026-01-02T00:00:00.000Z", 10, 1, 0, 0),
    );
    let entries = load_default(
        &SharedArgs::default(),
        fixture.path("sessions/outer/pi-data"),
        None,
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(&*entries[0].project, "project-a");
}

#[test]
fn tied_timestamps_are_ordered_by_usage_identity() {
    let fixture = ccusage_test_support::fs_fixture!({});
    let timestamp = "2026-01-02T00:00:00.000Z";
    fixture.write_file(
        "data/project/agent_session.jsonl",
        [
            usage_line(timestamp, 20, 1, 0, 0),
            usage_line(timestamp, 10, 1, 0, 0),
        ]
        .join("\n"),
    );
    let entries = load_default(&SharedArgs::default(), fixture.path("data"), None);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.usage.input_tokens)
            .collect::<Vec<_>>(),
        [10, 20],
    );
}

fn session_line(id: &str, timestamp: &str, parent: Option<&Path>) -> String {
    let mut line = json!({
        "type": "session",
        "id": id,
        "timestamp": timestamp,
    });
    if let Some(parent) = parent {
        line["parentSession"] = json!(parent.to_string_lossy().to_string());
    }
    line.to_string()
}

fn usage_line_with_model_and_total(
    timestamp: &str,
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    total_tokens: u64,
) -> String {
    json!({
        "type": "message",
        "timestamp": timestamp,
        "message": {
            "role": "assistant",
            "model": model,
            "usage": {
                "input": input,
                "output": output,
                "cacheRead": cache_read,
                "cacheWrite": cache_write,
                "totalTokens": total_tokens,
            },
        },
    })
    .to_string()
}

fn usage_line(
    timestamp: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> String {
    usage_line_with_model_and_total(
        timestamp,
        "gpt-5",
        input,
        output,
        cache_read,
        cache_write,
        input + output + cache_read + cache_write,
    )
}

fn linked_usage_line(id: &str, parent_id: Option<&str>, timestamp: &str, input: u64) -> String {
    let mut line = json!({
        "type": "message",
        "id": id,
        "timestamp": timestamp,
        "message": {
            "role": "assistant",
            "model": "gpt-5",
            "usage": {
                "input": input,
                "output": 10,
                "cacheRead": 20,
                "cacheWrite": 3,
                "totalTokens": input + 33,
            },
        },
    });
    line["parentId"] = parent_id.map_or(serde_json::Value::Null, |parent_id| json!(parent_id));
    line.to_string()
}

fn usage_line_with_display_cost(timestamp: &str, display_cost: f64) -> String {
    json!({
        "type": "message",
        "timestamp": timestamp,
        "message": {
            "role": "assistant",
            "model": "gpt-5",
            "usage": {
                "input": 10,
                "output": 20,
                "cacheRead": 30,
                "cacheWrite": 40,
                "totalTokens": 100,
                "cost": {"total": display_cost},
            },
        },
    })
    .to_string()
}

#[test]
fn skips_replayed_parent_prefix_but_keeps_child_usage() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
            usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
        ]
        .join("\n"),
    );
    let _child_a = fixture.write_file(
        "sessions/project-a/child-a.jsonl",
        [
            session_line("child-a", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
            usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
        ]
        .join("\n"),
    );
    let _child_b = fixture.write_file(
        "sessions/project-a/child-b.jsonl",
        [
            session_line("child-b", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 999),
            usage_line("2026-01-03T01:00:00.000Z", 70, 7, 8, 2),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 6, "single_thread={single_thread}");
        let child_a_entries = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child-a")
            .collect::<Vec<_>>();
        assert_eq!(child_a_entries.len(), 1, "single_thread={single_thread}");
        assert_eq!(
            child_a_entries[0].usage.input_tokens, 50,
            "single_thread={single_thread}"
        );
        let child_b_entries = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child-b")
            .collect::<Vec<_>>();
        assert_eq!(child_b_entries.len(), 2, "single_thread={single_thread}");
        assert_eq!(
            child_b_entries[0].usage.cache_creation_input_tokens, 999,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn skips_the_copied_active_parent_branch_after_an_abandoned_sibling() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            linked_usage_line("a", None, "2026-01-02T10:00:00.000Z", 100),
            linked_usage_line("x", Some("a"), "2026-01-02T11:00:00.000Z", 200),
            linked_usage_line("y", Some("a"), "2026-01-02T12:00:00.000Z", 300),
            linked_usage_line("z", Some("y"), "2026-01-02T13:00:00.000Z", 400),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            linked_usage_line("copy-a", None, "2026-01-02T10:00:00.000Z", 100),
            linked_usage_line("copy-y", Some("copy-a"), "2026-01-02T12:00:00.000Z", 300),
            linked_usage_line("copy-z", Some("copy-y"), "2026-01-02T13:00:00.000Z", 400),
            linked_usage_line(
                "child-only",
                Some("copy-z"),
                "2026-01-03T01:00:00.000Z",
                500,
            ),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 5, "single_thread={single_thread}");
        assert!(entries.iter().any(|entry| {
            entry.session_id.as_ref() == "root" && entry.usage.input_tokens == 200
        }));
        let child_entries = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child")
            .collect::<Vec<_>>();
        assert_eq!(child_entries.len(), 1, "single_thread={single_thread}");
        assert_eq!(
            child_entries[0].usage.input_tokens, 500,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn skips_copied_branch_when_parent_has_multiple_disconnected_roots() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            linked_usage_line("unrelated-root", None, "2026-01-02T09:00:00.000Z", 50),
            linked_usage_line("active-root", None, "2026-01-02T10:00:00.000Z", 100),
            linked_usage_line(
                "active-leaf",
                Some("active-root"),
                "2026-01-02T11:00:00.000Z",
                200,
            ),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            linked_usage_line("copy-root", None, "2026-01-02T10:00:00.000Z", 100),
            linked_usage_line(
                "copy-leaf",
                Some("copy-root"),
                "2026-01-02T11:00:00.000Z",
                200,
            ),
            linked_usage_line(
                "child-only",
                Some("copy-leaf"),
                "2026-01-03T01:00:00.000Z",
                300,
            ),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 4, "single_thread={single_thread}");
        assert!(entries.iter().any(|entry| {
            entry.session_id.as_ref() == "root" && entry.usage.input_tokens == 50
        }));
        let child_entries = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child")
            .collect::<Vec<_>>();
        assert_eq!(child_entries.len(), 1, "single_thread={single_thread}");
        assert_eq!(
            child_entries[0].usage.input_tokens, 300,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn fails_open_for_missing_malformed_and_unrelated_same_token_sessions() {
    let fixture = Fixture::new();
    let missing_parent = fixture.path("sessions/project-a/missing.jsonl");
    let malformed_parent = fixture.path("sessions/project-a/malformed-parent.jsonl");
    let outside_parent = fixture.write_file(
        "outside/root.jsonl",
        [
            session_line("outside", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/missing-child.jsonl",
        [
            session_line(
                "missing-child",
                "2026-01-03T00:00:00.000Z",
                Some(&missing_parent),
            ),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/outside-parent-child.jsonl",
        [
            session_line(
                "outside-parent-child",
                "2026-01-03T00:00:00.000Z",
                Some(&outside_parent),
            ),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/malformed-parent.jsonl",
        [
            "not json".to_string(),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/malformed-parent-child.jsonl",
        [
            session_line(
                "malformed-parent-child",
                "2026-01-03T00:00:00.000Z",
                Some(&malformed_parent),
            ),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/malformed-parent-grandchild.jsonl",
        [
            session_line(
                "malformed-parent-grandchild",
                "2026-01-04T00:00:00.000Z",
                Some(&fixture.path("sessions/project-a/malformed-parent-child.jsonl")),
            ),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/malformed-child.jsonl",
        [
            "not json".to_string(),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/regular-a.jsonl",
        usage_line("2026-01-02T11:00:00.000Z", 100, 10, 20, 3),
    );
    fixture.write_file(
        "sessions/project-a/regular-b.jsonl",
        usage_line("2026-01-02T11:00:00.000Z", 100, 10, 20, 3),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 8, "single_thread={single_thread}");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "missing-child")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "outside-parent-child")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "regular-a")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "regular-b")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "malformed-parent-grandchild")
                .count(),
            1,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn fails_open_for_self_referential_and_cyclic_sessions() {
    let fixture = Fixture::new();
    let self_path = fixture.path("sessions/project-a/self.jsonl");
    let cycle_a = fixture.path("sessions/project-a/cycle-a.jsonl");
    let cycle_b = fixture.path("sessions/project-a/cycle-b.jsonl");
    fixture.write_file(
        "sessions/project-a/self.jsonl",
        [
            session_line("self", "2026-01-03T00:00:00.000Z", Some(&self_path)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/cycle-a.jsonl",
        [
            session_line("cycle-a", "2026-01-03T00:00:00.000Z", Some(&cycle_b)),
            usage_line("2026-01-02T10:00:00.000Z", 200, 20, 30, 4),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/cycle-b.jsonl",
        [
            session_line("cycle-b", "2026-01-03T00:00:00.000Z", Some(&cycle_a)),
            usage_line("2026-01-02T10:00:00.000Z", 200, 20, 30, 4),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 3, "single_thread={single_thread}");
    }
}

#[test]
fn uses_raw_parent_stream_for_nested_forks() {
    let fixture = Fixture::new();
    let root = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
        ]
        .join("\n"),
    );
    let child = fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&root)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
            usage_line("2026-01-03T01:00:00.000Z", 300, 30, 40, 5),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/grandchild.jsonl",
        [
            session_line("grandchild", "2026-01-04T00:00:00.000Z", Some(&child)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
            usage_line("2026-01-03T01:00:00.000Z", 300, 30, 40, 5),
            usage_line("2026-01-04T01:00:00.000Z", 400, 40, 50, 6),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 4, "single_thread={single_thread}");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "grandchild")
                .count(),
            1,
            "single_thread={single_thread}"
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.session_id.as_ref() == "grandchild")
                .unwrap()
                .usage
                .input_tokens,
            400,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn reads_only_the_header_of_a_session_last_changed_before_since() {
    let fixture = Fixture::new();
    let root = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2098-12-01T00:00:00.000Z", None),
            usage_line("2098-12-01T01:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    let child = fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2099-01-09T12:00:00.000Z", Some(&root)),
            usage_line("2098-12-01T01:00:00.000Z", 100, 10, 20, 3),
            usage_line("2099-01-09T12:00:00.000Z", 300, 30, 40, 5),
        ]
        .join("\n"),
    );
    let grandchild = fixture.write_file(
        "sessions/project-a/grandchild.jsonl",
        [
            session_line("grandchild", "2099-01-11T00:00:00.000Z", Some(&child)),
            usage_line("2098-12-01T01:00:00.000Z", 100, 10, 20, 3),
            usage_line("2099-01-09T12:00:00.000Z", 300, 30, 40, 5),
            usage_line("2099-01-11T01:00:00.000Z", 400, 40, 50, 6),
        ]
        .join("\n"),
    );
    for (file, modified) in [
        (&root, "2099-01-05T00:00:00Z"),
        (&child, "2099-01-09T12:00:00Z"),
        (&grandchild, "2099-01-11T02:00:00Z"),
    ] {
        ccusage_test_support::set_modified(file, ccusage_test_support::instant(modified));
    }
    let shared = SharedArgs {
        mode: CostMode::Display,
        since: Some("20990110".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        clock: ccusage_core::cli::ReportClock::Fixed(
            ccusage_test_support::instant("2099-12-31T00:00:00Z").into(),
        ),
        ..SharedArgs::default()
    };

    let entries = load_default(&shared, fixture.path("sessions"), None);

    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.session_id.as_ref(), entry.usage.input_tokens))
            .collect::<Vec<_>>(),
        [("child", 100), ("child", 300), ("grandchild", 400)]
    );
}

#[test]
fn compares_every_effective_usage_field_before_suppressing_replay() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_model_and_total(
                "2026-01-02T10:00:00.000Z",
                "gpt-5",
                10,
                20,
                30,
                40,
                100,
            ),
        ]
        .join("\n"),
    );
    let variants = [
        ("input", "gpt-5", 11, 20, 30, 40, 101),
        ("output", "gpt-5", 10, 21, 30, 40, 101),
        ("cache-read", "gpt-5", 10, 20, 31, 40, 101),
        ("cache-write", "gpt-5", 10, 20, 30, 41, 101),
        ("model", "gpt-5-mini", 10, 20, 30, 40, 100),
        ("total-fallback", "gpt-5", 10, 20, 30, 40, 110),
    ];
    for (name, model, input, output, cache_read, cache_write, total_tokens) in variants {
        fixture.write_file(
            format!("sessions/project-a/child-{name}.jsonl"),
            [
                session_line(name, "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_model_and_total(
                    "2026-01-02T10:00:00.000Z",
                    model,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    total_tokens,
                ),
                usage_line("2026-01-03T01:00:00.000Z", 1, 2, 3, 4),
            ]
            .join("\n"),
        );
    }

    let shared = SharedArgs {
        mode: CostMode::Display,
        single_thread: false,
        ..SharedArgs::default()
    };
    let entries = load_default(&shared, fixture.path("sessions"), None);

    assert_eq!(entries.len(), 1 + variants.len() * 2);
    for (name, ..) in variants {
        let session_id = format!("child-{name}");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == session_id)
                .count(),
            2,
            "variant={name}"
        );
    }
}

#[test]
fn keeps_child_usage_when_display_cost_differs() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 1.0),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 2.0),
        ]
        .join("\n"),
    );

    for mode in [CostMode::Display, CostMode::Auto] {
        let shared = SharedArgs {
            mode,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 2, "mode={mode:?}");
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.session_id.as_ref() == "child")
                .unwrap()
                .cost,
            2.0,
            "mode={mode:?}"
        );
    }
}

#[test]
fn ignores_stored_display_cost_when_calculating_replay_identity() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 1.0),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 2.0),
        ]
        .join("\n"),
    );
    let shared = SharedArgs {
        mode: CostMode::Calculate,
        ..SharedArgs::default()
    };
    let entries = load_default(&shared, fixture.path("sessions"), None);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "root");
}

#[test]
fn treats_signed_zero_display_costs_as_equal_replay_identity() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 0.0),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", -0.0),
        ]
        .join("\n"),
    );

    for mode in [CostMode::Display, CostMode::Auto] {
        let shared = SharedArgs {
            mode,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 1, "mode={mode:?}");
        assert_eq!(entries[0].session_id.as_ref(), "root", "mode={mode:?}");
    }
}

#[test]
fn treats_missing_and_zero_display_costs_as_equal_replay_identity() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_model_and_total(
                "2026-01-02T10:00:00.000Z",
                "gpt-5",
                10,
                20,
                30,
                40,
                100,
            ),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 0.0),
        ]
        .join("\n"),
    );
    let shared = SharedArgs {
        mode: CostMode::Display,
        ..SharedArgs::default()
    };
    let entries = load_default(&shared, fixture.path("sessions"), None);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "root");
}

#[test]
fn dedupes_replay_when_raw_total_underreports_billable_usage() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line_with_model_and_total(
                "2026-01-02T10:00:00.000Z",
                "gpt-5",
                10,
                20,
                30,
                40,
                100,
            ),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line_with_model_and_total(
                "2026-01-02T10:00:00.000Z",
                "gpt-5",
                10,
                20,
                30,
                40,
                99,
            ),
            usage_line("2026-01-03T01:00:00.000Z", 1, 2, 3, 4),
        ]
        .join("\n"),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 2, "single_thread={single_thread}");
        let child_entries = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child")
            .collect::<Vec<_>>();
        assert_eq!(child_entries.len(), 1, "single_thread={single_thread}");
        assert_eq!(
            child_entries[0].usage.input_tokens, 1,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn matches_parent_files_across_multiple_paths_of_one_named_store() {
    let fixture = Fixture::new();
    let first_store = fixture.create_dir_all("first");
    let second_store = fixture.create_dir_all("second");
    let parent = fixture.write_file(
        "first/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "second/project-a/child.jsonl",
        [
            session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            usage_line("2026-01-03T01:00:00.000Z", 50, 5, 6, 1),
        ]
        .join("\n"),
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        single_thread: false,
        ..SharedArgs::default()
    };
    let entries = load_store(&shared, vec![first_store, second_store]);

    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "child")
            .count(),
        1
    );
    assert_eq!(entries[1].model.as_deref(), Some("[omp] gpt-5"));
}

#[test]
fn skips_subagent_artifact_transcripts_but_keeps_fresh_context_child_sessions() {
    let fixture = Fixture::new();
    fixture.write_file(
        "sessions/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "sessions/project-a/root/subagent-artifacts/run1_agent_0_transcript.jsonl",
        usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
    );
    fixture.write_file(
        "sessions/project-a/subagent-artifacts/other_transcript.jsonl",
        usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
    );
    fixture.write_file(
        "sessions/project-a/root/run-abc/run-0/session.jsonl",
        usage_line("2026-01-03T01:00:00.000Z", 50, 5, 6, 1),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread,
            ..SharedArgs::default()
        };
        let entries = load_default(&shared, fixture.path("sessions"), None);

        assert_eq!(entries.len(), 2, "single_thread={single_thread}");
        assert!(
            entries.iter().any(|entry| entry.usage.input_tokens == 100),
            "single_thread={single_thread}"
        );
        assert!(
            entries.iter().any(|entry| entry.usage.input_tokens == 50),
            "single_thread={single_thread}"
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry.session_id.as_ref().contains("transcript")),
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn skips_subagent_artifact_transcripts_for_named_stores() {
    let fixture = Fixture::new();
    let store = fixture.create_dir_all("store");
    fixture.write_file(
        "store/project-a/root.jsonl",
        [
            session_line("root", "2026-01-01T00:00:00.000Z", None),
            usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
        ]
        .join("\n"),
    );
    fixture.write_file(
        "store/project-a/root/subagent-artifacts/run1_agent_0_transcript.jsonl",
        usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        single_thread: false,
        ..SharedArgs::default()
    };
    let entries = load_store(&shared, vec![store]);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.input_tokens, 100);
}

#[test]
fn detects_subagent_artifact_path_segments() {
    assert!(is_subagent_artifact_transcript(Path::new(
        "sessions/project-a/root/subagent-artifacts/run1_agent_0_transcript.jsonl"
    )));
    assert!(!is_subagent_artifact_transcript(Path::new(
        "sessions/project-a/root/run-abc/run-0/session.jsonl"
    )));
    assert!(!is_subagent_artifact_transcript(Path::new(
        "sessions/project-a/root.jsonl"
    )));
}
