use super::*;

const PARENT: &str = "019a0000-0000-7000-8000-000000000001";
const CHILD: &str = "019a0000-0000-7000-8000-000000000002";
const MISSING_PARENT_WARNING: &str = "WARN  codex: could not find the parent rollout for 1 forked session. Replayed usage may be counted twice.";

fn user_fork_meta(timestamp: &str) -> String {
    json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {"id": CHILD, "forked_from_id": PARENT, "timestamp": timestamp},
    })
    .to_string()
}

fn spawned_meta(timestamp: &str) -> String {
    json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {
            "id": CHILD,
            "timestamp": timestamp,
            "source": {"subagent": {"thread_spawn": {"parent_thread_id": PARENT, "depth": 1}}},
        },
    })
    .to_string()
}

fn usage_record(timestamp: &str, thread_id: &str, response_id: &str, input_tokens: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "token_usage_record",
        "payload": {
            "thread_id": thread_id,
            "response_id": response_id,
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": 1,
                "total_tokens": input_tokens + 1,
            },
        },
    })
    .to_string()
}

fn settings_applied(timestamp: &str, thread_id: Option<&str>) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "thread_settings_applied",
            "thread_id": thread_id,
            "thread_settings": {"service_tier": "default"},
        },
    })
    .to_string()
}

struct ForkLoad {
    child_input: Vec<u64>,
    warnings: Vec<String>,
}

fn load_fork(parent: Option<&[String]>, child: &[String]) -> Vec<ForkLoad> {
    ccusage_test_support::install_pricing_snapshots();
    ccusage_test_support::Environment::with_var("LOG_LEVEL", "1").run(|| {
        let mut loads = Vec::new();
        for (parent_name, child_name) in [
            ("01-parent.jsonl", "02-child.jsonl"),
            ("02-parent.jsonl", "01-child.jsonl"),
        ] {
            let fixture = ccusage_test_support::Fixture::new();
            if let Some(parent) = parent {
                fixture.write_file(parent_name, parent.join("\n"));
            }
            fixture.write_file(child_name, child.join("\n"));
            let child_session = child_name.trim_end_matches(".jsonl");
            let child_input = crate::test_support::rollout_events(fixture.root())
                .into_iter()
                .filter(|event| event.session_id == child_session)
                .map(|event| event.input_tokens)
                .collect();
            let sources = [CodexUsageSource::new(
                fixture.root().to_path_buf(),
                fixture.root().to_path_buf(),
            )];
            let diagnostics = Diagnostics::new("codex");
            session_input_tokens(
                &sources,
                &SharedArgs {
                    timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                    ..SharedArgs::default()
                },
                &diagnostics,
            );
            loads.push(ForkLoad {
                child_input,
                warnings: diagnostics.warnings(&SharedArgs::default()),
            });
        }
        loads
    })
}

fn assert_child_usage(loads: &[ForkLoad], expected: &[u64], warnings: &[&str]) {
    for (order, load) in loads.iter().enumerate() {
        assert_eq!(load.child_input, expected, "discovery order {order}");
        assert_eq!(load.warnings, warnings, "discovery order {order}");
    }
}

fn parent_rollout() -> Vec<String> {
    vec![
        replay_metadata("2026-11-01T08:00:00.000Z", PARENT, None),
        replay_token_count("2026-11-01T08:01:00.000Z", 100),
        replay_token_count("2026-11-01T08:02:00.000Z", 200),
        replay_token_count("2026-11-01T08:03:00.000Z", 300),
    ]
}

