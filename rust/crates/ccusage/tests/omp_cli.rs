use std::{path::Path, process::Command};

use ccusage_test_support::Fixture;
use serde_json::{Value, json};

const SINCE: &str = "20260201";
const UNTIL: &str = "20260228";
const EXPECTED_TOTAL_TOKENS: u64 = 3_312;
const PARENT_PATH: &str = "home/.omp/agent/sessions/-workspace/2026-02-02T10-00-00-000Z_019c0000-0000-7000-8000-000000000001.jsonl";

#[test]
fn discovers_omp_in_unified_daily_weekly_monthly_and_session_json() {
    let fixture = omp_fixture();

    for (kind, rows_key) in [
        ("daily", "daily"),
        ("weekly", "weekly"),
        ("monthly", "monthly"),
        ("session", "session"),
    ] {
        let report = json_report(&fixture, &[kind, "--json"], "display");

        assert_eq!(
            report["totals"]["totalTokens"], EXPECTED_TOTAL_TOKENS,
            "unified {kind} should include discovered OMP usage"
        );
        assert_rows_include_agent_and_model(&report, rows_key, "omp", "[omp] rawmodel");
        assert!(
            report[rows_key]
                .as_array()
                .expect("report rows should be an array")
                .iter()
                .any(|row| row["modelsUsed"].as_array().is_some_and(|models| {
                    models.iter().any(|model| model == "[omp] override-model")
                })),
            "unified {kind} should retain OMP model_usage records"
        );
    }

    let sessions = json_report(&fixture, &["session", "--json"], "display");
    let parent = sessions["session"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["period"] == "019c0000-0000-7000-8000-000000000001")
        .unwrap_or_else(|| panic!("OMP parent session should be reported: {sessions}"));
    assert_eq!(parent["totalTokens"], 3_190);
    assert_eq!(
        parent["metadata"]["lastActivity"],
        "2026-02-02T10:05:00.000Z"
    );
    assert_eq!(parent["metadata"]["projectPath"], "-workspace");
}

#[test]
fn unified_tables_snapshot_omp_daily_weekly_monthly_and_session_reports() {
    let fixture = omp_fixture();

    for kind in ["daily", "weekly", "monthly", "session"] {
        let stdout = run_cli(&fixture, &[kind], "display");
        insta::assert_snapshot!(format!("unified_{kind}_table"), stdout);
    }
}

#[test]
fn unified_sections_include_discovered_omp_for_each_requested_report() {
    let fixture = omp_fixture();
    let report = json_report(
        &fixture,
        &["daily", "--sections", "weekly,monthly,session", "--json"],
        "display",
    );

    assert_eq!(report["totals"]["totalTokens"], EXPECTED_TOTAL_TOKENS);
    assert_rows_include_agent_and_model(&report, "daily", "omp", "[omp] rawmodel");
    assert_rows_include_agent_and_model(&report, "weekly", "omp", "[omp] rawmodel");
    assert_rows_include_agent_and_model(&report, "monthly", "omp", "[omp] rawmodel");
    assert_rows_include_agent_and_model(&report, "session", "omp", "[omp] rawmodel");
    assert_eq!(
        report["session"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["totalTokens"].as_u64().unwrap())
            .sum::<u64>(),
        EXPECTED_TOTAL_TOKENS
    );
}

#[test]
fn explicit_omp_named_store_overrides_home_discovery() {
    let fixture = omp_fixture();
    let manual = write_omp_session(
        &fixture,
        "home/manual-omp/-workspace/2026-02-02T12-00-00-000Z_019c0000-0000-7000-8000-000000000005.jsonl",
        include_str!("fixtures/omp/manual_store.jsonl"),
        "Configured OMP store",
    );
    let manual_root = manual
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let config = fixture.write_file(
        "config/named-omp.json",
        serde_json::to_string(&json!({
            "pi": {
                "stores": [{ "name": "omp", "path": manual_root }]
            }
        }))
        .unwrap(),
    );
    let report = json_report_with_config(&fixture, &["daily", "--json"], "display", &config);

    assert_eq!(report["totals"]["totalTokens"], 11);
    assert_rows_include_agent_and_model(&report, "daily", "omp", "[omp] manual-model");
    assert!(!report.to_string().contains("[omp] rawmodel"));
}

