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

fn set_file_modified(path: &std::path::Path, unix_seconds: u64) {
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix_seconds);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn run_bounded_daily(fixture: &Fixture, extra_args: &[&str]) -> std::process::Output {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("CLAUDE_CONFIG_DIR", fixture.root())
        .env("NO_COLOR", "1")
        .args([
            "claude",
            "daily",
            "--since",
            "20260910",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
        ])
        .args(extra_args)
        .output()
        .expect("failed to run ccusage");
    assert!(
        output.status.success(),
        "ccusage claude daily failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn daily_since_totals_match_after_pruning_stale_files() {
    let fixture = Fixture::new();
    let entry = |timestamp: &str, id: &str, cost: f64| {
        format!(
            r#"{{"timestamp":"{timestamp}","sessionId":"{id}","requestId":"request-{id}","costUSD":{cost},"message":{{"id":"message-{id}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":10,"output_tokens":2}}}}}}"#
        )
    };

    // Written long before --since: holds only pre-window history.
    let stale = fixture.write_file(
        "projects/project-a/stale.jsonl",
        entry("2026-09-01T12:00:00.000Z", "stale", 4.0),
    );
    // Its mtime is inside the 24 hour margin, so it is still read even though
    // its entry was flushed after it happened.
    let margin = fixture.write_file(
        "projects/project-a/margin.jsonl",
        entry("2026-09-10T08:00:00.000Z", "margin", 2.0),
    );
    let fresh = fixture.write_file(
        "projects/project-a/fresh.jsonl",
        [
            entry("2026-09-09T23:00:00.000Z", "fresh-before", 8.0),
            entry("2026-09-11T12:00:00.000Z", "fresh", 1.0),
        ]
        .join("\n"),
    );
    // 2026-09-01T12:00:00Z, 2026-09-09T06:00:00Z, 2026-09-11T12:00:00Z
    set_file_modified(&stale, 1_788_264_000);
    set_file_modified(&margin, 1_788_933_600);
    set_file_modified(&fresh, 1_789_128_000);

    let json_output = run_bounded_daily(&fixture, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let days = json["daily"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| {
            (
                day["date"].as_str().unwrap().to_string(),
                day["totalCost"].as_f64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        vec![
            ("2026-09-10".to_string(), 2.0),
            ("2026-09-11".to_string(), 1.0)
        ]
    );
    assert_eq!(json["totals"]["totalCost"], 3.0);
    assert_eq!(json["totals"]["totalTokens"], 24);

    let table_output = run_bounded_daily(&fixture, &[]);
    let table = String::from_utf8_lossy(&table_output.stdout);
    assert!(table.contains("2026-09-10"), "{table}");
    assert!(table.contains("2026-09-11"), "{table}");
    assert!(!table.contains("2026-09-01"), "{table}");
    assert!(!table.contains("2026-09-09"), "{table}");
    assert!(table.contains("$3.00"), "{table}");
}
