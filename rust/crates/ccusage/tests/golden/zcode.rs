use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::instant;

use crate::golden::{Case, windowed_table};

const SONNET: &str = "'claude-sonnet-4-20250514'";
const OPUS: &str = "'claude-opus-4-20250514'";
const HAIKU: &str = "'claude-haiku-4-5-20251001'";
const GPT5: &str = "'gpt-5'";
const UNPRICED: &str = "'zcode-mystery-1'";

const ZAI_PLAN: &str = "'builtin:zai-coding-plan'";
const CUSTOM_PROVIDER: &str = "'847d13c9-0568-4f2f-818e-8bd498e5d920'";

const COMPLETED: &str = "'completed'";

#[derive(Clone, Copy)]
enum Layout {
    Current,
    Legacy,
    Minimal,
}

struct Row {
    id: &'static str,
    session: &'static str,
    started_at: String,
    model: &'static str,
    provider: &'static str,
    status: &'static str,
    tokens: [&'static str; 5],
}

fn create_db(path: &Path, layout: Layout) -> sqlite::Connection {
    std::fs::create_dir_all(path.parent().expect("database path has a parent"))
        .expect("failed to create ZCode database directory");
    let db = sqlite::open(path).expect("failed to create ZCode database");
    let schema = match layout {
        Layout::Current => {
            "CREATE TABLE model_usage (
                 id TEXT, session_id TEXT, started_at INTEGER, model_id TEXT, provider_id TEXT,
                 status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                 cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                 computed_total_tokens INTEGER, raw_usage TEXT, prompt TEXT
             );
             CREATE TABLE session (id TEXT, directory TEXT, version TEXT, title TEXT);
             CREATE TABLE rollout (id TEXT, session_id TEXT, input_tokens INTEGER, output_tokens INTEGER);
             INSERT INTO rollout VALUES ('r-1', 's-alpha', 9999, 7777);"
        }
        Layout::Legacy => {
            "CREATE TABLE model_usage (
                 id TEXT, session_id TEXT, started_at INTEGER, model_id TEXT, status TEXT,
                 input_tokens INTEGER, output_tokens INTEGER, cache_read_input_tokens INTEGER,
                 computed_total_tokens INTEGER
             );
             CREATE TABLE session (id TEXT, directory TEXT);"
        }
        Layout::Minimal => {
            "CREATE TABLE model_usage (
                 id TEXT, session_id TEXT, started_at INTEGER, model_id TEXT, status TEXT,
                 input_tokens INTEGER, output_tokens INTEGER
             );
             CREATE TABLE session (id TEXT, directory TEXT);"
        }
    };
    db.execute(schema).expect("failed to create ZCode schema");
    db
}

fn insert(db: &sqlite::Connection, layout: Layout, row: &Row) {
    let Row {
        id,
        session,
        started_at,
        model,
        provider,
        status,
        tokens: [input, output, cache_creation, cache_read, total],
    } = row;
    let statement = match layout {
        Layout::Current => format!(
            "INSERT INTO model_usage VALUES ({id}, {session}, {started_at}, {model}, {provider}, \
             {status}, {input}, {output}, {cache_creation}, {cache_read}, {total}, \
             '{{\"input_tokens\":9999}}', 'never read')"
        ),
        Layout::Legacy => format!(
            "INSERT INTO model_usage VALUES ({id}, {session}, {started_at}, {model}, {status}, \
             {input}, {output}, {cache_read}, {total})"
        ),
        Layout::Minimal => format!(
            "INSERT INTO model_usage VALUES ({id}, {session}, {started_at}, {model}, {status}, \
             {input}, {output})"
        ),
    };
    db.execute(statement)
        .expect("failed to insert ZCode usage row");
}

