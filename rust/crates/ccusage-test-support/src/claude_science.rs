use std::path::Path;

use sqlite::Connection;

/// Creates the schema and representative rows used by Claude Science report
/// tests. The layout mirrors the Claude Science metadata database: one row
/// per conversation frame, with `root_frame_id` grouping sub-agent frames
/// into their parent session.
pub fn create_fixture(path: impl AsRef<Path>) {
    let db = sqlite::open(path).unwrap();
    db.execute(
        "CREATE TABLE frames (
            id TEXT PRIMARY KEY,
            parent_frame_id TEXT,
            root_frame_id TEXT,
            agent_name TEXT NOT NULL,
            status TEXT NOT NULL,
            model TEXT,
            project_id TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_write_tokens INTEGER,
            total_cost REAL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
    )
    .unwrap();
    db.execute("CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO projects VALUES ('proj_alpha', 'alpha'), ('proj_beta', 'beta')")
        .unwrap();
    insert_frame(
        &db,
        FixtureFrame {
            id: "frame-1",
            parent_frame_id: None,
            root_frame_id: "frame-1",
            model: "claude-sonnet-4-5",
            project_id: "proj_alpha",
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 25,
            cache_write_tokens: 15,
            total_cost: 0.5,
            timestamp: "2099-01-02T00:00:00.000Z",
        },
    );
    insert_frame(
        &db,
        FixtureFrame {
            id: "frame-2",
            parent_frame_id: Some("frame-1"),
            root_frame_id: "frame-1",
            model: "cs-switch-direct:claude-sonnet-4-5",
            project_id: "proj_alpha",
            input_tokens: 200,
            output_tokens: 20,
            cache_read_tokens: 40,
            cache_write_tokens: 30,
            total_cost: 1.0,
            timestamp: "2099-01-15T12:00:00.000Z",
        },
    );
    insert_frame(
        &db,
        FixtureFrame {
            id: "frame-3",
            parent_frame_id: None,
            root_frame_id: "frame-3",
            model: "claude-opus-4-6",
            project_id: "proj_beta",
            input_tokens: 50,
            output_tokens: 5,
            cache_read_tokens: 10,
            cache_write_tokens: 0,
            total_cost: 0.25,
            timestamp: "2099-02-01T00:00:00.000Z",
        },
    );
}

/// Describes one completed frame row for a Claude Science fixture.
struct FixtureFrame<'a> {
    id: &'a str,
    parent_frame_id: Option<&'a str>,
    root_frame_id: &'a str,
    model: &'a str,
    project_id: &'a str,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_cost: f64,
    timestamp: &'a str,
}

/// Inserts one completed frame row into a Claude Science fixture database.
fn insert_frame(db: &Connection, frame: FixtureFrame<'_>) {
    let mut statement = db
        .prepare(
            "INSERT INTO frames
             (id, parent_frame_id, root_frame_id, agent_name, status, model, project_id,
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, total_cost,
              created_at, updated_at)
             VALUES
              (?1, ?2, ?3, 'OPERON', 'completed', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        )
        .unwrap();
    let millis = frame
        .timestamp
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond();
    statement.bind((1, frame.id)).unwrap();
    statement
        .bind((
            2,
            frame
                .parent_frame_id
                .map(sqlite::Value::from)
                .unwrap_or(sqlite::Value::Null),
        ))
        .unwrap();
    statement.bind((3, frame.root_frame_id)).unwrap();
    statement.bind((4, frame.model)).unwrap();
    statement.bind((5, frame.project_id)).unwrap();
    statement.bind((6, frame.input_tokens)).unwrap();
    statement.bind((7, frame.output_tokens)).unwrap();
    statement.bind((8, frame.cache_read_tokens)).unwrap();
    statement.bind((9, frame.cache_write_tokens)).unwrap();
    statement.bind((10, frame.total_cost)).unwrap();
    statement.bind((11, millis)).unwrap();
    statement.next().unwrap();
}
