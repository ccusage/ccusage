use std::path::Path;

use ccusage_core::parse_ts_timestamp;
use ccusage_test_support::Fixture;

#[test]
fn zcode_cli_tables_snapshot_production_stdout_and_stderr() {
    let fixture = Fixture::new();
    let _ = fixture.create_dir_all("zcode/cli/db");
    create_zcode_fixture(&fixture.path("zcode/cli/db/db.sqlite"));

    for kind in ["daily", "monthly", "session"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
            .env("ZCODE_HOME", fixture.path("zcode"))
            .args([
                "zcode",
                kind,
                "--since",
                "20990101",
                "--until",
                "20990201",
                "--mode",
                "calculate",
                "--offline",
                "--no-color",
                "--timezone",
                "UTC",
            ])
            .output()
            .expect("failed to run ccusage");

        assert!(
            output.status.success(),
            "ccusage zcode {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
        insta::assert_snapshot!(
            format!("zcode_cli_{kind}_table"),
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        );
    }
}

fn create_zcode_fixture(path: &Path) {
    let db = sqlite::open(path).unwrap();
    db.execute(
        "CREATE TABLE model_usage (
            id TEXT PRIMARY KEY, session_id TEXT, started_at INTEGER, model_id TEXT,
            provider_id TEXT, status TEXT, input_tokens INTEGER, output_tokens INTEGER,
            cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
            computed_total_tokens INTEGER
        )",
    )
    .unwrap();
    db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, version TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO session VALUES
            ('session-a', '/workspace/project-a', '0.16.3'),
            ('session-b', '/workspace/project-b', '0.16.3')",
    )
    .unwrap();
    insert_zcode_usage(
        &db,
        ZcodeFixtureUsage {
            id: "usage-52",
            session_id: "session-a",
            timestamp: "2099-01-02T00:00:00.000Z",
            model: "GLM-5.2",
            input_tokens: 100,
            output_tokens: 10,
            cache_creation_tokens: 15,
            cache_read_tokens: 25,
            computed_total_tokens: 110,
        },
    );
    insert_zcode_usage(
        &db,
        ZcodeFixtureUsage {
            id: "usage-53",
            session_id: "session-a",
            timestamp: "2099-01-15T12:00:00.000Z",
            model: "GLM-5.3",
            input_tokens: 200,
            output_tokens: 20,
            cache_creation_tokens: 30,
            cache_read_tokens: 40,
            computed_total_tokens: 220,
        },
    );
    insert_zcode_usage(
        &db,
        ZcodeFixtureUsage {
            id: "usage-53-b",
            session_id: "session-b",
            timestamp: "2099-02-01T00:00:00.000Z",
            model: "GLM-5.3",
            input_tokens: 50,
            output_tokens: 5,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            computed_total_tokens: 55,
        },
    );
}

struct ZcodeFixtureUsage<'a> {
    id: &'a str,
    session_id: &'a str,
    timestamp: &'a str,
    model: &'a str,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    computed_total_tokens: i64,
}

fn insert_zcode_usage(db: &sqlite::Connection, usage: ZcodeFixtureUsage<'_>) {
    let mut statement = db
        .prepare(
            "INSERT INTO model_usage
             (id, session_id, started_at, model_id, provider_id, status, input_tokens,
              output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
              computed_total_tokens)
             VALUES (?1, ?2, ?3, ?4, 'builtin:zai-coding-plan', 'completed', ?5, ?6, ?7, ?8, ?9)",
        )
        .unwrap();
    statement.bind((1, usage.id)).unwrap();
    statement.bind((2, usage.session_id)).unwrap();
    statement
        .bind((3, parse_ts_timestamp(usage.timestamp).unwrap().as_millis()))
        .unwrap();
    statement.bind((4, usage.model)).unwrap();
    statement.bind((5, usage.input_tokens)).unwrap();
    statement.bind((6, usage.output_tokens)).unwrap();
    statement.bind((7, usage.cache_creation_tokens)).unwrap();
    statement.bind((8, usage.cache_read_tokens)).unwrap();
    statement.bind((9, usage.computed_total_tokens)).unwrap();
    statement.next().unwrap();
}
