use std::ffi::OsString;
use std::{fs, path::Path};

use ccusage_test_support::{Instant, instant, set_modified};

use crate::golden::Case;

const TRANSCRIPT_EVENTS_SCHEMA: &str = "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))";

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let state = root.join("openclaw/state");
    seed_legacy_sessions(&state);
    seed_agent_databases(&state);
    seed_alternate_store(&root.join("openclaw/alt"));
    vec![("OPENCLAW_DIR", vec![state.into_os_string()])]
}

pub(crate) fn extra_cases(root: &Path) -> Vec<Case> {
    let state = root.join("openclaw/state");
    let alt = root.join("openclaw/alt");
    let alt_only = alt.to_string_lossy().into_owned();
    let both = format!("{},{}", state.display(), alt.display());
    let missing = root.join("openclaw/missing").to_string_lossy().into_owned();
    vec![
        Case::new(
            "daily",
            "json_open_claw_path",
            &[
                "openclaw",
                "daily",
                "--offline",
                "--json",
                "--open-claw-path",
                &alt_only,
            ],
        ),
        Case::new(
            "session",
            "json_open_claw_path_list",
            &[
                "openclaw",
                "session",
                "--offline",
                "--json",
                "--open-claw-path",
                &both,
            ],
        ),
        Case::new(
            "daily",
            "json_open_claw_path_missing",
            &[
                "openclaw",
                "daily",
                "--offline",
                "--json",
                "--open-claw-path",
                &missing,
            ],
        ),
    ]
}

fn write_session(path: &Path, lines: &[&str], modified: Instant) {
    fs::create_dir_all(path.parent().expect("session file has a parent"))
        .expect("failed to create OpenClaw sessions directory");
    let mut contents = lines.join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    fs::write(path, contents).expect("failed to write OpenClaw session file");
    set_modified(path, modified);
}