fn counted(
    id: &'static str,
    session: &'static str,
    started_at: String,
    model: &'static str,
    provider: &'static str,
    tokens: [&'static str; 5],
) -> Row {
    Row {
        id,
        session,
        started_at,
        model,
        provider,
        status: COMPLETED,
        tokens,
    }
}

fn current_rows() -> Vec<Row> {
    vec![
        counted(
            "'u-stale'",
            "'s-alpha'",
            instant("2025-12-27T22:00:00Z").millis().to_string(),
            "'GLM-5.2'",
            "'builtin:bigmodel-coding-plan'",
            ["180", "50", "30", "50", "230"],
        ),
        counted(
            "'u-before'",
            "'s-alpha'",
            instant("2025-12-30T10:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["1000", "200", "100", "300", "1200"],
        ),
        counted(
            "'u-odd-types'",
            "'s-alpha'",
            format!("{}.9", instant("2025-12-30T11:00:00Z").millis()),
            "' GLM-5.3 '",
            "'z.ai'",
            ["'120abc'", "30.7", "-9999", "NULL", "NULL"],
        ),
        counted(
            "'u-custom'",
            "'s-alpha'",
            instant("2025-12-31T12:00:00Z").millis().to_string(),
            SONNET,
            CUSTOM_PROVIDER,
            ["500", "50", "0", "0", "550"],
        ),
        counted(
            "'u-tokyo'",
            "'s-beta'",
            instant("2025-12-31T15:30:00.666Z").millis().to_string(),
            "'GLM-5.3'",
            ZAI_PLAN,
            ["2000", "300", "400", "600", "2300"],
        ),
        counted(
            "'u-empty-provider'",
            "'s-beta'",
            instant("2025-12-31T16:00:00Z").millis().to_string(),
            "'GLM-5.3'",
            "''",
            ["200", "20", "0", "0", "220"],
        ),
        counted(
            "'u-replayed'",
            "'s-beta'",
            instant("2025-12-31T23:50:00Z").millis().to_string(),
            "'GLM-5.3-Flash'",
            "'account:zai-start-plan'",
            ["900", "400", "0", "100", "1300"],
        ),
        counted(
            "'u-replayed'",
            "'s-beta'",
            instant("2025-12-31T23:55:00Z").millis().to_string(),
            "'GLM-5.3-Flash'",
            "'account:zai-start-plan'",
            ["9999", "7777", "0", "0", "17776"],
        ),
        counted(
            "'u-replayed-fork'",
            "'s-beta'",
            instant("2026-01-01T00:10:00Z").millis().to_string(),
            "'GLM-5.3-Flash'",
            "'account:zai-start-plan'",
            ["900", "400", "0", "100", "1300"],
        ),
        counted(
            "'u-mystery'",
            "'s-alpha'",
            instant("2026-01-01T12:00:00Z").millis().to_string(),
            UNPRICED,
            "NULL",
            ["300", "70", "0", "0", "370"],
        ),
        counted(
            "'u-implicit-zai'",
            "'s-orphan'",
            instant("2026-01-01T13:00:00Z").millis().to_string(),
            "'glm-5.3'",
            "NULL",
            ["500", "100", "50", "0", "600"],
        ),
        counted(
            "'u-zai-haiku'",
            "'s-blank-dir'",
            instant("2026-01-01T14:00:00Z").millis().to_string(),
            HAIKU,
            "' ZAI '",
            ["800", "120", "200", "100", "920"],
        ),
        counted(
            "'u-extra-total'",
            "'s-alpha'",
            instant("2026-01-02T09:00:00Z").millis().to_string(),
            GPT5,
            "NULL",
            ["400", "100", "0", "0", "650"],
        ),
        counted(
            "'u-total-as-output'",
            "'s-alpha'",
            instant("2026-01-02T10:00:00Z").millis().to_string(),
            OPUS,
            "NULL",
            ["300", "0", "0", "50", "420"],
        ),
        counted(
            "'u-total-only'",
            "'s-beta'",
            instant("2026-01-02T11:00:00Z").millis().to_string(),
            GPT5,
            "NULL",
            ["0", "0", "0", "0", "345"],
        ),
        counted(
            "'u-clamped-creation'",
            "'s-beta'",
            instant("2026-01-02T12:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["100", "20", "500", "60", "90"],
        ),
        counted(
            "'u-clamped-read'",
            "'s-beta'",
            instant("2026-01-02T13:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["100", "10", "30", "250", "NULL"],
        ),
        counted(
            "'u-seconds'",
            "'s-beta'",
            instant("2025-12-31T15:00:00Z").secs().to_string(),
            GPT5,
            "NULL",
            ["11", "7", "0", "0", "18"],
        ),
    ]
}

fn ignored_rows() -> Vec<Row> {
    let leak = ["9999", "7777", "0", "0", "17776"];
    let noon = || instant("2025-12-31T12:00:00Z").millis().to_string();
    let row = |id, session, started_at, model, status| Row {
        id,
        session,
        started_at,
        model,
        provider: "NULL",
        status,
        tokens: leak,
    };
    vec![
        row("'x-running'", "'s-alpha'", noon(), SONNET, "'running'"),
        row("'x-failed'", "'s-alpha'", noon(), SONNET, "'failed'"),
        row("'x-status-null'", "'s-alpha'", noon(), SONNET, "NULL"),
        row(
            "'x-status-case'",
            "'s-alpha'",
            noon(),
            SONNET,
            "'Completed'",
        ),
        row(
            "'x-status-padded'",
            "'s-alpha'",
            noon(),
            SONNET,
            "'completed '",
        ),
        row("''", "'s-alpha'", noon(), SONNET, COMPLETED),
        row("'  '", "'s-alpha'", noon(), SONNET, COMPLETED),
        row("NULL", "'s-alpha'", noon(), SONNET, COMPLETED),
        row("'x-session-blank'", "' '", noon(), SONNET, COMPLETED),
        row("'x-session-null'", "NULL", noon(), SONNET, COMPLETED),
        row("'x-model-blank'", "'s-alpha'", noon(), "'  '", COMPLETED),
        row("'x-model-null'", "'s-alpha'", noon(), "NULL", COMPLETED),
        row(
            "'x-time-zero'",
            "'s-alpha'",
            "0".to_string(),
            SONNET,
            COMPLETED,
        ),
        row(
            "'x-time-negative'",
            "'s-alpha'",
            format!("-{}", noon()),
            SONNET,
            COMPLETED,
        ),
        row(
            "'x-time-null'",
            "'s-alpha'",
            "NULL".to_string(),
            SONNET,
            COMPLETED,
        ),
        row(
            "'x-time-words'",
            "'s-alpha'",
            "'yesterday'".to_string(),
            SONNET,
            COMPLETED,
        ),
        Row {
            id: "'x-no-tokens'",
            session: "'s-alpha'",
            started_at: noon(),
            model: SONNET,
            provider: "NULL",
            status: COMPLETED,
            tokens: ["-9999", "-7777", "NULL", "0", "0"],
        },
    ]
}

fn legacy_rows() -> Vec<Row> {
    vec![
        counted(
            "'l-glm'",
            "'s-legacy'",
            instant("2025-12-30T20:00:00Z").millis().to_string(),
            "'GLM-5.2'",
            "NULL",
            ["700", "90", "0", "200", "790"],
        ),
        counted(
            "'l-sonnet'",
            "'s-legacy'",
            instant("2025-12-31T18:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["600", "80", "0", "100", "680"],
        ),
        counted(
            "'u-tokyo'",
            "'s-alpha'",
            instant("2025-12-31T15:30:00.666Z").millis().to_string(),
            "'GLM-5.3'",
            "NULL",
            ["9999", "7777", "0", "0", "17776"],
        ),
        counted(
            "'l-tokyo-fork'",
            "'s-alpha'",
            instant("2026-01-01T02:00:00Z").millis().to_string(),
            "'GLM-5.3'",
            "NULL",
            ["2000", "300", "0", "600", "2300"],
        ),
        Row {
            id: "'l-running'",
            session: "'s-legacy'",
            started_at: instant("2025-12-31T12:00:00Z").millis().to_string(),
            model: SONNET,
            provider: "NULL",
            status: "'running'",
            tokens: ["9999", "7777", "0", "0", "17776"],
        },
    ]
}

fn minimal_rows() -> Vec<Row> {
    vec![
        counted(
            "'m-stale'",
            "'s-minimal'",
            instant("2025-12-27T23:00:00Z").millis().to_string(),
            OPUS,
            "NULL",
            ["150", "25", "0", "0", "0"],
        ),
        counted(
            "'m-last-ms'",
            "'s-minimal'",
            instant("2026-01-01T23:59:59.999Z").millis().to_string(),
            GPT5,
            "NULL",
            ["250", "40", "0", "0", "0"],
        ),
    ]
}

fn leak_row(id: &'static str) -> Row {
    counted(
        id,
        "'s-alpha'",
        instant("2025-12-31T12:00:00Z").millis().to_string(),
        SONNET,
        "NULL",
        ["9999", "7777", "0", "0", "17776"],
    )
}

fn db_path(home: &Path) -> PathBuf {
    home.join("cli/db/db.sqlite")
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("zcode");

    let current = store.join("current");
    let db = create_db(&db_path(&current), Layout::Current);
    db.execute(
        "INSERT INTO session VALUES
             ('s-alpha', '/work/project-alpha', '0.16.3', 'never read'),
             ('s-beta', '/work/project-beta', '0.17.0', NULL),
             ('s-blank-dir', '  ', NULL, NULL)",
    )
    .expect("failed to insert ZCode sessions");
    for row in current_rows().iter().chain(&ignored_rows()) {
        insert(&db, Layout::Current, row);
    }
    drop(db);
    for path in [
        current.join("db.sqlite"),
        current.join("cli/db/archive.sqlite"),
    ] {
        let db = create_db(&path, Layout::Current);
        insert(&db, Layout::Current, &leak_row("'x-wrong-location'"));
    }

    let legacy = store.join("legacy");
    let db = create_db(&db_path(&legacy), Layout::Legacy);
    db.execute(
        "PRAGMA journal_mode = WAL;
         PRAGMA wal_autocheckpoint = 0;
         INSERT INTO session VALUES
             ('s-legacy', '/work/project-legacy'),
             ('s-alpha', '/work/project-alpha-legacy')",
    )
    .expect("failed to insert legacy ZCode sessions");
    for row in &legacy_rows() {
        insert(&db, Layout::Legacy, row);
    }
    // NOTE(2026-11-15): SQLite checkpoints the WAL on the last connection close by default. Keep the fixture connection open so the test reads committed rows from the WAL.
    std::mem::forget(db);

    let minimal = store.join("minimal");
    let db = create_db(&db_path(&minimal), Layout::Minimal);
    db.execute("INSERT INTO session VALUES ('s-minimal', '/work/project-alpha')")
        .expect("failed to insert minimal ZCode session");
    for row in &minimal_rows() {
        insert(&db, Layout::Minimal, row);
    }
    drop(db);

    let unrelated = store.join("unrelated");
    let db = sqlite::open({
        let path = db_path(&unrelated);
        std::fs::create_dir_all(path.parent().expect("database path has a parent"))
            .expect("failed to create ZCode database directory");
        path
    })
    .expect("failed to create unrelated database");
    db.execute(
        "CREATE TABLE unrelated (id TEXT, input_tokens INTEGER);
         INSERT INTO unrelated VALUES ('x-unrelated', 9999);",
    )
    .expect("failed to fill unrelated database");
    drop(db);

    let incomplete = store.join("incomplete");
    let db = create_db(&db_path(&incomplete), Layout::Minimal);
    db.execute(format!(
        "ALTER TABLE model_usage DROP COLUMN status;
         INSERT INTO session VALUES ('s-alpha', '/work/project-alpha');
         INSERT INTO model_usage VALUES ('x-incomplete', 's-alpha', {}, {SONNET}, 9999, 7777);",
        instant("2025-12-31T12:00:00Z").millis(),
    ))
    .expect("failed to fill incomplete database");
    drop(db);

    let no_session = store.join("no-session");
    let db = create_db(&db_path(&no_session), Layout::Current);
    db.execute("DROP TABLE session")
        .expect("failed to drop session table");
    insert(&db, Layout::Current, &leak_row("'x-no-session-table'"));
    drop(db);

    let write = |home: &str, contents: &str| {
        let path = db_path(&store.join(home));
        std::fs::create_dir_all(path.parent().expect("database path has a parent"))
            .expect("failed to create ZCode database directory");
        std::fs::write(path, contents).expect("failed to write ZCode fixture file");
    };
    write("empty", "");
    write("truncated", "SQLite format 3\u{0}truncated");
    std::fs::create_dir_all(db_path(&store.join("dir-db")))
        .expect("failed to create directory in place of the database");
    std::fs::write(store.join("not-a-dir"), "").expect("failed to write file in place of a home");

    let homes = [
        "current",
        "current",
        "legacy",
        "minimal",
        "unrelated",
        "incomplete",
        "no-session",
        "empty",
        "truncated",
        "dir-db",
        "not-a-dir",
        "missing",
    ];
    let entries = homes
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mut entry = OsString::new();
            if index > 0 {
                entry.push(" ");
            }
            entry.push(store.join(name));
            if index + 1 < homes.len() {
                entry.push(" ");
            }
            entry
        })
        .collect::<Vec<_>>();
    vec![("ZCODE_HOME", entries)]
}

pub(crate) fn extra_cases(_root: &Path) -> Vec<Case> {
    let mut cases = vec![
        Case::new("daily", "table_warn", &["zcode", "daily", "--offline"]).env("LOG_LEVEL", "3"),
    ];
    cases.extend(
        ["daily", "monthly", "session"]
            .map(|kind| windowed_table(kind, "zcode", &["--mode", "calculate"])),
    );
    cases
}
