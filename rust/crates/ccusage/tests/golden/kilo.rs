use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::instant;

use serde_json::{Value, json};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const GPT5: &str = "gpt-5";
const UNPRICED: &str = "kilo-mystery-1";

/// `(message.id, message.session_id, message.data)`; a `None` is stored as SQL NULL.
type Row = (Option<&'static str>, &'static str, Option<String>);

fn row(id: &'static str, session_id: &'static str, data: &Value) -> Row {
    (Some(id), session_id, Some(data.to_string()))
}

fn create_db(path: &Path, rows: &[Row]) -> sqlite::Connection {
    std::fs::create_dir_all(path.parent().expect("Kilo database path has a parent"))
        .expect("failed to create Kilo data directory");
    let db = sqlite::open(path).expect("failed to create Kilo database");
    // No primary key on purpose: the adapter never relies on one, and the
    // fixture repeats row ids to reach the fallback dedupe key.
    db.execute(
        "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    )
    .expect("failed to create Kilo message table");
    for (id, session_id, data) in rows {
        let mut statement = db
            .prepare("INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, 0, 0, ?3)")
            .expect("failed to prepare Kilo message insert");
        statement.bind((1, *id)).expect("failed to bind Kilo id");
        statement
            .bind((2, *session_id))
            .expect("failed to bind Kilo session id");
        statement
            .bind((3, data.as_deref()))
            .expect("failed to bind Kilo data");
        statement.next().expect("failed to insert Kilo message");
    }
    db
}

fn primary_rows() -> Vec<Row> {
    let counted = vec![
        // Stale day, in an earlier week and (in Tokyo) on 2025-12-28.
        row(
            "row-stale",
            "ses-gamma",
            &json!({"id":"msg-stale","role":"assistant","providerID":"anthropic","modelID":SONNET,"time":{"created":instant("2025-12-27T22:00:00Z").millis()},"tokens":{"input":100,"output":50,"cache":{"read":0,"write":0}},"cost":0.01}),
        ),
        // Stored cost that disagrees with the token-derived one, plus
        // reasoning tokens, which count towards the total only.
        row(
            "row-a1",
            "ses-alpha",
            &json!({"id":"msg-a1","role":"assistant","providerID":"anthropic","modelID":SONNET,"time":{"created":instant("2025-12-30T10:00:00Z").millis(),"completed":instant("2025-12-30T10:01:00Z").millis()},"tokens":{"input":1000,"output":200,"reasoning":50,"cache":{"read":4000,"write":300}},"cost":0.5,"agent":"build","mode":"code"}),
        ),
        // Timestamp in seconds rather than milliseconds, and no stored cost.
        row(
            "row-a2",
            "ses-alpha",
            &json!({"id":"msg-a2","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2025-12-31T15:30:00Z").secs()},"tokens":{"input":700,"output":150,"cache":{"read":2500,"write":0}}}),
        ),
        // Replay of msg-a2 under another row id: same embedded id, so dropped.
        row(
            "row-a2-replay",
            "ses-alpha",
            &json!({"id":"msg-a2","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2025-12-31T15:31:00Z").secs()},"tokens":{"input":9999,"output":9999,"cache":{"read":9999,"write":9999}},"cost":99.0}),
        ),
        row(
            "row-b1",
            "ses-beta",
            &json!({"id":"msg-b1","role":"assistant","providerID":"anthropic","modelID":HAIKU,"time":{"created":instant("2025-12-31T23:50:00Z").millis()},"tokens":{"input":900,"output":400,"cache":{"read":7000,"write":200}},"cost":2.0}),
        ),
        // Stored cost of exactly zero on a priced model.
        row(
            "row-b2",
            "ses-beta",
            &json!({"id":"msg-b2","role":"assistant","providerID":"anthropic","modelID":SONNET,"time":{"created":instant("2026-01-01T00:10:00Z").millis()},"tokens":{"input":600,"output":100,"cache":{"read":3000,"write":0}},"cost":0}),
        ),
        row(
            "row-a3",
            "ses-alpha",
            &json!({"id":"msg-a3","role":"assistant","providerID":"kilo","modelID":UNPRICED,"time":{"created":instant("2026-01-01T12:00:00Z").millis()},"tokens":{"input":300,"output":70}}),
        ),
        row(
            "row-a4",
            "ses-alpha",
            &json!({"id":"msg-a4","role":"assistant","providerID":"kilo","modelID":UNPRICED,"time":{"created":instant("2026-01-01T12:10:00Z").millis()},"tokens":{"input":30,"output":7},"cost":0.2}),
        ),
        // Only a total: it becomes output tokens.
        row(
            "row-a5",
            "ses-alpha",
            &json!({"id":"msg-a5","role":"assistant","providerID":"azure","modelID":GPT5,"time":{"created":instant("2026-01-01T12:30:00Z").millis()},"tokens":{"total":345}}),
        ),
        // A total larger than its parts: the remainder is extra total tokens.
        row(
            "row-a6",
            "ses-alpha",
            &json!({"id":"msg-a6","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2026-01-01T13:00:00Z").millis()},"tokens":{"input":100,"output":20,"total":200}}),
        ),
        // `session_id` inside the payload wins over the row's column.
        row(
            "row-b3",
            "ses-beta",
            &json!({"id":"msg-b3","session_id":"ses-delta","role":"assistant","providerID":"vertex-ai","modelID":OPUS,"time":{"created":instant("2026-01-02T10:00:00Z").millis()},"tokens":{"input":800,"output":300,"cache":{"read":6000,"write":50}},"cost":1.25}),
        ),
        // Upstream's camelCase `sessionID` is not read; the row's column is.
        // The model id is the gateway-style one Kilo's own provider reports.
        row(
            "row-b4",
            "ses-beta",
            &json!({"id":"msg-b4","sessionID":"ses-camel-ignored","role":"assistant","providerID":"kilo","modelID":"anthropic/claude-sonnet-4","time":{"created":instant("2026-01-02T11:00:00Z").millis()},"tokens":{"input":500,"output":100}}),
        ),
        // Wrongly typed fields: cache is not an object, reasoning and cost are
        // strings. The record survives with those fields read as absent.
        row(
            "row-b5",
            "ses-beta",
            &json!({"id":"msg-b5","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2026-01-02T12:00:00Z").millis()},"tokens":{"input":40,"output":10,"cache":0,"reasoning":"7777"},"cost":"9.99"}),
        ),
        // No embedded id: the dedupe key falls back to `<db path>:<row id>`.
        row(
            "row-noid",
            "ses-beta",
            &json!({"role":"assistant","providerID":"anthropic","modelID":HAIKU,"time":{"created":instant("2025-12-30T11:00:00Z").millis()},"tokens":{"input":11,"output":4}}),
        ),
        // Same row id in the same database, still no embedded id: dropped.
        row(
            "row-noid",
            "ses-beta",
            &json!({"role":"assistant","providerID":"anthropic","modelID":HAIKU,"time":{"created":instant("2025-12-30T11:01:00Z").millis()},"tokens":{"input":9999,"output":9999}}),
        ),
        // NULL row id with an otherwise valid payload.
        (
            None,
            "ses-alpha",
            Some(
                json!({"id":"msg-null-row-id","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2026-01-01T14:00:00Z").millis()},"tokens":{"input":444,"output":111}})
                    .to_string(),
            ),
        ),
    ];

    let ignored_time = instant("2025-12-31T12:00:00Z").millis();
    let ignored = vec![
        row(
            "row-user",
            "ses-alpha",
            &json!({"id":"msg-user","role":"user","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"tokens":{"input":7777,"output":7777}}),
        ),
        row(
            "row-no-role",
            "ses-alpha",
            &json!({"id":"msg-no-role","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"tokens":{"input":7777,"output":7777}}),
        ),
        row(
            "row-no-tokens",
            "ses-alpha",
            &json!({"id":"msg-no-tokens","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"cost":77.0}),
        ),
        row(
            "row-scalar-tokens",
            "ses-alpha",
            &json!({"id":"msg-scalar-tokens","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"tokens":7777,"cost":77.0}),
        ),
        row(
            "row-zero-tokens",
            "ses-alpha",
            &json!({"id":"msg-zero-tokens","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"tokens":{"input":0,"output":0,"cache":{"read":0,"write":0}},"cost":77.0}),
        ),
        row(
            "row-string-tokens",
            "ses-alpha",
            &json!({"id":"msg-string-tokens","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":ignored_time},"tokens":{"input":"7777","output":"7777"}}),
        ),
        row(
            "row-no-model",
            "ses-alpha",
            &json!({"id":"msg-no-model","role":"assistant","providerID":"openai","time":{"created":ignored_time},"tokens":{"input":6666,"output":6666}}),
        ),
        row(
            "row-empty-model",
            "ses-alpha",
            &json!({"id":"msg-empty-model","role":"assistant","providerID":"openai","modelID":"","time":{"created":ignored_time},"tokens":{"input":6666,"output":6666}}),
        ),
        row(
            "row-no-time",
            "ses-alpha",
            &json!({"id":"msg-no-time","role":"assistant","providerID":"openai","modelID":GPT5,"tokens":{"input":8888,"output":8888}}),
        ),
        row(
            "row-zero-time",
            "ses-alpha",
            &json!({"id":"msg-zero-time","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":0},"tokens":{"input":8888,"output":8888}}),
        ),
        row(
            "row-negative-time",
            "ses-alpha",
            &json!({"id":"msg-negative-time","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":-ignored_time},"tokens":{"input":8888,"output":8888}}),
        ),
        row(
            "row-string-time",
            "ses-alpha",
            &json!({"id":"msg-string-time","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":"2025-12-31T12:00:00.000Z"},"tokens":{"input":8888,"output":8888}}),
        ),
        row(
            "row-float-time",
            "ses-alpha",
            &json!({"id":"msg-float-time","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2025-12-31T12:00:00.0005Z").millis_f64()},"tokens":{"input":8888,"output":8888}}),
        ),
        // Unknown record shape.
        row(
            "row-unknown-shape",
            "ses-alpha",
            &json!({"type":"compaction","summary":{"role":"assistant","modelID":GPT5,"time":{"created":ignored_time},"tokens":{"input":5555,"output":5555}}}),
        ),
        (
            Some("row-truncated"),
            "ses-alpha",
            Some(format!(
                r#"{{"id":"msg-truncated","role":"assistant","providerID":"openai","modelID":"{GPT5}","time":{{"created":{ignored_time}}},"tokens":{{"input":9999,"outp"#
            )),
        ),
        (Some("row-empty-data"), "ses-alpha", Some(String::new())),
        (Some("row-null-data"), "ses-alpha", None),
        (Some("row-array-data"), "ses-alpha", Some("[]".to_string())),
    ];

    counted.into_iter().chain(ignored).collect()
}

fn secondary_rows() -> Vec<Row> {
    vec![
        // Replay of the primary store's msg-a1 in a second data directory.
        row(
            "row-a1-copy",
            "ses-alpha",
            &json!({"id":"msg-a1","role":"assistant","providerID":"anthropic","modelID":SONNET,"time":{"created":instant("2025-12-30T10:00:00Z").millis()},"tokens":{"input":9999,"output":9999,"cache":{"read":9999,"write":9999}},"cost":99.0}),
        ),
        // Near misses, both counted: the primary's row id with another embedded
        // id, and the primary's id-less row id under another database path.
        row(
            "row-a1",
            "ses-alpha",
            &json!({"id":"msg-a1-fork","role":"assistant","providerID":"anthropic","modelID":SONNET,"time":{"created":instant("2025-12-30T10:05:00Z").millis()},"tokens":{"input":60,"output":15}}),
        ),
        row(
            "row-noid",
            "ses-beta",
            &json!({"role":"assistant","providerID":"anthropic","modelID":HAIKU,"time":{"created":instant("2025-12-30T11:00:00Z").millis()},"tokens":{"input":22,"output":8}}),
        ),
        // One second before the Asia/Tokyo day boundary.
        row(
            "row-s1",
            "ses-alpha",
            &json!({"id":"msg-s1","role":"assistant","providerID":"openai","modelID":GPT5,"time":{"created":instant("2025-12-31T15:00:00Z").millis() - 1000},"tokens":{"input":200,"output":40},"cost":0.03}),
        ),
    ]
}

pub fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("kilo");
    let db_file = |name: &str| store.join(name).join("kilo.db");

    let primary = create_db(&db_file("primary"), &primary_rows());
    // A non-text payload, and tables the adapter does not read.
    primary
        .execute(
            format!("INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('row-integer-data', 'ses-alpha', 0, 0, 7777);
             CREATE TABLE session (id TEXT, title TEXT, data TEXT);
             INSERT INTO session VALUES ('ses-alpha', 'Alpha', '{{\"role\":\"assistant\",\"modelID\":\"gpt-5\",\"time\":{{\"created\":{fixture_time_0}}},\"tokens\":{{\"input\":5555,\"output\":5555}}}}');
             CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT);
             INSERT INTO part VALUES ('part-1', 'row-a1', 'ses-alpha', '{{\"type\":\"step-finish\",\"tokens\":{{\"input\":5555,\"output\":5555}},\"cost\":55.0}}');", fixture_time_0 = instant("2025-12-31T12:00:00Z").millis()),
        )
        .expect("failed to add extra Kilo rows and tables");
    drop(primary);

    drop(create_db(&db_file("secondary"), &secondary_rows()));

    // A valid database without the `message` table.
    let no_table = db_file("no-table");
    std::fs::create_dir_all(no_table.parent().expect("Kilo database path has a parent"))
        .expect("failed to create Kilo data directory");
    sqlite::open(&no_table)
        .expect("failed to create Kilo database")
        .execute(
            format!("CREATE TABLE messages (id TEXT, session_id TEXT, data TEXT);
             INSERT INTO messages VALUES ('row-wrong-table', 'ses-alpha', '{{\"id\":\"msg-wrong-table\",\"role\":\"assistant\",\"modelID\":\"gpt-5\",\"time\":{{\"created\":{fixture_time_0}}},\"tokens\":{{\"input\":5555,\"output\":5555}}}}');", fixture_time_0 = instant("2025-12-31T12:00:00Z").millis()),
        )
        .expect("failed to create table in Kilo database");

    for (name, contents) in [
        ("empty-db", ""),
        ("corrupt-db", "SQLite format 3 but truncated\n"),
    ] {
        let path = db_file(name);
        std::fs::create_dir_all(path.parent().expect("Kilo database path has a parent"))
            .expect("failed to create Kilo data directory");
        std::fs::write(path, contents).expect("failed to write Kilo database file");
    }
    std::fs::create_dir_all(store.join("no-db")).expect("failed to create Kilo data directory");
    std::fs::write(store.join("no-db/notes.txt"), "not a database\n")
        .expect("failed to write Kilo note file");

    // KILO_DATA_DIR is a comma-separated list. The primary store is listed
    // twice and `missing` does not exist.
    let data_dirs = [
        "primary",
        "secondary",
        "primary",
        "no-table",
        "empty-db",
        "corrupt-db",
        "no-db",
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
        entry
    })
    .chain([OsString::new(), OsString::new()])
    .collect::<Vec<_>>();
    vec![("KILO_DATA_DIR", data_dirs)]
}