fn seed_legacy_sessions(state: &Path) {
    let sessions = state.join("agents/main/sessions");

    let replayed = &format!(
        r#"{{"type":"message","id":"a-4","message":{{"role":"assistant","usage":{{"input":1000,"output":200,"cacheRead":4000,"cacheWrite":300,"cost":{{"total":0.5}}}},"timestamp":{timestamp}}}}}"#,
        timestamp = instant("2025-12-30T10:00:00Z").millis()
    );
    write_session(
        &sessions.join("alpha.jsonl"),
        &[
            r#"{"type":"session","id":"alpha","usage":{"input":9999,"output":9999}}"#,
            r#"{"type":"message","id":"a-1","message":{"role":"assistant","usage":{"input":11,"output":7},"timestamp":"2025-12-27T22:00:00.000Z"}}"#,
            r#"{"type":"model_change","provider":"anthropic","modelId":"claude-sonnet-4-20250514"}"#,
            r#"{"type":"message","message":{"role":"user","usage":{"input":7777,"output":7777},"timestamp":"2025-12-30T09:59:00.000Z"}}"#,
            replayed,
            replayed,
            &format!(
                r#"{{"type":"message","id":"a-6","message":{{"role":"assistant","usage":{{"input":1000,"output":200,"cacheRead":4000,"cacheWrite":300,"cost":{{"total":0.75}}}},"timestamp":{timestamp}}}}}"#,
                timestamp = instant("2025-12-30T10:00:00Z").millis()
            ),
            r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":120,"output":30}}}"#,
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":9999,"outp"#,
            r#"{"type":"custom","customType":"note","data":{"modelId":"openclaw-ignored-model"},"usage":{"input":9999}}"#,
            r#"{"type":"custom","customType":"model-snapshot","data":{"provider":"openai","modelId":"gpt-5"}}"#,
            r#"{"type":"message","timestamp":"2025-12-31T15:30:00.000Z","message":{"role":"assistant","usage":{"input":700,"output":150,"cacheRead":2500,"cost":{"total":0.02}}}}"#,
            &format!(
                r#"{{"type":"message","message":{{"role":"assistant","usage":{{"totalTokens":345}},"timestamp":{timestamp}}}}}"#,
                timestamp = instant("2025-12-31T16:00:00.00075Z").millis_f64()
            ),
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":0,"output":0},"timestamp":"2025-12-31T17:00:00.000Z"}}"#,
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":"9999","output":-5,"cacheRead":9999.5},"timestamp":"2025-12-31T17:01:00.000Z"}}"#,
            r#"{"type":"message","message":{"role":"assistant","modelId":"claude-haiku-4-5-20251001","model":"openclaw-ignored-model","usage":{"input":900,"output":400,"cacheWrite":200,"cacheRead":7000,"cost":9.99},"timestamp":"2026-01-01T09:00:00.000Z"}}"#,
        ],
        instant("2025-12-30T12:00:00Z"),
    );

    let beta_model = r#"{"type":"model_change","model":"openclaw-mystery-1","provider":"mystery"}"#;
    let beta_replayed = r#"{"type":"message","message":{"role":"assistant","usage":{"input":300,"output":70,"cost":{"total":0.2}},"timestamp":"2025-12-31T23:50:00.000Z"}}"#;
    write_session(
        &sessions.join("beta.jsonl.reset.2026-01-01T00-00-00.000Z"),
        &[
            beta_model,
            beta_replayed,
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":40,"output":10},"timestamp":"2025-12-31T23:55:00.000Z"}}"#,
        ],
        instant("2026-01-01T00:00:00Z"),
    );
    write_session(
        &sessions.join("beta.jsonl"),
        &[
            beta_model,
            beta_replayed,
            r#"{"type":"message","message":{"role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input":600,"output":100,"cacheRead":3000},"timestamp":"2026-01-01T00:10:00.000Z"}}"#,
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":800,"output":300,"cacheWrite":50,"cacheRead":6000,"totalTokens":7500,"cost":{"total":0.33}},"timestamp":"2026-01-02T10:00:00.000Z"}}"#,
            r#"{"type":"message","message":{"role":"assistant","modelId":"claude-sonnet-4-20250514","usage":{"input":50,"output":25},"timestamp":-5}}"#,
        ],
        instant("2026-01-02T10:00:00Z"),
    );

    write_session(
        &sessions.join(format!(
            "gamma.jsonl.deleted.{deleted_at}",
            deleted_at = instant("2025-12-29T09:20:00Z").millis()
        )),
        &[
            r#"{"type":"message","message":{"role":"assistant","model":"claude-opus-4-20250514","usage":{"input":100,"output":50,"cost":{"total":0.01}},"timestamp":"2025-12-27T23:00:00Z"}}"#,
            &format!(
                r#"{{"type":"message","message":{{"role":"assistant","model":"claude-sonnet-4-20250514","usage":{{"input":1000,"output":200,"cacheRead":4000,"cacheWrite":300,"cost":{{"total":0.5}}}},"timestamp":{timestamp}}}}}"#,
                timestamp = instant("2025-12-30T10:00:00Z").millis()
            ),
        ],
        instant("2025-12-28T22:00:00Z"),
    );

    write_session(
        &sessions.join("empty.jsonl"),
        &[],
        instant("2025-12-28T22:00:00Z"),
    );
    fs::write(
        sessions.join("truncated.jsonl"),
        r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":9999,"outp"#,
    )
    .expect("failed to write truncated OpenClaw session file");
    set_modified(
        &sessions.join("truncated.jsonl"),
        instant("2025-12-28T22:00:00Z"),
    );

    let not_a_session = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":7777,"output":7777},"timestamp":"2025-12-31T12:00:00.000Z"}}"#;
    write_session(
        &sessions.join("alpha.jsonl.bak"),
        &[not_a_session],
        instant("2025-12-28T22:00:00Z"),
    );
    write_session(
        &sessions.join("sessions.json"),
        &[not_a_session],
        instant("2025-12-28T22:00:00Z"),
    );

    write_session(
        &state.join("logs/gateway.jsonl"),
        &[
            r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":13,"output":4},"timestamp":"2026-01-01T03:00:00.000Z"}}"#,
        ],
        instant("2026-01-01T00:00:00Z"),
    );
}

fn create_agent_database(path: &Path, rows: &[(&str, i64, &str)]) {
    fs::create_dir_all(path.parent().expect("database file has a parent"))
        .expect("failed to create OpenClaw agent directory");
    let db = sqlite::open(path).expect("failed to create OpenClaw agent database");
    db.execute(TRANSCRIPT_EVENTS_SCHEMA)
        .expect("failed to create transcript_events");
    for (seq, (session_id, created_at, event_json)) in rows.iter().enumerate() {
        let mut statement = db
            .prepare("INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES (?1, ?2, ?3, ?4)")
            .expect("failed to prepare transcript_events insert");
        statement.bind((1, *session_id)).expect("bind session_id");
        statement
            .bind((2, i64::try_from(seq).expect("row index fits in i64")))
            .expect("bind seq");
        statement.bind((3, *event_json)).expect("bind event_json");
        statement.bind((4, *created_at)).expect("bind created_at");
        statement.next().expect("failed to insert transcript event");
    }
}

