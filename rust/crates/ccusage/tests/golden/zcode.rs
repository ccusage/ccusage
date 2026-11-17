use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::instant;

use crate::golden::Case;

const SONNET: &str = "'claude-sonnet-4-20250514'";
const OPUS: &str = "'claude-opus-4-20250514'";
const HAIKU: &str = "'claude-haiku-4-5-20251001'";
const GPT5: &str = "'gpt-5'";
const UNPRICED: &str = "'zcode-mystery-1'";

const ZAI_PLAN: &str = "'builtin:zai-coding-plan'";
/// An opaque custom-provider id: never treated as Z.ai, so even a model with a
/// bare pricing entry stays unpriced under it.
const CUSTOM_PROVIDER: &str = "'847d13c9-0568-4f2f-818e-8bd498e5d920'";

const COMPLETED: &str = "'completed'";

/// Which optional columns a fixture database has. The adapter probes for each
/// of them separately.
#[derive(Clone, Copy)]
enum Layout {
    /// `provider_id`, both cache columns, `computed_total_tokens`, and
    /// `session.version`, plus columns and a table the adapter never reads.
    Current,
    /// `cache_read_input_tokens` and `computed_total_tokens` only.
    Legacy,
    /// Only the required columns.
    Minimal,
}

/// Every field is a raw SQL literal so rows can hold NULLs and odd types.
/// `tokens` is (input, output, cache creation, cache read, computed total);
/// input is inclusive of both cache slices, as ZCode records it.
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
    // `id` is not a primary key so a replayed usage row can exist in one
    // database.
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
        // REAL timestamp, padded model id, text / REAL / negative / NULL
        // token cells.
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
        // 2026-01-01 in Asia/Tokyo.
        counted(
            "'u-tokyo'",
            "'s-beta'",
            instant("2025-12-31T15:30:00.666Z").millis().to_string(),
            "'GLM-5.3'",
            ZAI_PLAN,
            ["2000", "300", "400", "600", "2300"],
        ),
        // An empty provider is a provider, and not a Z.ai one.
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
        // Same id again: only the first row of an id is kept.
        counted(
            "'u-replayed'",
            "'s-beta'",
            instant("2025-12-31T23:55:00Z").millis().to_string(),
            "'GLM-5.3-Flash'",
            "'account:zai-start-plan'",
            ["9999", "7777", "0", "0", "17776"],
        ),
        // Near miss of the replay: the same usage under another id is counted.
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
        // No provider, lower-case GLM id, and no `session` row to join.
        counted(
            "'u-implicit-zai'",
            "'s-orphan'",
            instant("2026-01-01T13:00:00Z").millis().to_string(),
            "'glm-5.3'",
            "NULL",
            ["500", "100", "50", "0", "600"],
        ),
        // A Z.ai provider with a model that has no `zai/` pricing entry.
        counted(
            "'u-zai-haiku'",
            "'s-blank-dir'",
            instant("2026-01-01T14:00:00Z").millis().to_string(),
            HAIKU,
            "' ZAI '",
            ["800", "120", "200", "100", "920"],
        ),
        // Computed total above the buckets, with output present.
        counted(
            "'u-extra-total'",
            "'s-alpha'",
            instant("2026-01-02T09:00:00Z").millis().to_string(),
            GPT5,
            "NULL",
            ["400", "100", "0", "0", "650"],
        ),
        // Computed total above the buckets, with no output.
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
        // Cache creation larger than what input leaves after cache read, and a
        // computed total below the buckets.
        counted(
            "'u-clamped-creation'",
            "'s-beta'",
            instant("2026-01-02T12:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["100", "20", "500", "60", "90"],
        ),
        // Cache read larger than input.
        counted(
            "'u-clamped-read'",
            "'s-beta'",
            instant("2026-01-02T13:00:00Z").millis().to_string(),
            SONNET,
            "NULL",
            ["100", "10", "30", "250", "NULL"],
        ),
        // Epoch seconds instead of milliseconds: read as a January 1970 instant.
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

/// Rows the adapter must drop. Most carry 9999/7777 so a leak is visible;
/// `x-no-tokens` clamps to zero usage instead.
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
        // No provider column: GLM ids are priced as Z.ai models.
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
        // Same id as a row of the first configured home: that home's row wins.
        counted(
            "'u-tokyo'",
            "'s-alpha'",
            instant("2025-12-31T15:30:00.666Z").millis().to_string(),
            "'GLM-5.3'",
            "NULL",
            ["9999", "7777", "0", "0", "17776"],
        ),
        // Near miss: the usage of `u-tokyo` under another id, in a session id
        // the first home maps to a different directory.
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

pub fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
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
    // Databases the adapter must not discover: outside `cli/db`, and under
    // another file name.
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
    // Closing the last connection would checkpoint the WAL into the database.
    // Keeping it open leaves every row in the WAL, as while ZCode is running.
    std::mem::forget(db);

    let minimal = store.join("minimal");
    let db = create_db(&db_path(&minimal), Layout::Minimal);
    db.execute("INSERT INTO session VALUES ('s-minimal', '/work/project-alpha')")
        .expect("failed to insert minimal ZCode session");
    for row in &minimal_rows() {
        insert(&db, Layout::Minimal, row);
    }
    drop(db);

    // Homes whose database is skipped as a whole.
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

    // A required column (`status`) is missing.
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

    // `ZCODE_HOME` is a comma-separated list whose order decides which copy of
    // a duplicated usage id is kept. `current` is listed twice and `missing`
    // does not exist.
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
    ]
    .into_iter()
    .enumerate()
    .map(|(index, name)| {
        let mut entry = OsString::new();
        if index > 0 {
            entry.push(" ");
        }
        entry.push(store.join(name));
        if index < 11 {
            entry.push(" ");
        }
        entry
    })
    .collect::<Vec<_>>();
    vec![("ZCODE_HOME", homes)]
}

pub fn extra_cases(_root: &Path) -> Vec<Case> {
    vec![Case::new("daily", "table_warn", &["zcode", "daily", "--offline"]).env("LOG_LEVEL", "3")]
}
