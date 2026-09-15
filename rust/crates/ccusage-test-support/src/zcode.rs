use std::path::Path;

use sqlite::Connection;

/// Describes one completed ZCode usage row for a SQLite fixture.
#[derive(Clone, Copy)]
struct FixtureUsage<'a> {
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

/// Creates the schema and representative rows used by ZCode report tests.
pub fn create_fixture(path: impl AsRef<Path>) {
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
    insert_usage(
        &db,
        FixtureUsage {
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
    insert_usage(
        &db,
        FixtureUsage {
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
    insert_usage(
        &db,
        FixtureUsage {
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

/// Inserts one completed usage row into a ZCode fixture database.
fn insert_usage(db: &Connection, usage: FixtureUsage<'_>) {
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
        .bind((
            3,
            usage
                .timestamp
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_millisecond(),
        ))
        .unwrap();
    statement.bind((4, usage.model)).unwrap();
    statement.bind((5, usage.input_tokens)).unwrap();
    statement.bind((6, usage.output_tokens)).unwrap();
    statement.bind((7, usage.cache_creation_tokens)).unwrap();
    statement.bind((8, usage.cache_read_tokens)).unwrap();
    statement.bind((9, usage.computed_total_tokens)).unwrap();
    statement.next().unwrap();
}
