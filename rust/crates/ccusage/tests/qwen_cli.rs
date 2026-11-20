#![cfg(test)]

use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::Value;

mod support;

fn qwen_fixture() -> Fixture {
    fs_fixture!({
        "qwen/projects/myProject/chats/until-day.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:00:00.000Z","sessionId":"until-day","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50}}"#,
        "qwen/projects/myProject/chats/after-day.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T18:00:00.000Z","sessionId":"after-day","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
    })
}

const WINDOW_ARGS: [&str; 12] = [
    "qwen",
    "session",
    "--since",
    "20260223",
    "--until",
    "20260223",
    "--timezone",
    "America/Los_Angeles",
    "--mode",
    "display",
    "--offline",
    "--no-color",
];

#[test]
fn qwen_session_window_keeps_sessions_last_active_on_the_until_day() {
    let fixture = qwen_fixture();

    let json_args = WINDOW_ARGS
        .iter()
        .copied()
        .chain(["--json"])
        .collect::<Vec<_>>();
    let report: Value = serde_json::from_str(&run_cli(&fixture, &json_args)).unwrap();

    assert_eq!(report["sessions"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["sessions"][0]["sessionId"], "until-day");
    assert_eq!(
        report["sessions"][0]["lastActivity"],
        "2026-02-24T02:00:00.000Z"
    );
    assert_eq!(report["totals"]["totalTokens"], 150);

    insta::assert_snapshot!(
        "qwen_cli_session_window_table",
        run_cli(&fixture, &WINDOW_ARGS)
    );
}

#[test]
fn qwen_session_includes_archived_chat_files() {
    let fixture = fs_fixture!({
        "qwen/projects/myProject/chats/live.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T02:00:00.000Z","sessionId":"live","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50}}"#,
        "qwen/projects/myProject/chats/archive/archived.jsonl": r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T03:00:00.000Z","sessionId":"archived","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
    });

    let report: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "qwen",
            "session",
            "--since",
            "20260224",
            "--until",
            "20260224",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--no-color",
            "--json",
        ],
    ))
    .unwrap();

    let sessions = report["sessions"].as_array().expect("sessions");
    let ids: Vec<&str> = sessions
        .iter()
        .map(|session| session["sessionId"].as_str().expect("sessionId"))
        .collect();
    assert!(ids.contains(&"live"));
    assert!(ids.contains(&"archived"));
    assert_eq!(report["totals"]["totalTokens"], 165);
}

fn run_cli(fixture: &Fixture, args: &[&str]) -> String {
    let output = support::hermetic_command(fixture.root())
        .env("QWEN_DATA_DIR", fixture.path("qwen"))
        .env("COLUMNS", "240")
        .args(args)
        .output()
        .expect("failed to run ccusage");
    assert!(
        output.status.success(),
        "ccusage {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8")
}
