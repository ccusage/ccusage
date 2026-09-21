use std::process::{Command, Output};

use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::{Value, json};

const SESSION_UUID: &str = "01a0bb8c-c7c0-7630-9d82-860b875930f0";
const SESSION_ID: &str =
    "2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0";
const SESSION_FILE: &str = "rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0";
const OTHER_UUID: &str = "01a0cc9d-d8d1-8741-ae93-971c986041a1";

#[test]
fn selects_codex_session_by_supported_id_forms() {
    let fixture = codex_fixture();

    for requested_id in [
        SESSION_ID.to_string(),
        SESSION_FILE.to_string(),
        format!("{SESSION_FILE}.jsonl"),
        SESSION_UUID.to_string(),
        format!("codex://threads/{SESSION_UUID}"),
    ] {
        let output = run_cli(
            &fixture,
            &["codex", "session", "--id", &requested_id, "--json"],
        );
        let report: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(report["sessionId"], SESSION_ID);
        assert_eq!(report["totalTokens"], 150);
        assert!(report.get("totalCost").is_some());
        assert!(report.get("costUSD").is_none());
        assert!(report.get("sessions").is_none());
        assert!(report.get("totals").is_none());
    }
}

#[test]
fn filters_codex_session_table_by_full_id() {
    let fixture = codex_fixture();

    let output = run_cli(
        &fixture,
        &["codex", "session", "--id", SESSION_ID, "--no-cost"],
    );

    assert!(output.contains(SESSION_UUID));
    assert!(!output.contains(OTHER_UUID));
    let selected_row = output
        .lines()
        .find(|line| line.contains(SESSION_UUID))
        .expect("selected session row should render");
    assert!(selected_row.contains("150"));
}

#[test]
fn reports_unknown_codex_session_id() {
    let fixture = codex_fixture();

    let output = run_cli_output(&fixture, &["codex", "session", "--id", "missing"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("No Codex session found with ID: missing")
    );
}

#[test]
fn reports_ambiguous_codex_session_uuid() {
    let fixture = fs_fixture!({
        "codex/sessions/2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-19T23:22:40.000Z", 100, 50),
        "codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-20T10:00:00.000Z", 200, 75),
    });

    let output = run_cli_output(&fixture, &["codex", "session", "--id", SESSION_UUID]);

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains(&format!(
        "Codex session ID '{SESSION_UUID}' is ambiguous and matches 2 sessions."
    )));
}

fn codex_fixture() -> Fixture {
    fs_fixture!({
        "codex/sessions/2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-19T23:22:40.000Z", 100, 50),
        "codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-01a0cc9d-d8d1-8741-ae93-971c986041a1.jsonl": usage("2026-09-20T10:00:00.000Z", 900, 100),
    })
}

fn usage(timestamp: &str, input_tokens: u64, output_tokens: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5",
                "last_token_usage": {
                    "input_tokens": input_tokens,
                    "cached_input_tokens": 10,
                    "output_tokens": output_tokens,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input_tokens + output_tokens,
                },
            },
        },
    })
    .to_string()
}

fn run_cli(fixture: &Fixture, args: &[&str]) -> String {
    let output = run_cli_output(fixture, args);
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}

fn run_cli_output(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .args(args)
        .args(["--offline", "--no-color", "--timezone", "UTC"])
        .env("CODEX_HOME", fixture.path("codex"))
        .env("HOME", fixture.path("empty-home"))
        .env("USERPROFILE", fixture.path("empty-userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("empty-xdg-config"))
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "240")
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .expect("ccusage CLI should run")
}
