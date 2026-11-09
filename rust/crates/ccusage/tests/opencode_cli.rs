use std::process::Command;

use ccusage_test_support::Fixture;
use serde_json::Value;

const MODEL: &str = r#"{"id":"gpt-5","providerID":"openai"}"#;

/// `mixed-session` has an assistant turn and a completed compaction, plus an
/// inflated session aggregate that message rows must suppress. `lone-session`
/// keeps only a compaction row; its aggregate already includes that usage.
fn opencode_fixture() -> Fixture {
    let fixture = Fixture::new();
    let _ = fixture.create_dir_all("opencode");
    let db = sqlite::open(fixture.path("opencode/opencode.db")).unwrap();
    db.execute(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, \
         seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT);
         CREATE TABLE session (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, \
         tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, \
         tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT);",
    )
    .unwrap();
    let assistant = format!(
        r#"{{"model":{MODEL},"time":{{"created":1767312000000}},"content":[],"tokens":{{"input":10,"output":2,"reasoning":0,"cache":{{"read":0,"write":0}}}},"cost":0.01}}"#
    );
    let compaction = |created: u64| {
        format!(
            r#"{{"status":"completed","reason":"auto","summary":"s","model":{MODEL},"time":{{"created":{created}}},"tokens":{{"input":100,"output":20,"reasoning":7,"cache":{{"read":30,"write":5}}}},"cost":0.02}}"#
        )
    };
    for (id, session, kind, seq, created, data) in [
        (
            "mixed-assistant",
            "mixed-session",
            "assistant",
            0,
            1_767_312_000_000_u64,
            assistant,
        ),
        (
            "mixed-compaction",
            "mixed-session",
            "compaction",
            1,
            1_767_312_000_001,
            compaction(1_767_312_000_001),
        ),
        (
            "lone-compaction",
            "lone-session",
            "compaction",
            5,
            1_769_990_400_000,
            compaction(1_769_990_400_000),
        ),
    ] {
        let mut statement = db
            .prepare(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .unwrap();
        statement
            .bind::<&[(_, sqlite::Value)]>(&[
                (1, id.into()),
                (2, session.into()),
                (3, kind.into()),
                (4, i64::from(seq).into()),
                (5, i64::try_from(created).unwrap().into()),
                (6, data.into()),
            ])
            .unwrap();
        while statement.next().unwrap() != sqlite::State::Done {}
    }
    db.execute(format!(
        "INSERT INTO session VALUES ('mixed-session', 1767312000000, 9.99, 9999, 9999, 0, 0, 0, '{MODEL}');
         INSERT INTO session VALUES ('lone-session', 1769990400000, 0.05, 300, 40, 30, 5, 7, '{MODEL}');"
    ))
    .unwrap();
    fixture
}

fn run_cli<const N: usize>(fixture: &Fixture, args: [&str; N]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("XDG_DATA_HOME", fixture.path("xdg-data"))
        .env("OPENCODE_DATA_DIR", fixture.path("opencode"))
        .env("LOG_LEVEL", "0")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "120")
        .args(["opencode"])
        .args(args)
        .args([
            "--mode",
            "display",
            "--offline",
            "--no-color",
            "--timezone",
            "UTC",
        ])
        .output()
        .expect("ccusage CLI should run");
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}

fn totals(json: &str) -> (u64, f64) {
    let report: Value = serde_json::from_str(json).expect("report JSON should parse");
    let totals = &report["totals"];
    (
        totals["totalTokens"].as_u64().unwrap(),
        totals["totalCost"].as_f64().unwrap(),
    )
}

#[test]
fn opencode_period_reports_count_compaction_usage() {
    let fixture = opencode_fixture();

    // Period reports never use cumulative session aggregates, so both
    // compactions count: 12 + 162 + 162 tokens.
    for kind in ["daily", "weekly", "monthly"] {
        let (tokens, cost) = totals(&run_cli(&fixture, [kind, "--json"]));
        assert_eq!(tokens, 336, "{kind} total tokens");
        assert!((cost - 0.05).abs() < 1e-9, "{kind} total cost {cost}");
    }
}

#[test]
fn opencode_session_report_counts_compaction_once() {
    let fixture = opencode_fixture();

    // `mixed-session` counts its rows (174 tokens) and ignores its aggregate;
    // `lone-session` uses its aggregate (382 tokens), which already includes
    // the compaction, instead of adding the compaction row on top.
    let json = run_cli(&fixture, ["session", "--json"]);
    let (tokens, cost) = totals(&json);
    assert_eq!(tokens, 556);
    assert!((cost - 0.08).abs() < 1e-9, "session total cost {cost}");
}

#[test]
fn opencode_compaction_report_tables_snapshot() {
    let fixture = opencode_fixture();

    for kind in ["daily", "weekly", "monthly", "session"] {
        insta::assert_snapshot!(
            format!("opencode_compaction_{kind}_table"),
            run_cli(&fixture, [kind])
        );
    }
}
