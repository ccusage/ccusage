use std::{collections::BTreeSet, process::Command};

use ccusage_test_support::{Fixture, fs_fixture};

#[test]
fn snapshots_cline_focused_json_and_table_reports_for_all_kinds() {
    let fixture = cline_fixture();

    for kind in ["daily", "weekly", "monthly", "session"] {
        let json_output = run_cli(&fixture, kind, true);
        let report: serde_json::Value =
            serde_json::from_str(&json_output).expect("Cline JSON output should be valid");
        assert_only_accepted_requests_are_reported(&report, kind);
        insta::assert_json_snapshot!(format!("cline_cli_{kind}_json"), report);

        let table_output = run_cli(&fixture, kind, false);
        insta::assert_snapshot!(format!("cline_cli_{kind}_table"), table_output);
    }
}

fn assert_only_accepted_requests_are_reported(report: &serde_json::Value, kind: &str) {
    let totals = &report["totals"];
    for (key, expected) in [
        ("inputTokens", 255),
        ("outputTokens", 51),
        ("cacheCreationTokens", 30),
        ("cacheReadTokens", 50),
        ("totalTokens", 386),
    ] {
        assert_eq!(totals[key].as_u64(), Some(expected), "{key} totals");
    }
    assert!((totals["totalCost"].as_f64().unwrap() - 0.0012).abs() < 1e-12);
    assert!(totals.get("unpricedModels").is_none());

    let (rows_key, period_key, expected_periods): (&str, &str, &[&str]) = match kind {
        "daily" => ("daily", "date", &["2026-10-01", "2026-11-01"]),
        "weekly" => ("weekly", "week", &["2026-09-28", "2026-10-26"]),
        "monthly" => ("monthly", "month", &["2026-10", "2026-11"]),
        "session" => (
            "sessions",
            "sessionId",
            &["cli:session-a", "task:session-a"],
        ),
        _ => unreachable!("test iterates over supported Cline reports"),
    };
    let rows = report[rows_key]
        .as_array()
        .expect("report rows should be an array");
    let mut periods = rows
        .iter()
        .map(|row| row[period_key].as_str().expect("period should be a string"))
        .collect::<Vec<_>>();
    periods.sort_unstable();
    assert_eq!(periods, expected_periods.to_vec());

    let models = rows
        .iter()
        .flat_map(|row| {
            row["modelsUsed"]
                .as_array()
                .expect("modelsUsed should be an array")
        })
        .map(|model| model.as_str().expect("model should be a string"))
        .collect::<BTreeSet<_>>();
    assert_eq!(models, BTreeSet::from(["claude-sonnet-4-20250514"]));
}

fn cline_fixture() -> Fixture {
    let fixture = fs_fixture!({
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
    });
    let _ = fixture.write_file(
        "data/sessions/rejected-cache/rejected-cache.messages.json",
        r#"{"version":1,"sessionId":"rejected-cache","messages":[{"role":"assistant","ts":1793491260000,"id":"cache-exceeds-input-session","modelInfo":{"id":"claude-sonnet-4-20250514","provider":"anthropic"},"metrics":{"inputTokens":5,"outputTokens":2,"cacheReadTokens":4,"cacheWriteTokens":2,"cost":0.5}}]}"#,
    );
    fixture
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