#[test]
fn counts_only_own_usage_after_the_copied_history_of_a_missing_parent() {
    let child = [
        user_fork_meta("2026-11-01T09:00:00.000Z"),
        usage_record("2026-11-01T09:00:00.010Z", PARENT, "resp_parent_1", 100),
        replay_token_count("2026-11-01T09:00:00.010Z", 100),
        usage_record("2026-11-01T09:00:00.020Z", PARENT, "resp_parent_2", 200),
        replay_token_count("2026-11-01T09:00:00.020Z", 200),
        settings_applied("2026-11-01T09:00:00.040Z", Some(CHILD)),
        usage_record("2026-11-01T09:00:00.500Z", CHILD, "resp_child_1", 400),
        replay_token_count("2026-11-01T09:00:00.500Z", 400),
        usage_record("2026-11-01T09:05:00.000Z", CHILD, "resp_child_2", 500),
        replay_token_count("2026-11-01T09:05:00.000Z", 500),
    ];

    assert_child_usage(&load_fork(None, &child), &[400, 500], &[]);
}

#[test]
fn counts_a_dense_first_request_pair_of_a_spawned_child_with_own_records() {
    let child = [
        spawned_meta("2026-11-01T09:00:00.000Z"),
        replay_token_count("2026-11-01T09:00:00.200Z", 400),
        replay_token_count("2026-11-01T09:00:00.400Z", 500),
        usage_record("2026-11-01T09:05:00.000Z", CHILD, "resp_child_3", 600),
        replay_token_count("2026-11-01T09:05:00.000Z", 600),
    ];

    assert_child_usage(
        &load_fork(None, &child),
        &[400, 500, 600],
        &[MISSING_PARENT_WARNING],
    );
    assert_child_usage(
        &load_fork(Some(&parent_rollout()), &child),
        &[400, 500, 600],
        &[],
    );
}

#[test]
fn counts_a_spawned_child_usage_that_precedes_its_first_record() {
    let child = [
        spawned_meta("2026-11-01T09:00:00.000Z"),
        settings_applied("2026-11-01T09:00:00.000Z", Some(CHILD)),
        replay_token_count("2026-11-01T09:00:05.000Z", 400),
        usage_record("2026-11-01T09:05:00.000Z", CHILD, "resp_child_2", 500),
        replay_token_count("2026-11-01T09:05:00.000Z", 500),
    ];

    assert_child_usage(&load_fork(None, &child), &[400, 500], &[]);
}

#[test]
fn skips_a_leading_run_that_replays_a_suffix_of_the_parent() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.000Z", 200),
        replay_token_count("2026-11-01T09:00:03.000Z", 300),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(&load_fork(Some(&parent_rollout()), &child), &[400], &[]);
}

#[test]
fn counts_dense_usage_at_the_fork_instant_that_the_parent_never_had() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.100Z", 400),
        replay_token_count("2026-11-01T09:00:00.300Z", 500),
        replay_token_count("2026-11-01T09:05:00.000Z", 600),
    ];

    assert_child_usage(
        &load_fork(Some(&parent_rollout()), &child),
        &[400, 500, 600],
        &[],
    );
}

#[test]
fn counts_all_usage_of_a_legacy_fork_whose_parent_is_missing_and_warns() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.010Z", 100),
        replay_token_count("2026-11-01T09:00:00.020Z", 200),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(
        &load_fork(None, &child),
        &[100, 200, 400],
        &[MISSING_PARENT_WARNING],
    );
}

#[test]
fn keeps_the_settings_boundary_when_a_spawned_child_logs_an_untagged_record() {
    let child = [
        spawned_meta("2026-11-01T09:00:00.000Z"),
        settings_applied("2026-11-01T09:00:00.000Z", Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.010Z", 100),
        replay_token_count("2026-11-01T09:00:00.020Z", 200),
        settings_applied("2026-11-01T09:00:00.040Z", Some(CHILD)),
        json!({
            "timestamp": "2026-11-01T09:00:05.000Z",
            "type": "token_usage_record",
            "payload": {
                "response_id": "resp_child_1",
                "usage": {"input_tokens": 400, "output_tokens": 1, "total_tokens": 401},
            },
        })
        .to_string(),
        replay_token_count("2026-11-01T09:00:05.000Z", 400),
    ];

    assert_child_usage(&load_fork(None, &child), &[400], &[]);
}

#[test]
fn keeps_own_turns_a_legacy_fork_wrote_before_an_upgraded_codex_resumed_it() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        settings_applied("2026-11-01T09:00:00.000Z", None),
        replay_token_count("2026-11-01T09:01:00.000Z", 700),
        replay_token_count("2026-11-01T09:02:00.000Z", 800),
        usage_record("2026-11-03T09:00:00.000Z", CHILD, "resp_child_9", 900),
        replay_token_count("2026-11-03T09:00:00.000Z", 900),
    ];

    assert_child_usage(
        &load_fork(Some(&parent_rollout()), &child),
        &[700, 800, 900],
        &[],
    );
    assert_child_usage(
        &load_fork(None, &child),
        &[700, 800, 900],
        &[MISSING_PARENT_WARNING],
    );
}

