#![cfg(test)]

use ccusage_test_support::{Fixture, instant};
use serde_json::Value;

mod support;

const MODEL: &str = r#"{"id":"gpt-5","providerID":"openai"}"#;

fn opencode_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.create_dir_all("opencode");
    let db = sqlite::open(fixture.path("opencode/opencode.db"))
        .expect("open the OpenCode fixture database");
    db.execute(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, \
         seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT);
         CREATE TABLE session (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, \
         tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, \
         tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT);",
    )
    .expect("create the OpenCode fixture schema");
    let assistant = format!(
        r#"{{"model":{MODEL},"time":{{"created":{created}}},"content":[],"tokens":{{"input":10,"output":2,"reasoning":0,"cache":{{"read":0,"write":0}}}},"cost":0.01}}"#,
        created = instant("2026-01-02T00:00:00Z").millis()
    );
    let compaction = |created: i64| {
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
            instant("2026-01-02T00:00:00Z").millis(),
            assistant,
        ),
        (
            "mixed-compaction",
            "mixed-session",
            "compaction",
            1,
            instant("2026-01-02T00:00:00.001Z").millis(),
            compaction(instant("2026-01-02T00:00:00.001Z").millis()),
        ),
        (
            "lone-compaction",
            "lone-session",
            "compaction",
            5,
            instant("2026-02-02T00:00:00Z").millis(),
            compaction(instant("2026-02-02T00:00:00Z").millis()),
        ),
    ] {
        let mut statement = db
            .prepare(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .expect("prepare fixture message insert");
        statement
            .bind::<&[(_, sqlite::Value)]>(&[
                (1, id.into()),
                (2, session.into()),
                (3, kind.into()),
                (4, i64::from(seq).into()),
                (5, created.into()),
                (6, data.into()),
            ])
            .expect("bind fixture message values");
        while statement.next().expect("insert fixture message") != sqlite::State::Done {}
    }
    db.execute(format!(
        "INSERT INTO session VALUES ('mixed-session', {mixed_created}, 9.99, 9999, 9999, 0, 0, 0, '{MODEL}');
         INSERT INTO session VALUES ('lone-session', {lone_created}, 0.05, 300, 40, 30, 5, 7, '{MODEL}');",
        mixed_created = instant("2026-01-02T00:00:00Z").millis(),
        lone_created = instant("2026-02-02T00:00:00Z").millis()
    ))
    .expect("insert fixture sessions");
    fixture
}

fn run_cli<const N: usize>(fixture: &Fixture, args: [&str; N]) -> String {
    let output = support::hermetic_command(fixture.root())
        .env("OPENCODE_DATA_DIR", fixture.path("opencode"))
        .env("LOG_LEVEL", "0")
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
        totals["totalTokens"]
            .as_u64()
            .expect("report has total tokens"),
        totals["totalCost"]
            .as_f64()
            .expect("report has a total cost"),
    )
}

#[test]
fn opencode_period_reports_count_compaction_usage() {
    let fixture = opencode_fixture();

    for kind in ["daily", "weekly", "monthly"] {
        let (tokens, cost) = totals(&run_cli(&fixture, [kind, "--json"]));
        assert_eq!(tokens, 336, "{kind} total tokens");
        assert!((cost - 0.05).abs() < 1e-9, "{kind} total cost {cost}");
    }
}

#[test]
fn opencode_session_report_counts_compaction_once() {
    let fixture = opencode_fixture();

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

#[test]
fn opencode_logs_an_unindexed_date_window_once_when_the_parallel_read_falls_back() {
    let fixture = Fixture::new();
    fixture.create_dir_all("opencode");
    sqlite::open(fixture.path("opencode/opencode.db"))
        .expect("open the OpenCode fixture database")
        .execute(
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, \
             seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT) WITHOUT ROWID;
             CREATE TABLE padding (bytes BLOB);
             INSERT INTO padding VALUES (zeroblob(70000000));",
        )
        .expect("create a WITHOUT ROWID table past the parallel size");

    let output = support::hermetic_command(fixture.root())
        .env("OPENCODE_DATA_DIR", fixture.path("opencode"))
        .args(["opencode", "daily", "--since", "20260101", "--debug"])
        .args(["--json", "--offline", "--timezone", "UTC"])
        .output()
        .expect("ccusage CLI should run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "ccusage CLI failed: {stderr}");
    assert_eq!(stderr.matches("scanning unfiltered").count(), 1, "{stderr}");
}