fn seed_agent_databases(state: &Path) {
    let database = |agent: &str| state.join(format!("agents/{agent}/agent/openclaw-agent.sqlite"));

    let without_id =
        r#"{"type":"message","message":{"role":"assistant","usage":{"input":400,"output":100}}}"#;
    create_agent_database(
        &database("main"),
        &[
            (
                "alpha",
                0,
                r#"{"id":"evt-a10","type":"message","timestamp":"2025-12-31T15:30:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":700,"output":150,"cacheRead":2500,"cost":{"total":0.03}}}}"#,
            ),
            (
                "alpha",
                0,
                &format!(
                    r#"{{"id":"evt-a4","type":"message","message":{{"role":"assistant","model":"claude-sonnet-4-20250514","usage":{{"input":1000,"output":200,"cacheRead":4000,"cacheWrite":300,"cost":{{"total":0.6}}}},"timestamp":{timestamp}}}}}"#,
                    timestamp = instant("2025-12-30T10:00:00Z").millis()
                ),
            ),
            (
                "beta",
                0,
                r#"{"type":"message","message":{"role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input":600,"output":100,"cacheRead":3000},"timestamp":"2026-01-01T00:10:00.000Z"}}"#,
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                r#"{"id":"evt-d0","type":"model_change","modelId":"claude-haiku-4-5-20251001","provider":"anthropic"}"#,
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                &format!(
                    r#"{{"id":"evt-d1","type":"message","message":{{"role":"assistant","usage":{{"input":2000,"output":500,"cacheRead":10000,"cacheWrite":1000,"cost":{{"total":0.11}}}},"timestamp":{timestamp}}}}}"#,
                    timestamp = instant("2025-12-31T15:00:00Z").millis()
                ),
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                &format!(
                    r#"{{"id":"evt-d1","type":"message","message":{{"role":"assistant","usage":{{"input":8888,"output":8888}},"timestamp":{timestamp}}}}}"#,
                    timestamp = instant("2025-12-31T15:01:00Z").millis()
                ),
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                "not json",
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                without_id,
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                without_id,
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                r#"{"id":"evt-d9","type":"redaction","message":{"role":"assistant","usage":{"input":9999,"output":9999}}}"#,
            ),
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                r#"{"id":"evt-d10","type":"message","message":{"role":"user","usage":{"input":7777,"output":7777}}}"#,
            ),
            (
                "epsilon",
                0,
                r#"{"id":"evt-d1","type":"message","message":{"role":"assistant","usage":{"input":60,"output":40},"timestamp":"2026-01-02T08:00:00.000Z"}}"#,
            ),
        ],
    );

    create_agent_database(
        &database("worker"),
        &[
            (
                "delta",
                instant("2026-01-01T12:00:00Z").millis(),
                &format!(
                    r#"{{"id":"evt-d1","type":"message","message":{{"role":"assistant","model":"gpt-5","usage":{{"input":6666,"output":6666}},"timestamp":{timestamp}}}}}"#,
                    timestamp = instant("2025-12-31T15:02:00Z").millis()
                ),
            ),
            (
                "zeta",
                0,
                r#"{"id":"evt-z1","type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":500,"output":250,"cacheRead":1500},"timestamp":"2026-01-01T23:30:00.000Z"}}"#,
            ),
            (
                "zeta",
                0,
                r#"{"id":"evt-z2","type":"message","timestamp":"2025-12-31T15:30:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":700,"output":150,"cacheRead":2500,"cost":{"total":0.03}}}}"#,
            ),
        ],
    );

    let write_database_file = |agent: &str, contents: &[u8]| {
        let path = database(agent);
        fs::create_dir_all(path.parent().expect("database file has a parent"))
            .expect("failed to create OpenClaw agent directory");
        fs::write(path, contents).expect("failed to write OpenClaw agent database file");
    };
    write_database_file("blank", b"");
    write_database_file(
        "corrupt",
        b"this is not a SQLite database, only a file with its name\n",
    );
    let other = database("other");
    fs::create_dir_all(other.parent().expect("database file has a parent"))
        .expect("failed to create OpenClaw agent directory");
    sqlite::open(&other)
        .expect("failed to create OpenClaw agent database")
        .execute("CREATE TABLE other (id TEXT)")
        .expect("failed to create unrelated table");
}

fn seed_alternate_store(alt: &Path) {
    write_session(
        &alt.join("agents/main/sessions/beta.jsonl"),
        &[
            r#"{"type":"model_change","model":"openclaw-mystery-1","provider":"mystery"}"#,
            r#"{"type":"message","message":{"role":"assistant","usage":{"input":40,"output":10},"timestamp":"2025-12-31T23:55:00.000Z"}}"#,
            r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":210,"output":90,"cost":{"total":0.04}},"timestamp":"2026-01-01T06:00:00.000Z"}}"#,
        ],
        instant("2026-01-01T00:00:00Z"),
    );
}
