use std::{path::PathBuf, process::Command};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../adapters/junie/tests/fixtures")
}

#[test]
fn snapshots_junie_focused_json_and_table_reports_for_all_kinds() {
    for kind in ["daily", "weekly", "monthly", "session"] {
        let json_output = run_cli(kind, true);
        let report: serde_json::Value =
            serde_json::from_str(&json_output).expect("Junie JSON output should be valid");
        assert_totals(&report);
        insta::assert_json_snapshot!(format!("junie_cli_{kind}_json"), report);

        let table_output = run_cli(kind, false);
        insta::assert_snapshot!(format!("junie_cli_{kind}_table"), table_output);
    }
}

#[test]
fn session_report_carries_project_directories_from_the_index() {
    let report: serde_json::Value = serde_json::from_str(&run_cli("session", true)).unwrap();
    let projects = report["sessions"]
        .as_array()
        .expect("sessions should be an array")
        .iter()
        .map(|row| {
            (
                row["sessionId"].as_str().unwrap(),
                row["projectPath"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        projects,
        [
            ("session-261031-115900-a1b2", "/work/web-app"),
            ("session-261101-170000-c3d4", "/work/cli-tool"),
            ("session-261102-091500-e5f6", "Junie"),
        ]
    );
}

fn assert_totals(report: &serde_json::Value) {
    let totals = &report["totals"];
    for (key, expected) in [
        ("inputTokens", 4481),
        ("outputTokens", 320),
        ("cacheCreationTokens", 50),
        ("cacheReadTokens", 400),
        ("totalTokens", 5251),
    ] {
        assert_eq!(totals[key].as_u64(), Some(expected), "{key} totals");
    }
    let cost = totals["totalCost"].as_f64().unwrap();
    assert!(
        (cost - 0.0128075).abs() < 1e-12,
        "recorded cost total: {cost}"
    );
}

fn run_cli(kind: &str, json: bool) -> String {
    let fixture = fixture_root();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccusage"));
    command
        .env_clear()
        .env("JUNIE_HOME", &fixture)
        .env("HOME", fixture.join("home"))
        .env("USERPROFILE", fixture.join("userprofile"))
        .env("COLUMNS", "120")
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .args([
            "junie",
            kind,
            "--since",
            "20261001",
            "--until",
            "20261130",
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
        "ccusage junie {kind} failed: {}",
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