#[test]
fn matches_the_parent_before_an_own_marker_that_follows_threadless_copies() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        settings_applied("2026-11-01T09:00:00.000Z", None),
        replay_token_count("2026-11-01T09:00:00.000Z", 100),
        replay_token_count("2026-11-01T09:00:00.000Z", 200),
        settings_applied("2026-11-01T09:00:00.010Z", Some(CHILD)),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(&load_fork(Some(&parent_rollout()), &child), &[400], &[]);
}

#[test]
fn counts_and_warns_when_a_fork_names_no_child_id() {
    let child = [
        json!({
            "timestamp": "2026-11-01T09:00:00.000Z",
            "type": "session_meta",
            "payload": {"forked_from_id": PARENT},
        })
        .to_string(),
        replay_token_count("2026-11-01T09:00:01.000Z", 100),
        settings_applied("2026-11-01T09:00:02.000Z", Some(CHILD)),
        replay_token_count("2026-11-01T09:00:03.000Z", 200),
    ];

    assert_child_usage(
        &load_fork(None, &child),
        &[100, 200],
        &[MISSING_PARENT_WARNING],
    );
}

#[test]
fn treats_a_blank_thread_id_as_absent() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        settings_applied("2026-11-01T09:00:00.000Z", Some("")),
        replay_token_count("2026-11-01T09:00:01.000Z", 100),
    ];

    assert_child_usage(&load_fork(None, &child), &[100], &[MISSING_PARENT_WARNING]);
}

#[test]
fn needs_an_own_boundary_before_dropping_usage_after_a_foreign_marker() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        settings_applied("2026-11-01T09:00:00.000Z", Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.010Z", 200),
        replay_token_count("2026-11-01T09:00:00.020Z", 300),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(
        &load_fork(None, &child),
        &[200, 300, 400],
        &[MISSING_PARENT_WARNING],
    );
    assert_child_usage(&load_fork(Some(&parent_rollout()), &child), &[400], &[]);
}

#[test]
fn keeps_usage_logged_before_the_first_foreign_marker() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.000Z", 700),
        settings_applied("2026-11-01T09:00:01.000Z", Some(PARENT)),
        replay_token_count("2026-11-01T09:00:01.010Z", 100),
        replay_token_count("2026-11-01T09:00:01.020Z", 200),
        settings_applied("2026-11-01T09:00:01.040Z", Some(CHILD)),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(
        &load_fork(None, &child),
        &[700, 400],
        &[MISSING_PARENT_WARNING],
    );
    assert_child_usage(
        &load_fork(Some(&parent_rollout()), &child),
        &[700, 400],
        &[],
    );
}

#[test]
fn skips_a_parent_suffix_that_a_copied_marker_splits() {
    let child = [
        replay_metadata("2026-11-01T09:00:00.000Z", CHILD, Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.010Z", 200),
        settings_applied("2026-11-01T09:00:00.020Z", Some(PARENT)),
        replay_token_count("2026-11-01T09:00:00.030Z", 300),
        settings_applied("2026-11-01T09:00:00.040Z", Some(CHILD)),
        replay_token_count("2026-11-01T09:05:00.000Z", 400),
    ];

    assert_child_usage(&load_fork(Some(&parent_rollout()), &child), &[400], &[]);
}