#[test]
fn alternate_named_store_owns_an_overlapping_omp_root_once() {
    let fixture = omp_fixture();
    let config = fixture.write_file(
        "config/named-overlap.json",
        serde_json::to_string(&json!({
            "pi": {
                "stores": [{ "name": "archive", "path": "~/.omp/agent/sessions" }]
            }
        }))
        .unwrap(),
    );
    let report = json_report_with_config(&fixture, &["daily", "--json"], "display", &config);

    assert_eq!(report["totals"]["totalTokens"], EXPECTED_TOTAL_TOKENS);
    assert_rows_include_agent_and_model(&report, "daily", "archive", "[archive] rawmodel");
    assert!(!report.to_string().contains("[omp] rawmodel"));
}

#[test]
fn pi_focused_reports_keep_omp_sessions_out() {
    let fixture = omp_fixture();
    let _ = fixture.write_file(
        "home/.pi/agent/sessions/-workspace/pi-session.jsonl",
        include_str!("fixtures/omp/pi_session.jsonl")
            .replace("{{WORKSPACE}}", &workspace(&fixture)),
    );

    let report = json_report(&fixture, &["pi", "daily", "--json"], "display");

    assert_eq!(report["totals"]["totalTokens"], 42);
    assert!(report.to_string().contains("[pi] pi-only-model"));
    assert!(!report.to_string().contains("[omp]"));
}

#[test]
fn stored_zero_cost_is_kept_until_a_matching_custom_price_override_is_configured() {
    let fixture = omp_fixture();
    let default_report = json_report(&fixture, &["daily", "--json"], "auto");
    let default_override_model_cost = model_cost(&default_report, "[omp] override-model");
    assert_eq!(default_override_model_cost, 0.0);

    let config = fixture.write_file(
        "config/pricing-override.json",
        serde_json::to_string(&json!({
            "defaults": {
                "pricingOverrides": {
                    "[omp] override-model": {
                        "inputCostPerToken": 0.001,
                        "outputCostPerToken": 0.002
                    }
                }
            }
        }))
        .unwrap(),
    );
    let overridden = json_report_with_config(&fixture, &["daily", "--json"], "auto", &config);

    assert_eq!(overridden["totals"]["totalTokens"], EXPECTED_TOTAL_TOKENS);
    assert_eq!(model_cost(&overridden, "[omp] override-model"), 5.0);
}

#[test]
fn omp_is_not_a_focused_cli_command() {
    let fixture = omp_fixture();
    let output = run_cli_output(&fixture, &["omp", "daily", "--json"], "display");

    assert!(!output.status.success());
}

fn omp_fixture() -> Fixture {
    let fixture = Fixture::new();
    let parent_path = fixture.path(PARENT_PATH);
    let parent_path = parent_path.to_string_lossy();

    write_omp_session(
        &fixture,
        PARENT_PATH,
        include_str!("fixtures/omp/parent.jsonl"),
        "OMP parent session",
    );
    write_omp_session(
        &fixture,
        "home/.omp/agent/sessions/-workspace/2026-02-02T10-10-00-000Z_019c0000-0000-7000-8000-000000000002.jsonl",
        &include_str!("fixtures/omp/path_child.jsonl").replace("{{PARENT_PATH}}", &parent_path),
        "OMP path child session",
    );
    write_omp_session(
        &fixture,
        "home/.omp/agent/sessions/-workspace/2026-02-02T11-00-00-000Z_019c0000-0000-7000-8000-000000000003.jsonl",
        include_str!("fixtures/omp/id_parent.jsonl"),
        "OMP id parent session",
    );
    write_omp_session(
        &fixture,
        "home/.omp/agent/sessions/-workspace/2026-02-02T11-05-00-000Z_019c0000-0000-7000-8000-000000000004.jsonl",
        include_str!("fixtures/omp/id_child.jsonl"),
        "OMP id child session",
    );

    fixture
}

