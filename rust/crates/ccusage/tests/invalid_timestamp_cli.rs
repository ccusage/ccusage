#![cfg(test)]

use ccusage_test_support::fs_fixture;
use serde_json::Value;

mod support;

#[test]
fn undated_usage_is_dropped_with_one_warning_line_on_stderr() {
    let fixture = fs_fixture!({
        "gemini/chats/session.jsonl": concat!(
            r#"{"type":"gemini","timestamp":"2026-02-24T02:00:00.000Z","model":"gemini-2.5-pro","tokens":{"input":100,"output":20}}"#,
            "\n",
            r#"{"type":"gemini","model":"gemini-2.5-pro","tokens":{"input":7777,"output":7777}}"#,
            "\n",
            r#"{"type":"gemini","timestamp":"yesterday","model":"gemini-2.5-pro","tokens":{"input":7777,"output":7777}}"#,
            "\n",
        ),
    });

    let output = support::hermetic_command(fixture.root())
        .env("GEMINI_DATA_DIR", fixture.path("gemini"))
        .args([
            "gemini",
            "daily",
            "--json",
            "--offline",
            "--no-color",
            "--timezone",
            "UTC",
        ])
        .output()
        .expect("failed to run ccusage");

    let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
    assert!(output.status.success(), "ccusage failed: {stderr}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["totals"]["totalTokens"], 120);
    assert_eq!(
        stderr.lines().collect::<Vec<_>>(),
        ["WARN  gemini: skipped 2 records in 1 input (invalid timestamp)"]
    );
}
