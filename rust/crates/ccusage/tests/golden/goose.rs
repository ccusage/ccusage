use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::instant;

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "goose-mystery-1";
const PROVIDER_PREFIXED: &str = "qwen/qwen3-coder";

const SESSIONS_SCHEMA: &str = r"
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT,
    working_dir TEXT,
    model_config_json TEXT,
    provider_name TEXT,
    created_at TIMESTAMP,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    accumulated_total_tokens INTEGER,
    accumulated_input_tokens INTEGER,
    accumulated_output_tokens INTEGER
);
";

struct Row {
    id: &'static str,
    model_config: String,
    provider: &'static str,
    created_at: String,
    tokens: [&'static str; 6],
}

fn model(name: &str) -> String {
    format!(r#"'{{"model_name":"{name}","context_limit":200000}}'"#)
}

fn insert(db: &sqlite::Connection, row: &Row) {
    let [total, input, output, acc_total, acc_input, acc_output] = row.tokens;
    db.execute(format!(
        "INSERT INTO sessions (id, name, working_dir, model_config_json, provider_name, \
         created_at, total_tokens, input_tokens, output_tokens, accumulated_total_tokens, \
         accumulated_input_tokens, accumulated_output_tokens) VALUES ('{}', 'fixture', \
         '/work/project', {}, {}, {}, {total}, {input}, {output}, {acc_total}, {acc_input}, \
         {acc_output});",
        row.id, row.model_config, row.provider, row.created_at,
    ))
    .expect("failed to insert Goose session row");
}

fn counted_rows() -> Vec<Row> {
    vec![
        Row {
            id: "s-stale",
            model_config: model(SONNET),
            provider: "'anthropic'",
            created_at: "'2025-12-27 22:00:00'".into(),
            tokens: ["NULL", "NULL", "NULL", "180", "100", "50"],
        },
        Row {
            id: "s-before",
            model_config: model("gpt-5"),
            provider: "'openai'",
            created_at: "'2025-12-30T10:00:00.000Z'".into(),
            tokens: ["NULL", "NULL", "NULL", "NULL", "1000", "200"],
        },
        Row {
            id: "s-tokyo",
            model_config: model(OPUS),
            provider: "NULL",
            created_at: instant("2025-12-31T15:30:00Z").secs().to_string(),
            tokens: ["2500", "2000", "300", "NULL", "NULL", "NULL"],
        },
        Row {
            id: "s-replayed",
            model_config: model(HAIKU),
            provider: "'anthropic'",
            created_at: "'2025-12-31T23:50:00'".into(),
            tokens: ["NULL", "NULL", "NULL", "1300", "900", "400"],
        },
        Row {
            id: "s-replayed-fork",
            model_config: model(HAIKU),
            provider: "'anthropic'",
            created_at: instant("2026-01-01T00:10:00Z").millis().to_string(),
            tokens: ["1350", "900", "8888", "0", "-1", "400"],
        },
        Row {
            id: "s-mystery",
            model_config: model(UNPRICED),
            provider: "NULL",
            created_at: "'2026-01-01 12:00:00'".into(),
            tokens: ["NULL", "NULL", "NULL", "370", "300", "70"],
        },
        Row {
            id: "s-prefixed",
            model_config: model(PROVIDER_PREFIXED),
            provider: "NULL",
            created_at: "'2026-01-01T13:00:00+09:00'".into(),
            tokens: ["NULL", "NULL", "NULL", "600", "500", "100"],
        },
        Row {
            id: "s-total-only",
            model_config: model("gpt-5"),
            provider: "'  '",
            created_at: "'2026-01-02'".into(),
            tokens: ["NULL", "NULL", "NULL", "345", "NULL", "NULL"],
        },
        Row {
            id: "s-total-short",
            model_config: model(SONNET),
            provider: "'azure-openai'",
            created_at: "'2026-01-02 10:00:00'".into(),
            tokens: ["NULL", "NULL", "NULL", "500", "800", "300"],
        },
    ]
}

fn ignored_rows() -> Vec<Row> {
    let leak = ["NULL", "NULL", "NULL", "17776", "9999", "7777"];
    let ignored = |id, model_config: String, created_at: &str| Row {
        id,
        model_config,
        provider: "'anthropic'",
        created_at: created_at.into(),
        tokens: leak,
    };
    vec![
        ignored("x-config-null", "NULL".to_string(), "'2025-12-31 12:00:00'"),
        ignored(
            "x-config-blank",
            "'   '".to_string(),
            "'2025-12-31 12:00:00'",
        ),
        ignored(
            "x-config-truncated",
            format!(r#"'{{"model_name":"{SONNET}'"#),
            "'2025-12-31 12:00:00'",
        ),
        ignored(
            "x-config-no-model",
            r#"'{"context_limit":200000}'"#.to_string(),
            "'2025-12-31 12:00:00'",
        ),
        ignored(
            "x-config-empty-model",
            r#"'{"model_name":""}'"#.to_string(),
            "'2025-12-31 12:00:00'",
        ),
        ignored(
            "x-config-numeric-model",
            r#"'{"model_name":42}'"#.to_string(),
            "'2025-12-31 12:00:00'",
        ),
        ignored("x-time-null", model(SONNET), "NULL"),
        ignored("x-time-blank", model(SONNET), "''"),
        ignored("x-time-zero", model(SONNET), "0"),
        ignored("x-time-words", model(SONNET), "'yesterday'"),
        ignored("x-time-slashes", model(SONNET), "'2025/12/31 12:00:00'"),
        Row {
            id: "x-no-tokens",
            model_config: model(SONNET),
            provider: "'anthropic'",
            created_at: "'2025-12-31 12:00:00'".into(),
            tokens: ["0", "-9999", "NULL", "0", "0", "-7777"],
        },
    ]
}

fn create_db(path: &Path, rows: &[Row]) {
    let db = sqlite::open(path).expect("failed to create Goose database");
    db.execute(SESSIONS_SCHEMA)
        .expect("failed to create Goose sessions table");
    db.execute(
        "CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, tokens INTEGER);
         INSERT INTO messages (session_id, tokens) VALUES ('s-before', 9999);
         CREATE TABLE schema_version (version INTEGER);
         INSERT INTO schema_version (version) VALUES (7);",
    )
    .expect("failed to create extra Goose tables");
    for row in rows {
        insert(&db, row);
    }
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("goose");
    let sessions = store.join("data/sessions");
    std::fs::create_dir_all(&sessions).expect("failed to create Goose sessions directory");

    let mut rows = counted_rows();
    rows.extend(ignored_rows());
    create_db(&sessions.join("sessions.db"), &rows);

    let leak = Row {
        id: "x-wrong-location",
        model_config: model(SONNET),
        provider: "'anthropic'",
        created_at: "'2025-12-31 12:00:00'".into(),
        tokens: ["NULL", "NULL", "NULL", "17776", "9999", "7777"],
    };
    create_db(&store.join("sessions.db"), std::slice::from_ref(&leak));
    create_db(&sessions.join("archive.db"), std::slice::from_ref(&leak));

    let write = |name: &str, contents: &str| {
        std::fs::write(sessions.join(name), contents).expect("failed to write Goose fixture file");
    };
    write("empty.db", "");
    write("sessions.db.bak", "SQLite format 3\u{0}truncated");
    write(
        "20251231_120000.jsonl",
        &format!(
            r#"{{"working_dir":"/work/project","total_tokens":17776,"input_tokens":9999,"output_tokens":7777,"model_config":{{"model_name":"{SONNET}"}}}}
{{"role":"assistant","created":{created},"content":[{{"type":"text","te"#,
            created = instant("2025-12-31T12:00:00Z").secs()
        ),
    );

    vec![("GOOSE_PATH_ROOT", vec![store.into_os_string()])]
}
