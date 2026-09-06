use ccusage_test_support::Fixture;
use serde_json::Value;

#[test]
fn gjc_cli_reports_daily_monthly_and_session_json() {
    let fixture = Fixture::new();
    let sessions = fixture.path("gjc/agent/sessions/project");
    let _ = fixture.create_dir_all("gjc/agent/sessions/project");
    std::fs::write(
        sessions.join("session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"session-a\",\"timestamp\":\"2099-01-02T01:00:00.000Z\",\"cwd\":\"/workspace/project\"}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2099-01-02T01:02:03.000Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input\":100,\"output\":50,\"cacheRead\":20,\"cacheWrite\":10,\"totalTokens\":180,\"cost\":{\"total\":0.25}}}}\n"
        ),
    )
    .unwrap();

    for (kind, rows_key, period_key, period) in [
        ("daily", "daily", "date", "2099-01-02"),
        ("monthly", "monthly", "month", "2099-01"),
        ("session", "sessions", "sessionId", "session-a"),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("GJC_CODING_AGENT_DIR", fixture.path("gjc/agent"))
            .args(["gjc", kind, "--json", "--timezone", "UTC"])
            .output()
            .expect("failed to run ccusage");

        assert!(
            output.status.success(),
            "ccusage gjc {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report[rows_key][0][period_key], period);
        assert_eq!(report[rows_key][0]["inputTokens"], 100);
        assert_eq!(report[rows_key][0]["outputTokens"], 50);
        assert_eq!(report[rows_key][0]["cacheCreationTokens"], 10);
        assert_eq!(report[rows_key][0]["cacheReadTokens"], 20);
        assert_eq!(report[rows_key][0]["totalTokens"], 180);
        assert_eq!(report["totals"]["totalCost"], 0.25);
        if kind == "session" {
            assert_eq!(report[rows_key][0]["projectPath"], "/workspace/project");
            assert_eq!(
                report[rows_key][0]["firstActivity"],
                "2099-01-02T01:02:03.000Z"
            );
            assert_eq!(
                report[rows_key][0]["lastActivity"],
                "2099-01-02T01:02:03.000Z"
            );
        }
    }
}
