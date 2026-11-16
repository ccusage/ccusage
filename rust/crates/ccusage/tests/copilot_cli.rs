use std::process::Command;

use ccusage_test_support::{Fixture, fs_fixture};

const SINCE: &str = "20260101";
const UNTIL: &str = "20260228";

#[test]
fn snapshots_copilot_focused_daily_stdout() {
    let fixture = copilot_fixture();

    insta::assert_snapshot!(
        "focused_daily_json",
        format!(
            "Daily JSON\n{}",
            run_cli(&fixture, ["copilot", "daily", "--json"]),
        )
    );
    insta::assert_snapshot!(
        "focused_daily_table",
        format!("Daily\n{}", run_cli(&fixture, ["copilot", "daily"]),)
    );
}

#[test]
fn snapshots_copilot_focused_monthly_and_session_stdout() {
    let fixture = copilot_fixture();

    insta::assert_snapshot!(
        "focused_monthly_and_session_json",
        format!(
            "Monthly JSON\n{}\nSession JSON\n{}",
            run_cli(&fixture, ["copilot", "monthly", "--json"]),
            run_cli(&fixture, ["copilot", "session", "--json"]),
        )
    );
    insta::assert_snapshot!(
        "focused_monthly_and_session_table",
        format!(
            "Monthly\n{}\n\nSession\n{}",
            run_cli(&fixture, ["copilot", "monthly"]),
            run_cli(&fixture, ["copilot", "session"]),
        )
    );
}

#[test]
fn snapshots_copilot_unified_monthly_and_session_stdout() {
    let fixture = copilot_fixture();

    insta::assert_snapshot!(
        "unified_monthly_and_session_json",
        format!(
            "Monthly JSON\n{}\nSession JSON\n{}",
            run_cli(&fixture, ["monthly", "--json"]),
            run_cli(&fixture, ["session", "--json"]),
        )
    );
    insta::assert_snapshot!(
        "unified_monthly_and_session_table",
        format!(
            "Monthly\n{}\n\nSession\n{}",
            run_cli(&fixture, ["monthly"]),
            run_cli(&fixture, ["session"]),
        )
    );
}

#[test]
fn reports_copilot_daily_usage_from_usage_records() {
    let fixture = fs_fixture!({
        "copilot/session-state/session-receipts/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-receipts/events.jsonl"
        ),
        "copilot/session-state/session-resumed-receipts/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-resumed-receipts/events.jsonl"
        ),
    });

    let output: serde_json::Value =
        serde_json::from_str(&run_cli(&fixture, ["copilot", "daily", "--json"]))
            .expect("daily JSON should parse");
    let days = output["daily"]
        .as_array()
        .expect("daily rows")
        .iter()
        .map(|day| {
            (
                day["date"].as_str().unwrap().to_string(),
                day["inputTokens"].as_u64().unwrap(),
                day["outputTokens"].as_u64().unwrap(),
                day["cacheCreationTokens"].as_u64().unwrap(),
                day["cacheReadTokens"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        days,
        [
            ("2026-02-05".to_string(), 200, 120, 100, 1200),
            ("2026-02-06".to_string(), 50, 60, 50, 400),
            ("2026-02-07".to_string(), 100, 40, 100, 300),
            ("2026-02-08".to_string(), 50, 30, 0, 200),
        ]
    );
}

fn copilot_fixture() -> Fixture {
    fs_fixture!({
        "copilot/session-state/session-a/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-a/events.jsonl"
        ),
        "copilot/session-state/session-b/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-b/events.jsonl"
        ),
        "copilot/session-state/session-credits/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-credits/events.jsonl"
        ),
        "copilot/otel/trace.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/otel/trace.jsonl"
        ),
    })
}

fn run_cli<const N: usize>(fixture: &Fixture, args: [&str; N]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .args(args)
        .args(["--offline", "--no-color", "--timezone", "UTC"])
        .args(["--since", SINCE, "--until", UNTIL])
        .env("COPILOT_HOME", fixture.path("copilot"))
        .env("HOME", fixture.path("empty-home"))
        .env("USERPROFILE", fixture.path("empty-userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("empty-xdg-config"))
        .env("XDG_DATA_HOME", fixture.path("empty-xdg-data"))
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "120")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("OPENCODE_DATA_DIR")
        .env_remove("AMP_DATA_DIR")
        .env_remove("DROID_SESSIONS_DIR")
        .env_remove("CODEBUFF_DATA_DIR")
        .env_remove("HERMES_HOME")
        .env_remove("PI_AGENT_DIR")
        .env_remove("GOOSE_PATH_ROOT")
        .env_remove("OPENCLAW_DIR")
        .env_remove("KILO_DATA_DIR")
        .env_remove("COPILOT_OTEL_FILE_EXPORTER_PATH")
        .env_remove("GEMINI_DATA_DIR")
        .env_remove("KIMI_DATA_DIR")
        .env_remove("QWEN_DATA_DIR")
        .env_remove("GROK_HOME")
        .env_remove("CLINE_DATA_DIR")
        .env_remove("JUNIE_HOME")
        .output()
        .expect("ccusage CLI should run");
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}
