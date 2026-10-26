use std::process::Command;

use ccusage_test_support::{Fixture, fs_fixture};

#[test]
fn snapshots_cline_focused_json_and_table_reports_for_all_kinds() {
    let fixture = cline_fixture();

    for kind in ["daily", "weekly", "monthly", "session"] {
        let json_output = run_cli(&fixture, kind, true);
        let report: serde_json::Value =
            serde_json::from_str(&json_output).expect("Cline JSON output should be valid");
        insta::assert_json_snapshot!(format!("cline_cli_{kind}_json"), report);

        let table_output = run_cli(&fixture, kind, false);
        insta::assert_snapshot!(format!("cline_cli_{kind}_table"), table_output);
    }
}

fn cline_fixture() -> Fixture {
    fs_fixture!({
        "tasks/session-a/ui_messages.json": include_str!(
            "../../../adapters/cline/tests/fixtures/extension/ui_messages.json"
        ),
        "tasks/session-a/task_metadata.json": include_str!(
            "../../../adapters/cline/tests/fixtures/extension/task_metadata.json"
        ),
        "tasks/session-a/api_conversation_history.json": include_str!(
            "../../../adapters/cline/tests/fixtures/extension/api_conversation_history.json"
        ),
        "data/sessions/session-a/session-a.messages.json": include_str!(
            "../../../adapters/cline/tests/fixtures/cli/session.messages.json"
        ),
    })
}

fn run_cli(fixture: &Fixture, kind: &str, json: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccusage"));
    command
        .env_clear()
        .env("CLINE_DATA_DIR", fixture.root())
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("COLUMNS", "120")
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .args([
            "cline",
            kind,
            "--since",
            "20261001",
            "--until",
            "20261101",
            "--offline",
            "--no-color",
            "--timezone",
            "UTC",
        ]);
    if json {
        command.arg("--json");
    }

    let output = command.output().expect("ccusage should run");
    assert!(
        output.status.success(),
        "ccusage cline {kind} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout should be UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("CLI stderr should be UTF-8");
    if json {
        assert!(stderr.is_empty(), "JSON report should not write to stderr");
        stdout
    } else {
        format!("stdout:\n{stdout}\nstderr:\n{stderr}")
    }
}
