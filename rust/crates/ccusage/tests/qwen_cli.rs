use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::Value;

/// Two sessions in America/Los_Angeles (UTC-8): the first is last active at
/// 2026-02-24T02:00Z, still 2026-02-23 locally, and the second a day later.
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

fn run_cli(fixture: &Fixture, args: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("QWEN_DATA_DIR", fixture.path("qwen"))
        .envs(std::env::var_os("TZDIR").map(|zoneinfo| ("TZDIR", zoneinfo)))
        .env("NO_COLOR", "1")
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
