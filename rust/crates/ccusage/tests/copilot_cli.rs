#![cfg(test)]

use ccusage_test_support::{Fixture, fs_fixture};

mod support;

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

#[test]
fn reports_the_credits_copilot_recorded_for_a_resumed_session() {
    let fixture = fs_fixture!({
        "copilot/session-state/session-recorded-credits/events.jsonl": include_str!(
            "../../../adapters/copilot/tests/fixtures/session-state/session-recorded-credits/events.jsonl"
        ),
    });
    let session = |mode: &str| {
        let output: serde_json::Value = serde_json::from_str(&run_cli(
            &fixture,
            ["copilot", "session", "--json", "--mode", mode],
        ))
        .expect("session JSON should parse");
        let session = output["sessions"][0].clone();
        (
            session["totalCost"].as_f64().unwrap(),
            [
                session["inputTokens"].as_u64().unwrap(),
                session["cacheCreationTokens"].as_u64().unwrap(),
                session["cacheReadTokens"].as_u64().unwrap(),
                session["outputTokens"].as_u64().unwrap(),
            ],
        )
    };
    let recorded = 0.004_024_719;
    let tokens = [6, 16_517, 16_473, 10];

    let (auto_cost, auto_tokens) = session("auto");
    let (display_cost, display_tokens) = session("display");
    let (calculated_cost, calculated_tokens) = session("calculate");

    assert!((auto_cost - recorded).abs() < 1e-12, "{auto_cost}");
    assert!((display_cost - recorded).abs() < 1e-12, "{display_cost}");
    let token_priced = 6.0 * 2.5e-6 + 16_517.0 * 2.5e-6 + 16_473.0 * 2.5e-7 + 10.0 * 1.5e-5;
    assert!(
        (calculated_cost - token_priced).abs() < 1e-12,
        "{calculated_cost}"
    );
    assert_eq!(auto_tokens, tokens);
    assert_eq!(display_tokens, tokens);
    assert_eq!(calculated_tokens, tokens);
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
    let output = support::hermetic_command(fixture.root())
        .args(args)
        .args(["--offline", "--no-color", "--timezone", "UTC"])
        .args(["--since", SINCE, "--until", UNTIL])
        .env("COPILOT_HOME", fixture.path("copilot"))
        .env("LOG_LEVEL", "0")
        .env("COLUMNS", "120")
        .output()
        .expect("ccusage CLI should run");
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}