fn write_omp_session(fixture: &Fixture, path: &str, body: &str, title: &str) -> std::path::PathBuf {
    let body = body.replace("{{WORKSPACE}}", &workspace(fixture));
    let slot = omp_title_slot(title, "2026-02-02T10:00:00.000Z");
    fixture.write_file(path, format!("{slot}{body}"))
}

fn omp_title_slot(title: &str, updated_at: &str) -> String {
    let mut slot = json!({
        "type": "title",
        "v": 1,
        "title": title,
        "source": "auto",
        "updatedAt": updated_at,
        "pad": "",
    });
    let unpadded = serde_json::to_string(&slot).unwrap();
    let padding = 255usize
        .checked_sub(unpadded.len())
        .expect("OMP title slot metadata must fit its fixed-width record");
    slot["pad"] = json!(" ".repeat(padding));
    let line = serde_json::to_string(&slot).unwrap();
    assert_eq!(line.len() + 1, 256);
    format!("{line}\n")
}

fn workspace(fixture: &Fixture) -> String {
    fixture
        .path("home/workspace")
        .to_string_lossy()
        .into_owned()
}

fn json_report(fixture: &Fixture, args: &[&str], mode: &str) -> Value {
    let output = run_cli(fixture, args, mode);
    serde_json::from_str(&output).expect("ccusage JSON stdout should parse")
}

fn json_report_with_config(fixture: &Fixture, args: &[&str], mode: &str, config: &Path) -> Value {
    let mut args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    args.push("--config".to_string());
    args.push(config.to_string_lossy().into_owned());
    let output = run_cli_owned(fixture, &args, mode);
    serde_json::from_str(&output).expect("ccusage JSON stdout should parse")
}

fn assert_rows_include_agent_and_model(report: &Value, rows_key: &str, agent: &str, model: &str) {
    let rows = report[rows_key]
        .as_array()
        .expect("report rows should be an array");
    assert!(
        rows.iter().any(|row| {
            row["agent"] == agent
                || row["metadata"]["agents"]
                    .as_array()
                    .is_some_and(|agents| agents.iter().any(|value| value == agent))
        }),
        "{rows_key} should label usage as {agent}: {report}"
    );
    assert!(
        rows.iter().any(|row| {
            row["modelsUsed"]
                .as_array()
                .is_some_and(|models| models.iter().any(|value| value == model))
        }),
        "{rows_key} should include model {model}: {report}"
    );
}

fn model_cost(report: &Value, model: &str) -> f64 {
    report["daily"][0]["modelBreakdowns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|breakdown| breakdown["modelName"] == model)
        .unwrap_or_else(|| panic!("model breakdown {model} should exist in {report}"))["cost"]
        .as_f64()
        .unwrap()
}

fn run_cli(fixture: &Fixture, args: &[&str], mode: &str) -> String {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_cli_owned(fixture, &args, mode)
}

fn run_cli_owned(fixture: &Fixture, args: &[String], mode: &str) -> String {
    let output = run_cli_output_owned(fixture, args, mode);
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage stdout should be UTF-8")
}

fn run_cli_output(fixture: &Fixture, args: &[&str], mode: &str) -> std::process::Output {
    run_cli_output_owned(
        fixture,
        &args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>(),
        mode,
    )
}

fn run_cli_output_owned(fixture: &Fixture, args: &[String], mode: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .current_dir(fixture.root())
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("home"))
        .env("XDG_CONFIG_HOME", fixture.path("home/.config"))
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "180")
        .args(args)
        .args([
            "--offline",
            "--no-color",
            "--timezone",
            "UTC",
            "--mode",
            mode,
            "--since",
            SINCE,
            "--until",
            UNTIL,
        ])
        .output()
        .expect("ccusage CLI should run")
}
