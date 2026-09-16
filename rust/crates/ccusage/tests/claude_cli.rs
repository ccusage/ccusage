use ccusage_test_support::Fixture;

#[test]
fn session_id_deduplicates_repeated_message_usage() {
    let fixture = Fixture::new();
    let messages = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:01.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:02.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
    ];
    let _ = fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        messages.join("\n"),
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("CLAUDE_CONFIG_DIR", fixture.root())
        .args([
            "session",
            "--id",
            "session-a",
            "--json",
            "--mode",
            "display",
        ])
        .output()
        .expect("failed to run ccusage");

    assert!(
        output.status.success(),
        "ccusage session --id failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["totalCost"], 1.25);
    assert_eq!(json["totalTokens"], 12);
    assert_eq!(json["entries"].as_array().unwrap().len(), 1);
}
