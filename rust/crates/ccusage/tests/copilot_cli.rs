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

fn copilot_fixture() -> Fixture {
    fs_fixture!({
        "copilot/session-state/session-a/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-a/events.jsonl"
        ),
        "copilot/session-state/session-b/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-b/events.jsonl"
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
        .output()
        .expect("ccusage CLI should run");
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}
