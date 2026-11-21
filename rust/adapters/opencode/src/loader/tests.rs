use std::path::Path;

use super::{LinkedEntry, load_directories};
use crate::{
    LoadedEntry,
    cli::{AgentReportKind, CostMode, SharedArgs},
};
use ccusage_core::Diagnostics;
use ccusage_test_support::{fs_fixture, read_fixture_dir};

fn load_linked(
    directories: &[&Path],
    shared: &SharedArgs,
    report_kind: AgentReportKind,
) -> Vec<LinkedEntry> {
    let directories = directories
        .iter()
        .map(|directory| directory.to_path_buf())
        .collect::<Vec<_>>();
    load_directories(
        &directories,
        shared,
        report_kind,
        &Diagnostics::new("opencode"),
    )
}

fn load_entries_from_directory_for_report(
    directory: &Path,
    shared: &SharedArgs,
    report_kind: AgentReportKind,
) -> Vec<LoadedEntry> {
    load_linked(&[directory], shared, report_kind)
        .into_iter()
        .map(|(entry, _)| entry)
        .collect()
}

fn load_entries_from_directory(directory: &Path, shared: &SharedArgs) -> Vec<LoadedEntry> {
    load_entries_from_directory_for_report(directory, shared, AgentReportKind::Daily)
}

#[test]
fn reports_failed_schema_probes_without_database_details() {
    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = ccusage_test_support::Fixture::new();
    let path = fixture.path("opencode.db");
    let connection = sqlite::open(&path).unwrap();
    connection
        .execute("CREATE TABLE session_message (id TEXT, session_id TEXT, seq INTEGER)")
        .unwrap();
    let diagnostics = Diagnostics::new("opencode");
    assert!(super::schema::table_exists(
        &connection,
        super::Table::SessionMessage,
        &path,
        &diagnostics,
    ));
    connection.execute("DROP TABLE session_message").unwrap();
    assert!(
        super::TableSchema::read(
            &connection,
            super::Table::SessionMessage,
            &path,
            &diagnostics
        )
        .is_none()
    );
    assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    let warnings = diagnostics
        .warnings(&SharedArgs {
            debug: true,
            ..Default::default()
        })
        .join("\n");
    assert!(warnings.contains("database schema is not supported"));
    assert!(!warnings.contains("no such table"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_message_directory_is_reported() {
    use std::{fs, os::unix::fs::PermissionsExt};

    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = ccusage_test_support::fs_fixture!({
        "storage/message/blocked/m.json": "{}",
    });
    let blocked = fixture.path("storage/message/blocked");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = read_fixture_dir(&blocked).is_err();
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        ..Default::default()
    };
    let diagnostics = Diagnostics::new("opencode");
    load_directories(
        &[fixture.root().to_path_buf()],
        &shared,
        AgentReportKind::Daily,
        &diagnostics,
    );
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    if !denied {
        return;
    }

    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
}

#[cfg(unix)]
#[test]
fn a_data_directory_that_cannot_be_listed_for_databases_is_reported() {
    use std::{fs, os::unix::fs::PermissionsExt};

    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = ccusage_test_support::fs_fixture!({
        "data/storage/message/s/m.json": "{}",
    });
    let data = fixture.path("data");
    fs::set_permissions(&data, fs::Permissions::from_mode(0o300)).unwrap();
    let denied = read_fixture_dir(&data).is_err();
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        ..Default::default()
    };
    let diagnostics = Diagnostics::new("opencode");
    load_directories(
        std::slice::from_ref(&data),
        &shared,
        AgentReportKind::Daily,
        &diagnostics,
    );
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    if !denied {
        return;
    }

    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
}

#[test]
fn specialized_diagnostics_keep_healthy_database_rows() {
    let fixture = ccusage_test_support::Fixture::new();
    let db = fixture.path("opencode.db");
    create_db_message(
        &db,
        "good",
        "s",
        r#"{"modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#,
    );
    create_db_message_with_time(&db, "bad", "s", 1767312000000, "{SECRET_CANARY");
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        debug: true,
        ..Default::default()
    };
    let diagnostics = ccusage_core::Diagnostics::new("opencode");
    let loaded = load_directories(
        &[fixture.root().to_path_buf()],
        &shared,
        AgentReportKind::Daily,
        &diagnostics,
    );
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].0.usage.input_tokens, 10);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    assert!(
        !diagnostics
            .warnings(&shared)
            .join("\n")
            .contains("SECRET_CANARY")
    );
}

#[test]
fn names_a_session_message_table_without_usage_columns_an_unsupported_schema() {
    use ccusage_core::SkipKind;

    let fixture = ccusage_test_support::Fixture::new();
    let db = fixture.path("opencode.db");
    create_db_message(
        &db,
        "good",
        "s",
        r#"{"modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#,
    );
    sqlite::open(&db)
        .unwrap()
        .execute("CREATE TABLE session_message (id TEXT, session_id TEXT, seq INTEGER)")
        .unwrap();
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        debug: true,
        ..Default::default()
    };
    let diagnostics = ccusage_core::Diagnostics::new("opencode");

    let loaded = load_directories(
        &[fixture.root().to_path_buf()],
        &shared,
        AgentReportKind::Daily,
        &diagnostics,
    );

    assert_eq!(loaded.len(), 1);
    assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
    assert!(
        diagnostics
            .warnings(&shared)
            .join("\n")
            .contains("opencode.db: database schema is not supported")
    );
}

#[test]
fn specialized_diagnostics_leave_optional_sources_and_nonusage_quiet() {
    let fixture = ccusage_test_support::Fixture::new();
    sqlite::open(fixture.path("opencode.db"))
        .unwrap()
        .execute("CREATE TABLE session_v2 (id TEXT PRIMARY KEY)")
        .unwrap();
    fixture.write_file(
        "storage/message/user.json",
        r#"{"role":"user","content":"SECRET_CANARY"}"#,
    );
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        debug: true,
        ..Default::default()
    };
    let diagnostics = ccusage_core::Diagnostics::new("opencode");
    let loaded = load_directories(
        &[fixture.root().to_path_buf()],
        &shared,
        AgentReportKind::Session,
        &diagnostics,
    );
    assert!(loaded.is_empty());
    assert!(diagnostics.warnings(&shared).is_empty());
}

#[test]
fn reports_aggregate_schema_read_failures() {
    use super::{
        DatabaseLoadContext, DateWindow, Table, TableSchema, load_session_aggregate_entries,
    };
    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = ccusage_test_support::Fixture::new();
    let path = fixture.path("opencode.db");
    create_db_session_aggregate_table(&path, "session_v2");
    let connection = sqlite::open(&path).unwrap();
    let diagnostics = Diagnostics::new("opencode");
    let schema = TableSchema::probe(&connection, Table::SessionV2, &path, &diagnostics).unwrap();
    connection.execute("DROP TABLE session_v2").unwrap();
    let shared = SharedArgs::default();
    let context = DatabaseLoadContext {
        tz: &jiff::tz::TimeZone::UTC,
        mode: CostMode::Display,
        pricing: ccusage_core::cost::PriceTable::Display,
        window: DateWindow::UNBOUNDED,
        shared: &shared,
        db_path: &path,
        diagnostics: &diagnostics,
    };

    assert!(load_session_aggregate_entries(&connection, &schema, &context).is_empty());
    assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
}

#[test]
fn skips_usage_without_a_creation_time_in_every_format() {
    const UNTIMED: &str = r#"{"modelID":"gpt-test","providerID":"openai","model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":10,"output":1},"cost":0.01}"#;
    let fixture = fs_fixture!({
        "storage/message/file-session/file-untimed.json": r#"{"id":"file-untimed","sessionID":"file-session","modelID":"gpt-test","providerID":"openai","time":{"created":0},"tokens":{"input":10,"output":1},"cost":0.01}"#,
    });
    let db = fixture.path("opencode.db");
    create_db_message(
        &db,
        "timed",
        "timed-session",
        &assistant_payload(10, 1, 0.01).replace(
            r#""model":{"id":"gpt-test","providerID":"openai"},"#,
            r#""modelID":"gpt-test","providerID":"openai","#,
        ),
    );
    create_db_message_with_time(&db, "db-untimed", "db-session", 0, UNTIMED);
    create_db_session_message_table(&db, true);
    insert_db_session_message(
        &db,
        "v2-timed",
        "v2-session",
        "assistant",
        1_767_312_000_000,
        assistant_payload(10, 1, 0.01).as_str(),
        true,
    );
    insert_db_session_message(
        &db,
        "v2-untimed",
        "v2-session",
        "assistant",
        0,
        UNTIMED,
        false,
    );
    create_db_session_aggregate_table(&db, "session_v2");
    insert_db_session_aggregate(
        &db,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "aggregate-session",
            time_created: 0,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 0.5,
            input_tokens: 200,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );
    for (since, until, undatable) in [(None, None, 4), (Some("20260102"), Some("20260102"), 3)] {
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: since.map(|date| date.parse().unwrap()),
            until: until.map(|date| date.parse().unwrap()),
            ..SharedArgs::default()
        };
        let diagnostics = Diagnostics::new("opencode");

        let entries = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Session,
            &diagnostics,
        );

        let ids = entries
            .iter()
            .map(|(entry, _)| entry.message_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(ids, [Some("timed"), Some("v2-timed")], "{since:?}");
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            undatable,
            "{since:?}"
        );
        assert_eq!(
            diagnostics.warnings(&shared),
            [format!(
                "WARN  opencode: skipped 1 input and {} records in 2 inputs (invalid timestamp)",
                undatable - 1
            )]
        );
    }
}

#[test]
fn untimed_rows_in_two_tables_count_as_one_database_input() {
    let fixture = ccusage_test_support::Fixture::new();
    let db = fixture.path("opencode.db");
    const UNTIMED: &str = r#"{"modelID":"gpt-test","providerID":"openai","model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":10,"output":1},"cost":0.01}"#;
    create_db_message_with_time(&db, "legacy", "session", 0, UNTIMED);
    create_db_session_message_table(&db, true);
    insert_db_session_message(&db, "current", "session", "assistant", 0, UNTIMED, false);
    let shared = SharedArgs {
        mode: CostMode::Display,
        ..SharedArgs::default()
    };
    let diagnostics = Diagnostics::new("opencode");
    assert!(
        load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Daily,
            &diagnostics
        )
        .is_empty()
    );
    assert_eq!(
        diagnostics.warnings(&shared),
        ["WARN  opencode: skipped 2 records in 1 input (invalid timestamp)"]
    );
    let samples = diagnostics.warnings(&SharedArgs {
        debug: true,
        ..shared
    });
    assert!(samples.iter().any(|line| line.contains(":message:legacy:")));
    assert!(
        samples
            .iter()
            .any(|line| line.contains(":session_message:current:"))
    );
}

#[test]
fn specialized_diagnostics_distinguish_v2_usage_from_suppressed_rows() {
    let fixture = ccusage_test_support::Fixture::new();
    let db = fixture.path("opencode.db");
    create_db_session_message_table(&db, true);
    for (id, kind, data) in [
        (
            "good",
            "assistant",
            r#"{"modelID":"claude-opus-5","providerID":"anthropic","tokens":{"input":10,"output":1}}"#,
        ),
        ("bad", "assistant", "{SECRET_CANARY"),
        ("user", "user", "{SECRET_CANARY"),
        (
            "failed",
            "compaction",
            r#"{"status":"failed","tokens":{"input":10}}"#,
        ),
        ("pending", "assistant", r#"{"tokens":null}"#),
    ] {
        insert_db_session_message(&db, id, "s", kind, 1767312000000, data, true);
    }
    let shared = SharedArgs {
        offline: true,
        mode: CostMode::Display,
        debug: true,
        ..Default::default()
    };
    let diagnostics = ccusage_core::Diagnostics::new("opencode");
    let loaded = load_directories(
        &[fixture.root().to_path_buf()],
        &shared,
        AgentReportKind::Daily,
        &diagnostics,
    );
    assert_eq!(loaded.len(), 1);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
    assert!(
        !diagnostics
            .warnings(&shared)
            .join("\n")
            .contains("SECRET_CANARY")
    );
}

pub(super) fn fill_session_messages(path: &Path, rows: usize) {
    let db = sqlite::open(path).unwrap();
    db.execute("BEGIN").unwrap();
    let mut statement = db
        .prepare(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .unwrap();
    for index in 0..rows {
        let created = 1_767_312_000_000_i64 + i64::try_from(index % 7).unwrap();
        let (kind, data) = match index % 9 {
            0 => ("assistant", "{broken".to_string()),
            1 => (
                "assistant",
                r#"{"model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":3,"output":1}}"#
                    .to_string(),
            ),
            2 => ("user", r#"{"text":"hi"}"#.to_string()),
            3 => (
                "compaction",
                format!(
                    r#"{{"status":"completed","model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":{created}}},"tokens":{{"input":5,"output":2}}}}"#
                ),
            ),
            _ => (
                "assistant",
                assistant_payload(u64::try_from(index).unwrap(), 1, 0.01)
                    .replace("1767312000000", &created.to_string()),
            ),
        };
        statement.reset().unwrap();
        statement.bind((1, format!("m{index}").as_str())).unwrap();
        statement
            .bind((2, format!("s{}", index % 5).as_str()))
            .unwrap();
        statement.bind((3, kind)).unwrap();
        statement.bind((4, i64::try_from(index).unwrap())).unwrap();
        statement.bind((5, created)).unwrap();
        statement.bind((6, data.as_str())).unwrap();
        statement.next().unwrap();
    }
    drop(statement);
    db.execute("COMMIT").unwrap();
}

fn create_db_message(path: &Path, id: &str, session_id: &str, data: &str) {
    let created = serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|value| value["time"]["created"].as_i64())
        .expect("test message payload needs time.created");
    create_db_message_with_time(path, id, session_id, created, data);
}

fn create_db_message_with_time(
    path: &Path,
    id: &str,
    session_id: &str,
    time_created_ms: i64,
    data: &str,
) {
    let db = sqlite::open(path).unwrap();
    db.execute(
        "CREATE TABLE IF NOT EXISTS message \
             (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER NOT NULL DEFAULT 0, data TEXT)",
    )
    .unwrap();
    let mut statement = db
        .prepare("INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)")
        .unwrap();
    statement.bind((1, id)).unwrap();
    statement.bind((2, session_id)).unwrap();
    statement.bind((3, time_created_ms)).unwrap();
    statement.bind((4, data)).unwrap();
    statement.next().unwrap();
}

fn create_db_message_legacy_schema(path: &Path, id: &str, session_id: &str, data: &str) {
    let db = sqlite::open(path).unwrap();
    db.execute("CREATE TABLE IF NOT EXISTS message (id TEXT, session_id TEXT, data TEXT)")
        .unwrap();
    let mut statement = db
        .prepare("INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)")
        .unwrap();
    statement.bind((1, id)).unwrap();
    statement.bind((2, session_id)).unwrap();
    statement.bind((3, data)).unwrap();
    statement.next().unwrap();
}

fn create_db_session_message_table(path: &Path, with_time_created: bool) {
    let db = sqlite::open(path).unwrap();
    let schema = if with_time_created {
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT)"
    } else {
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0, data TEXT)"
    };
    db.execute(schema).unwrap();
}

fn set_db_session_message_seq(path: &Path, id: &str, seq: i64) {
    let db = sqlite::open(path).unwrap();
    let mut statement = db
        .prepare("UPDATE session_message SET seq = ?1 WHERE id = ?2")
        .unwrap();
    statement.bind((1, seq)).unwrap();
    statement.bind((2, id)).unwrap();
    statement.next().unwrap();
}

fn insert_db_session_message(
    path: &Path,
    id: &str,
    session_id: &str,
    message_type: &str,
    time_created: i64,
    data: &str,
    with_time_created: bool,
) {
    let db = sqlite::open(path).unwrap();
    if with_time_created {
        let mut statement = db
            .prepare(
                "INSERT INTO session_message (id, session_id, type, time_created, data) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, message_type)).unwrap();
        statement.bind((4, time_created)).unwrap();
        statement.bind((5, data)).unwrap();
        statement.next().unwrap();
    } else {
        let mut statement = db
            .prepare(
                "INSERT INTO session_message (id, session_id, type, data) \
                     VALUES (?1, ?2, ?3, ?4)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, message_type)).unwrap();
        statement.bind((4, data)).unwrap();
        statement.next().unwrap();
    }
}

fn create_db_session_aggregate_table(path: &Path, table: &str) {
    create_db_session_aggregate_table_with_fork_columns(path, table, false);
}

fn create_db_session_aggregate_table_with_fork_columns(
    path: &Path,
    table: &str,
    with_fork_columns: bool,
) {
    let fork_columns = if with_fork_columns {
        ", fork_session_id TEXT, fork_boundary TEXT"
    } else {
        ""
    };
    let schema = match table {
        "session_v2" => {
            format!(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT{fork_columns})"
            )
        }
        "session" => {
            format!(
                "CREATE TABLE session (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT{fork_columns})"
            )
        }
        _ => panic!("unsupported test session table: {table}"),
    };
    sqlite::open(path)
        .unwrap()
        .execute(schema.as_str())
        .unwrap();
}

fn insert_db_session_fork(
    path: &Path,
    table: &str,
    fork_id: &str,
    parent_id: &str,
    boundary: &str,
) {
    let sql = match table {
        "session_v2" => {
            "INSERT INTO session_v2 (id, fork_session_id, fork_boundary) VALUES (?1, ?2, ?3)"
        }
        "session" => "INSERT INTO session (id, fork_session_id, fork_boundary) VALUES (?1, ?2, ?3)",
        _ => panic!("unsupported test session table: {table}"),
    };
    let db = sqlite::open(path).unwrap();
    let mut statement = db.prepare(sql).unwrap();
    statement.bind((1, fork_id)).unwrap();
    statement.bind((2, parent_id)).unwrap();
    statement.bind((3, boundary)).unwrap();
    statement.next().unwrap();
}

struct SessionAggregateFixture<'a> {
    session_id: &'a str,
    time_created: i64,
    model: &'a str,
    cost: f64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
}

fn insert_db_session_aggregate(path: &Path, table: &str, fixture: &SessionAggregateFixture<'_>) {
    let sql = match table {
        "session_v2" => {
            "INSERT INTO session_v2 (id, time_created, cost, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        }
        "session" => {
            "INSERT INTO session (id, time_created, cost, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        }
        _ => panic!("unsupported test session table: {table}"),
    };
    let db = sqlite::open(path).unwrap();
    let mut statement = db.prepare(sql).unwrap();
    statement.bind((1, fixture.session_id)).unwrap();
    statement.bind((2, fixture.time_created)).unwrap();
    statement.bind((3, fixture.cost)).unwrap();
    statement.bind((4, fixture.input_tokens)).unwrap();
    statement.bind((5, fixture.output_tokens)).unwrap();
    statement.bind((6, fixture.cache_read_tokens)).unwrap();
    statement.bind((7, fixture.cache_write_tokens)).unwrap();
    statement.bind((8, fixture.reasoning_tokens)).unwrap();
    statement.bind((9, fixture.model)).unwrap();
    statement.next().unwrap();
}

#[test]
fn loads_message_json_files() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"cache":{"read":10,"write":20}},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date.to_string(), "2026-01-02");
    assert_eq!(entries[0].session_id.as_ref(), "session-a");
    assert_eq!(
        entries[0].model.as_deref(),
        Some("claude-sonnet-4-20250514")
    );
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.output_tokens, 50);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
    assert_eq!(entries[0].cost, 0.02);
}

#[test]
fn loads_messages_from_sqlite_database() {
    let fixture = fs_fixture!({});
    create_db_message(
        &fixture.path("opencode.db"),
        "db-msg-1",
        "db-session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60,"cache":{"read":12,"write":24}},"cost":0.03}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date.to_string(), "2026-01-02");
    assert_eq!(entries[0].session_id.as_ref(), "db-session-a");
    assert_eq!(entries[0].message_id.as_deref(), Some("db-msg-1"));
    assert_eq!(entries[0].usage.input_tokens, 120);
    assert_eq!(entries[0].usage.output_tokens, 60);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 24);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 12);
    assert_eq!(entries[0].cost, 0.03);
}

#[test]
fn loads_nested_v2_assistant_usage_and_ignores_session_aggregate() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_db_session_message(
        &db_path,
        "msg-v2-user",
        "v2-session",
        "user",
        1_767_312_000_000,
        r#"{"text":"hello","time":{"created":1767312000000}}"#,
        true,
    );
    insert_db_session_message(
        &db_path,
        "msg-v2-assistant",
        "v2-session",
        "assistant",
        1_767_312_000_001,
        r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":120,"output":60,"reasoning":10,"cache":{"read":12,"write":24}},"cost":0.03}"#,
        true,
    );
    create_db_session_aggregate_table(&db_path, "session");
    insert_db_session_aggregate(
        &db_path,
        "session",
        &SessionAggregateFixture {
            session_id: "v2-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 9.99,
            input_tokens: 9_999,
            output_tokens: 9_999,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let entries =
        load_entries_from_directory_for_report(fixture.root(), &shared, AgentReportKind::Session);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-v2-assistant"));
    assert_eq!(entries[0].session_id.as_ref(), "v2-session");
    assert_eq!(entries[0].usage.input_tokens, 120);
    assert_eq!(entries[0].usage.output_tokens, 60);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 24);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 12);
    assert_eq!(entries[0].extra_total_tokens, 10);
    assert_eq!(entries[0].cost, 0.03);
}

#[test]
fn loads_v2_messages_without_a_time_created_column() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, false);
    insert_db_session_message(
        &db_path,
        "msg-v2-no-time-column",
        "v2-session",
        "assistant",
        1_767_312_000_000,
        r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":2,"output":3},"cost":0.01}"#,
        false,
    );

    let entries = load_entries_from_directory(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date.to_string(), "2026-01-02");
    assert_eq!(
        entries[0].message_id.as_deref(),
        Some("msg-v2-no-time-column")
    );
}

#[test]
fn deduplicates_legacy_and_v2_rows_by_message_id() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_message(
        &db_path,
        "msg-shared",
        "legacy-session",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
    );
    create_db_session_message_table(&db_path, true);
    insert_db_session_message(
        &db_path,
        "msg-shared",
        "v2-session",
        "assistant",
        1_767_312_000_000,
        r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":9.99}"#,
        true,
    );

    let entries = load_entries_from_directory(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "legacy-session");
    assert_eq!(entries[0].usage.input_tokens, 120);
    assert_eq!(entries[0].cost, 0.03);
}

const COMPACTION_COMPLETED: &str = r#"{"status":"completed","reason":"auto","summary":"s","model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":100,"output":20,"reasoning":7,"cache":{"read":30,"write":5}},"cost":0.02}"#;

fn insert_assistant_and_compaction(db_path: &Path, session_id: &str, prefix: &str) {
    insert_db_session_message(
        db_path,
        &format!("{prefix}-assistant"),
        session_id,
        "assistant",
        1_767_312_000_000,
        assistant_payload(10, 2, 0.01).as_str(),
        true,
    );
    set_db_session_message_seq(db_path, &format!("{prefix}-assistant"), 0);
    insert_db_session_message(
        db_path,
        &format!("{prefix}-compaction"),
        session_id,
        "compaction",
        1_767_312_000_001,
        COMPACTION_COMPLETED,
        true,
    );
    set_db_session_message_seq(db_path, &format!("{prefix}-compaction"), 1);
}

fn total_tokens(entries: &[crate::LoadedEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| {
            let usage = &entry.usage;
            usage.input_tokens
                + usage.output_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens
                + entry.extra_total_tokens
        })
        .sum()
}

#[test]
fn a_message_wins_over_a_compaction_with_its_id_in_an_earlier_directory() {
    let fixture = fs_fixture!({});
    let first = fixture.path("first/opencode.db");
    let second = fixture.path("second/opencode.db");
    std::fs::create_dir_all(fixture.path("first")).unwrap();
    std::fs::create_dir_all(fixture.path("second")).unwrap();
    create_db_session_message_table(&first, true);
    insert_db_session_message(
        &first,
        "shared-id",
        "v2-session",
        "compaction",
        1_767_312_000_000,
        COMPACTION_COMPLETED,
        true,
    );
    create_db_session_message_table(&second, true);
    insert_db_session_message(
        &second,
        "shared-id",
        "v2-session",
        "assistant",
        1_767_312_000_000,
        assistant_payload(10, 2, 0.01).as_str(),
        true,
    );

    let entries = load_linked(
        &[&fixture.path("first"), &fixture.path("second")],
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
        AgentReportKind::Daily,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.usage.input_tokens, 10);
}

#[test]
fn counts_completed_v2_compaction_usage_alongside_assistant_usage() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_assistant_and_compaction(&db_path, "v2-session", "msg");
    insert_db_session_message(
        &db_path,
        "msg-compaction-running",
        "v2-session",
        "compaction",
        1_767_312_000_002,
        r#"{"status":"running","reason":"auto","summary":"","time":{"created":1767312000000}}"#,
        true,
    );
    insert_db_session_message(
        &db_path,
        "msg-compaction-failed",
        "v2-session",
        "compaction",
        1_767_312_000_003,
        r#"{"status":"failed","reason":"auto","error":{"type":"unknown","message":"x"},"time":{"created":1767312000000},"tokens":{"input":50,"output":0},"cost":0.5}"#,
        true,
    );
    for (id, status) in [
        ("msg-compaction-failed-with-model", "failed"),
        ("msg-compaction-running-with-usage", "running"),
        ("msg-compaction-without-status", ""),
    ] {
        let status = if status.is_empty() {
            String::new()
        } else {
            format!(r#""status":"{status}","#)
        };
        insert_db_session_message(
            &db_path,
            id,
            "v2-session",
            "compaction",
            1_767_312_000_004,
            format!(
                r#"{{{status}"reason":"auto","model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":1767312000000}},"tokens":{{"input":40,"output":4}},"cost":0.4}}"#
            )
            .as_str(),
            true,
        );
    }

    let entries = load_entries_from_directory(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
    );

    let mut ids: Vec<_> = entries
        .iter()
        .filter_map(|entry| entry.message_id.as_deref())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["msg-assistant", "msg-compaction"]);
    assert_eq!(total_tokens(&entries), 174);
    let cost: f64 = entries.iter().map(|entry| entry.cost).sum();
    assert!((cost - 0.03).abs() < 1e-9);
}

#[test]
fn prefers_session_aggregate_over_a_lone_v2_compaction_row() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_db_session_message(
        &db_path,
        "msg-compaction",
        "v2-session",
        "compaction",
        1_767_312_000_001,
        COMPACTION_COMPLETED,
        true,
    );
    create_db_session_aggregate_table(&db_path, "session");
    insert_db_session_aggregate(
        &db_path,
        "session",
        &SessionAggregateFixture {
            session_id: "v2-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 0.03,
            input_tokens: 110,
            output_tokens: 22,
            cache_read_tokens: 30,
            cache_write_tokens: 5,
            reasoning_tokens: 7,
        },
    );
    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let session_entries =
        load_entries_from_directory_for_report(fixture.root(), &shared, AgentReportKind::Session);
    assert_eq!(session_entries.len(), 1);
    assert_ne!(
        session_entries[0].message_id.as_deref(),
        Some("msg-compaction")
    );
    assert_eq!(total_tokens(&session_entries), 174);
    assert!((session_entries[0].cost - 0.03).abs() < 1e-9);

    let daily_entries = load_entries_from_directory(fixture.root(), &shared);
    assert_eq!(daily_entries.len(), 1);
    assert_eq!(
        daily_entries[0].message_id.as_deref(),
        Some("msg-compaction")
    );
    assert_eq!(total_tokens(&daily_entries), 162);
}

#[test]
fn excludes_copied_v2_compaction_rows_from_forks() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_assistant_and_compaction(&db_path, "parent", "parent");
    insert_assistant_and_compaction(&db_path, "fork", "fork-copy");
    insert_db_session_message(
        &db_path,
        "fork-own",
        "fork",
        "assistant",
        1_767_312_000_002,
        assistant_payload(3, 1, 0.01).as_str(),
        true,
    );
    set_db_session_message_seq(&db_path, "fork-own", 2);
    create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
    insert_db_session_fork(
        &db_path,
        "session_v2",
        "fork",
        "parent",
        r#"{"type":"through","messageID":"parent-compaction"}"#,
    );

    let entries = fork_entries(&db_path);

    assert_eq!(total_tokens(&entries), 178);
    let fork_ids: Vec<_> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "fork")
        .filter_map(|entry| entry.message_id.as_deref())
        .collect();
    assert_eq!(fork_ids, vec!["fork-own"]);
}

fn assistant_payload(input: u64, output: u64, cost: f64) -> String {
    format!(
        r#"{{"model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":1767312000000}},"tokens":{{"input":{input},"output":{output}}},"cost":{cost}}}"#,
    )
}

fn insert_fork_history(
    db_path: &Path,
    parent_id: &str,
    fork_id: &str,
    boundary_type: &str,
    boundary_message_id: &str,
) {
    for (id, seq, input) in [
        ("parent-msg-1", 0, 100),
        ("parent-msg-2", 1, 200),
        (boundary_message_id, 2, 300),
        ("parent-msg-4", 3, 400),
    ] {
        insert_db_session_message(
            db_path,
            id,
            parent_id,
            "assistant",
            1_767_312_000_000 + seq,
            assistant_payload(input, 10, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(db_path, id, seq);
    }
    for (copy_id, seq, input) in [
        ("fork-copy-1", 0, 100),
        ("fork-copy-2", 1, 200),
        ("fork-copy-boundary", 2, 300),
    ] {
        insert_db_session_message(
            db_path,
            copy_id,
            fork_id,
            "assistant",
            1_767_312_000_000 + seq,
            assistant_payload(input, 10, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(db_path, copy_id, seq);
    }
    insert_db_session_message(
        db_path,
        "fork-own",
        fork_id,
        "assistant",
        1_767_312_000_004,
        assistant_payload(40, 4, 0.02).as_str(),
        true,
    );
    set_db_session_message_seq(db_path, "fork-own", 4);
    create_db_session_aggregate_table_with_fork_columns(db_path, "session_v2", true);
    insert_db_session_fork(
        db_path,
        "session_v2",
        fork_id,
        parent_id,
        format!(r#"{{"type":"{boundary_type}","messageID":"{boundary_message_id}"}}"#,).as_str(),
    );
}

fn fork_entries(db_path: &Path) -> Vec<crate::LoadedEntry> {
    load_entries_from_directory(
        db_path.parent().expect("fixture db has a parent directory"),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
    )
}

#[test]
fn excludes_copied_history_through_the_fork_boundary() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");

    let entries = fork_entries(&db_path);

    assert_eq!(entries.len(), 5);
    let parent_input: u64 = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "parent")
        .map(|entry| entry.usage.input_tokens)
        .sum();
    assert_eq!(parent_input, 1000);
    let fork_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "fork")
        .collect();
    assert_eq!(fork_entries.len(), 1);
    assert_eq!(fork_entries[0].message_id.as_deref(), Some("fork-own"));
    assert_eq!(fork_entries[0].usage.input_tokens, 40);
}

#[test]
fn excludes_copied_history_before_the_fork_boundary() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "before", "boundary-msg");

    let entries = fork_entries(&db_path);

    let mut fork_ids: Vec<_> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "fork")
        .filter_map(|entry| entry.message_id.as_deref())
        .collect();
    fork_ids.sort_unstable();
    assert_eq!(fork_ids, vec!["fork-copy-boundary", "fork-own"]);
}

#[test]
fn keeps_fork_rows_when_the_boundary_cannot_be_resolved() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
    let db = sqlite::open(&db_path).unwrap();
    db.execute(
        "UPDATE session_v2 SET fork_boundary = '{\"type\":\"through\",\"messageID\":\"missing\"}' \
             WHERE id = 'fork'",
    )
    .unwrap();

    let entries = fork_entries(&db_path);

    assert_eq!(entries.len(), 8);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .count(),
        4
    );
}

#[test]
fn keeps_fork_rows_without_fork_metadata() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
    let db = sqlite::open(&db_path).unwrap();
    db.execute("DROP TABLE session_v2").unwrap();

    let entries = fork_entries(&db_path);

    assert_eq!(entries.len(), 8);
}

#[test]
fn keeps_fork_rows_when_session_messages_have_no_seq_column() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    sqlite::open(&db_path)
        .unwrap()
        .execute(
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER, data TEXT)",
        )
        .unwrap();
    for (id, session_id, input) in [
        ("parent-msg", "parent", 100),
        ("fork-copy", "fork", 100),
        ("fork-own", "fork", 40),
    ] {
        insert_db_session_message(
            &db_path,
            id,
            session_id,
            "assistant",
            1_767_312_000_000,
            assistant_payload(input, 10, 0.01).as_str(),
            true,
        );
    }
    create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
    insert_db_session_fork(
        &db_path,
        "session_v2",
        "fork",
        "parent",
        r#"{"type":"through","messageID":"parent-msg"}"#,
    );

    let entries = fork_entries(&db_path);

    assert_eq!(entries.len(), 3);
}

#[test]
fn excludes_copied_history_from_a_fork_of_a_fork() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
    for (copy_id, seq) in [
        ("grand-copy-1", 0),
        ("grand-copy-2", 1),
        ("grand-copy-boundary", 2),
        ("grand-copy-fork-own", 4),
    ] {
        insert_db_session_message(
            &db_path,
            copy_id,
            "grandchild",
            "assistant",
            1_767_312_000_000 + seq,
            assistant_payload(100, 10, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(&db_path, copy_id, seq);
    }
    insert_db_session_message(
        &db_path,
        "grand-own",
        "grandchild",
        "assistant",
        1_767_312_000_005,
        assistant_payload(7, 1, 0.01).as_str(),
        true,
    );
    set_db_session_message_seq(&db_path, "grand-own", 5);
    insert_db_session_fork(
        &db_path,
        "session_v2",
        "grandchild",
        "fork",
        r#"{"type":"through","messageID":"fork-own"}"#,
    );

    let entries = fork_entries(&db_path);

    let grandchild_ids: Vec<_> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "grandchild")
        .filter_map(|entry| entry.message_id.as_deref())
        .collect();
    assert_eq!(grandchild_ids, vec!["grand-own"]);
    assert_eq!(entries.len(), 6);
}

#[test]
fn falls_back_to_the_fork_local_aggregate_when_only_copies_remain() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
    sqlite::open(&db_path)
        .unwrap()
        .execute("DELETE FROM session_message WHERE id = 'fork-own'")
        .unwrap();
    sqlite::open(&db_path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET time_created = 1767312000000, cost = 0.02, \
                 tokens_input = 40, tokens_output = 4, tokens_cache_read = 0, \
                 tokens_cache_write = 0, tokens_reasoning = 0, \
                 model = '{\"id\":\"gpt-test\",\"providerID\":\"openai\"}' WHERE id = 'fork'",
        )
        .unwrap();
    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let entries =
        load_entries_from_directory_for_report(fixture.root(), &shared, AgentReportKind::Session);

    let fork_input: Vec<u64> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "fork")
        .map(|entry| entry.usage.input_tokens)
        .collect();
    assert_eq!(fork_input, vec![40]);
    assert_eq!(entries.len(), 5);
}

#[test]
fn keeps_fork_rows_after_the_last_copy_when_before_seqs_are_sparse() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
    for (session_id, id, seq, input) in [
        ("parent", "parent-a", 0, 100),
        ("parent", "parent-b", 10, 200),
        ("parent", "parent-boundary", 100, 300),
        ("fork", "fork-copy-a", 0, 100),
        ("fork", "fork-copy-b", 10, 200),
        ("fork", "fork-own", 20, 40),
    ] {
        insert_db_session_message(
            &db_path,
            id,
            session_id,
            "assistant",
            1_767_312_000_000 + seq,
            assistant_payload(input, 10, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(&db_path, id, seq);
    }
    insert_db_session_fork(
        &db_path,
        "session_v2",
        "fork",
        "parent",
        r#"{"type":"before","messageID":"parent-boundary"}"#,
    );

    let entries = fork_entries(&db_path);

    let fork_ids: Vec<_> = entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == "fork")
        .filter_map(|entry| entry.message_id.as_deref())
        .collect();
    assert_eq!(fork_ids, vec!["fork-own"]);
}

#[test]
fn suppresses_session_aggregate_when_legacy_json_has_message() {
    let fixture = fs_fixture!({
        "storage/message/legacy-json-session/legacy-json-message.json": r#"{"id":"legacy-json-message","sessionID":"legacy-json-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
    });
    let db_path = fixture.path("opencode.db");
    create_db_session_aggregate_table(&db_path, "session_v2");
    insert_db_session_aggregate(
        &db_path,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "legacy-json-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 9.99,
            input_tokens: 9_999,
            output_tokens: 9_999,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );

    let entries = load_entries_from_directory_for_report(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
        AgentReportKind::Session,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "legacy-json-session");
    assert_eq!(
        entries[0].message_id.as_deref(),
        Some("legacy-json-message")
    );
    assert_eq!(entries[0].usage.input_tokens, 120);
    assert_eq!(entries[0].cost, 0.03);
}

#[test]
fn uses_session_v2_aggregate_when_no_message_level_usage_exists() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_aggregate_table(&db_path, "session_v2");
    insert_db_session_aggregate(
        &db_path,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "aggregate-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 0.25,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 20,
            reasoning_tokens: 5,
        },
    );

    let entries = load_entries_from_directory_for_report(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
        AgentReportKind::Session,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "aggregate-session");
    assert_eq!(
        entries[0].message_id.as_deref(),
        Some("session:aggregate-session")
    );
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.output_tokens, 50);
    assert_eq!(entries[0].extra_total_tokens, 5);
    assert_eq!(entries[0].cost, 0.25);
}

#[test]
fn excludes_cumulative_session_aggregate_from_bounded_window() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_aggregate_table(&db_path, "session_v2");
    insert_db_session_aggregate(
        &db_path,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "spanning-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 0.25,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 20,
            reasoning_tokens: 5,
        },
    );

    let entries = load_entries_from_directory_for_report(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20260102".parse().unwrap()),
            until: Some("20260103".parse().unwrap()),
            ..SharedArgs::default()
        },
        AgentReportKind::Session,
    );

    assert!(entries.is_empty());
}

#[test]
fn suppresses_aggregate_when_message_usage_is_in_another_configured_directory() {
    let aggregate_fixture = fs_fixture!({});
    let aggregate_db_path = aggregate_fixture.path("opencode.db");
    create_db_session_aggregate_table(&aggregate_db_path, "session_v2");
    insert_db_session_aggregate(
        &aggregate_db_path,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "cross-directory-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 9.99,
            input_tokens: 9_999,
            output_tokens: 9_999,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );
    let message_fixture = fs_fixture!({
        "storage/message/cross-directory-session/cross-directory-message.json": r#"{"id":"cross-directory-message","sessionID":"cross-directory-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":12,"output":6},"cost":0.03}"#,
    });
    let entries = load_linked(
        &[aggregate_fixture.root(), message_fixture.root()],
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
        AgentReportKind::Session,
    );
    let entries = entries.iter().map(|(entry, _)| entry).collect::<Vec<_>>();

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].message_id.as_deref(),
        Some("cross-directory-message")
    );
    assert_eq!(entries[0].cost, 0.03);
}

#[test]
fn only_session_reports_use_cumulative_session_aggregate_fallback() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_message(
        &db_path,
        "period-message",
        "period-message-session",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1},"cost":0.01}"#,
    );
    create_db_session_aggregate_table(&db_path, "session_v2");
    insert_db_session_aggregate(
        &db_path,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "period-aggregate-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 9.99,
            input_tokens: 9_999,
            output_tokens: 9_999,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );
    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for report_kind in [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
    ] {
        let entries = load_entries_from_directory_for_report(fixture.root(), &shared, report_kind);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("period-message"));
    }

    let session_entries =
        load_entries_from_directory_for_report(fixture.root(), &shared, AgentReportKind::Session);
    assert_eq!(session_entries.len(), 2);
    assert!(
        session_entries.iter().any(|entry| {
            entry.message_id.as_deref() == Some("session:period-aggregate-session")
        })
    );
}

#[test]
fn pairs_claude_agent_rows_with_their_claude_session_across_data_dirs() {
    let live = fs_fixture!({});
    let live_db = live.path("opencode.db");
    create_db_session_message_table(&live_db, true);
    for (id, session_id, data) in [
        (
            "agent",
            "agent-session",
            r#"{"time":{"created":1767312000000},"model":{"id":"opus","providerID":"claude-agent"},"content":[{"type":"tool","native":{"sessionID":"claude-a","messageID":"m"}}],"tokens":{"input":100},"cost":0}"#,
        ),
        (
            "plain",
            "plain-session",
            r#"{"time":{"created":1767312000000},"model":{"id":"gpt-test","providerID":"openai"},"content":[{"type":"text","text":"\"native\": not a part"}],"tokens":{"input":5},"cost":0}"#,
        ),
    ] {
        insert_db_session_message(
            &live_db,
            id,
            session_id,
            "assistant",
            1_767_312_000_000,
            data,
            true,
        );
    }
    let archive = fs_fixture!({});
    let archive_db = archive.path("opencode.db");
    create_db_session_aggregate_table(&archive_db, "session_v2");
    insert_db_session_aggregate(
        &archive_db,
        "session_v2",
        &SessionAggregateFixture {
            session_id: "agent-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"opus","providerID":"claude-agent"}"#,
            cost: 0.0,
            input_tokens: 9_999,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        },
    );
    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let entries = load_linked(
        &[live.root(), archive.root()],
        &shared,
        AgentReportKind::Session,
    );
    let linked = entries
        .iter()
        .map(|(entry, native_session)| {
            (
                entry.message_id.as_deref().unwrap(),
                native_session.as_deref(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(linked, [("agent", Some("claude-a")), ("plain", None)]);
}

#[test]
fn uses_current_session_table_as_v2_aggregate_fallback() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    create_db_session_aggregate_table(&db_path, "session");
    insert_db_session_aggregate(
        &db_path,
        "session",
        &SessionAggregateFixture {
            session_id: "current-session",
            time_created: 1_767_312_000_000,
            model: r#"{"id":"gpt-test","providerID":"openai"}"#,
            cost: 0.5,
            input_tokens: 200,
            output_tokens: 100,
            cache_read_tokens: 20,
            cache_write_tokens: 40,
            reasoning_tokens: 0,
        },
    );

    let entries = load_entries_from_directory_for_report(
        fixture.root(),
        &SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        },
        AgentReportKind::Session,
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "current-session");
    assert_eq!(entries[0].usage.input_tokens, 200);
    assert_eq!(entries[0].cost, 0.5);
}

#[test]
fn loads_channel_sqlite_database() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    create_db_message(
        &fixture.path("opencode-beta.db"),
        "channel-msg-1",
        "channel-session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":80,"output":40}}"#,
    );

    let entries = load_entries_from_directory(fixture.root(), &SharedArgs::default());

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "channel-session-a");
    assert_eq!(entries[0].usage.input_tokens, 80);
}

#[test]
fn prefers_database_messages_over_duplicate_json_files() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"json-session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":0.99}"#,
    });
    create_db_message(
        &fixture.path("opencode.db"),
        "msg-1",
        "db-session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_ref(), "db-session-a");
    assert_eq!(entries[0].usage.input_tokens, 120);
    assert_eq!(entries[0].cost, 0.03);
}

#[test]
fn skips_message_files_already_covered_by_database() {
    let fixture = fs_fixture!({
        "storage/message/ses_a/msg-db.json": r#"{"id":"msg-db","sessionID":"json-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":0.99}"#,
        "storage/message/ses_a/msg-file.json": r#"{"id":"msg-file","sessionID":"file-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000001},"tokens":{"input":50,"output":25},"cost":0.01}"#,
    });
    create_db_message(
        &fixture.path("opencode.db"),
        "msg-db",
        "db-session",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 2);
    let db_entry = entries
        .iter()
        .find(|entry| entry.message_id.as_deref() == Some("msg-db"))
        .expect("db-covered message present");
    assert_eq!(db_entry.session_id.as_ref(), "db-session");
    assert_eq!(db_entry.usage.input_tokens, 120);
    let file_entry = entries
        .iter()
        .find(|entry| entry.message_id.as_deref() == Some("msg-file"))
        .expect("db-uncovered message present");
    assert_eq!(file_entry.session_id.as_ref(), "file-session");
    assert_eq!(file_entry.usage.input_tokens, 50);
}

#[test]
fn dedup_is_stable_across_thread_counts() {
    let fixture = ccusage_test_support::Fixture::new();
    for session in 0..4 {
        for message in 0..15 {
            let id = format!("msg-{session}-{message}");
            let created = 1_767_312_000_000_i64 + i64::from(session * 100 + message);
            let path = format!("storage/message/ses_{session}/{id}.json");
            let data = format!(
                r#"{{"id":"{id}","sessionID":"ses_{session}","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{{"created":{created}}},"tokens":{{"input":{input},"output":10}}}}"#,
                input = 100 + message,
            );
            fixture.write_file(path, data);
        }
    }
    fixture.write_file(
        "storage/message/ses_dup/msg-0-0.json",
        r#"{"id":"msg-0-0","sessionID":"ses_dup","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312999999},"tokens":{"input":7777,"output":10}}"#,
    );

    create_db_message(
        &fixture.path("opencode.db"),
        "msg-1-1",
        "db-session",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60}}"#,
    );

    let single = SharedArgs {
        mode: CostMode::Display,
        single_thread: true,
        ..SharedArgs::default()
    };
    let multi = SharedArgs {
        mode: CostMode::Display,
        single_thread: false,
        ..SharedArgs::default()
    };

    let single_entries = load_entries_from_directory(fixture.root(), &single);
    let multi_entries = load_entries_from_directory(fixture.root(), &multi);

    let project = |entries: &[crate::LoadedEntry]| {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.timestamp.as_millis(),
                    entry.message_id.clone(),
                    entry.session_id.to_string(),
                    entry.usage.input_tokens,
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(project(&single_entries), project(&multi_entries));
}

#[test]
fn since_filter_drops_db_rows_older_than_lower_bound() {
    let fixture = fs_fixture!({});
    create_db_message_with_time(
        &fixture.path("opencode.db"),
        "msg-old",
        "session-old",
        1_767_139_200_000,
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767139200000},"tokens":{"input":1,"output":1}}"#,
    );
    create_db_message_with_time(
        &fixture.path("opencode.db"),
        "msg-new",
        "session-new",
        1_767_484_800_000,
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767484800000},"tokens":{"input":2,"output":2}}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260103".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-new"));
}

#[test]
fn until_filter_drops_db_rows_at_or_after_upper_bound() {
    let fixture = fs_fixture!({});
    create_db_message_with_time(
        &fixture.path("opencode.db"),
        "msg-early",
        "session-early",
        1_767_312_000_000,
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
    );
    create_db_message_with_time(
        &fixture.path("opencode.db"),
        "msg-late",
        "session-late",
        1_768_089_600_000,
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1768089600000},"tokens":{"input":2,"output":2}}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        until: Some("20260105".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-early"));
}

#[test]
fn legacy_schema_without_time_created_still_returns_in_range_rows() {
    let fixture = fs_fixture!({});
    create_db_message_legacy_schema(
        &fixture.path("opencode.db"),
        "msg-in-range",
        "session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767571200000},"tokens":{"input":3,"output":3}}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260103".parse().unwrap()),
        until: Some("20260107".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-in-range"));
}

#[test]
fn legacy_schema_without_time_created_still_drops_out_of_range_rows() {
    let fixture = fs_fixture!({});
    create_db_message_legacy_schema(
        &fixture.path("opencode.db"),
        "msg-out-of-range",
        "session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":3,"output":3}}"#,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260103".parse().unwrap()),
        until: Some("20260107".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert!(
        entries.is_empty(),
        "fallback scan must still exclude out-of-range rows via the in-loop check"
    );
}

#[test]
fn filters_json_file_entries_by_until() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        until: Some("20260101".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert!(
        entries.is_empty(),
        "message on 2026-01-02 should be excluded by until=20260101"
    );
}

#[test]
fn filters_json_file_entries_by_since() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260103".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert!(
        entries.is_empty(),
        "message on 2026-01-02 should be excluded by since=20260103"
    );
}

#[test]
fn includes_entries_when_since_until_bracket_date() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260101".parse().unwrap()),
        until: Some("20260103".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert_eq!(
        entries.len(),
        1,
        "message on 2026-01-02 should be included when since=20260101 and until=20260103"
    );
}

#[test]
fn includes_entries_when_since_exact_match() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260102".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert_eq!(
        entries.len(),
        1,
        "message on 2026-01-02 should be included when since=20260102"
    );
}

#[test]
fn includes_entries_when_until_exact_match() {
    let fixture = fs_fixture!({
        "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        until: Some("20260102".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert_eq!(
        entries.len(),
        1,
        "message on 2026-01-02 should be included when until=20260102"
    );
}

// NOTE(2026-07-27, #1492): Legacy OpenCode pretty-prints message files. Cover the window filter on the layout found on disk.
const PRETTY_PRINTED_MESSAGE: &str = r#"{
  "id": "msg-pretty",
  "sessionID": "session-a",
  "role": "assistant",
  "providerID": "anthropic",
  "modelID": "claude-sonnet-4-20250514",
  "time": {
    "created": 1767312000000,
    "completed": 1767312001000
  },
  "tokens": {
    "input": 100,
    "output": 50
  },
  "cost": 0.02
}"#;

#[test]
fn extracts_timestamp_from_pretty_printed_file_when_out_of_range() {
    let fixture = fs_fixture!({
        "storage/message/message.json": PRETTY_PRINTED_MESSAGE,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        until: Some("20260101".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert!(
        entries.is_empty(),
        "pretty-printed message on 2026-01-02 must be excluded by until=20260101"
    );
}

#[test]
fn extracts_timestamp_from_pretty_printed_file_when_in_range() {
    let fixture = fs_fixture!({
        "storage/message/message.json": PRETTY_PRINTED_MESSAGE,
    });

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260101".parse().unwrap()),
        until: Some("20260103".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-pretty"));
}

const NESTED_TIME_BEFORE_OWN: &str = r#"{"metadata":{"time":{"created":1766923200000}},"id":"msg-nested","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767268800000},"tokens":{"input":100,"output":50}}"#;

const NESTED_TIME_BEFORE_OWN_V2: &str = r#"{"metadata":{"time":{"created":1766923200000}},"model":{"id":"claude-sonnet-4-20250514","providerID":"anthropic"},"time":{"created":1767268800000},"tokens":{"input":100,"output":50}}"#;

fn bounded_and_unbounded_tokens(directory: &Path) -> (u64, u64) {
    let unbounded = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let bounded = SharedArgs {
        since: Some("20260101".parse().unwrap()),
        until: Some("20260101".parse().unwrap()),
        ..unbounded.clone()
    };
    (
        total_tokens(&load_entries_from_directory(directory, &bounded)),
        total_tokens(&load_entries_from_directory(directory, &unbounded)),
    )
}

#[test]
fn a_nested_time_before_a_files_own_does_not_move_it_out_of_the_window() {
    let fixture = fs_fixture!({
        "storage/message/session-a/msg-nested.json": NESTED_TIME_BEFORE_OWN,
    });

    assert_eq!(bounded_and_unbounded_tokens(fixture.root()), (150, 150));
}

#[test]
fn a_nested_time_before_a_rows_own_does_not_move_it_out_of_the_window() {
    let fixture = fs_fixture!({});
    create_db_message(
        &fixture.path("opencode.db"),
        "msg-nested",
        "session-a",
        NESTED_TIME_BEFORE_OWN,
    );

    assert_eq!(bounded_and_unbounded_tokens(fixture.root()), (150, 150));
}

#[test]
fn a_nested_time_before_a_v2_rows_own_does_not_move_it_out_of_the_window() {
    let fixture = fs_fixture!({});
    let db_path = fixture.path("opencode.db");
    create_db_session_message_table(&db_path, true);
    insert_db_session_message(
        &db_path,
        "msg-nested",
        "session-a",
        "assistant",
        1_767_268_800_000,
        NESTED_TIME_BEFORE_OWN_V2,
        true,
    );

    assert_eq!(bounded_and_unbounded_tokens(fixture.root()), (150, 150));
}

const NOON_UTC_2026_01_01: &str = r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767268800000},"tokens":{"input":1,"output":1}}"#;

const EARLY_UTC_2026_01_02: &str = r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767333600000},"tokens":{"input":1,"output":1}}"#;

#[test]
fn since_bound_follows_local_midnight_in_the_reporting_timezone() {
    let fixture = fs_fixture!({});
    create_db_message(
        &fixture.path("opencode.db"),
        "msg-utc-plus-14",
        "session-a",
        NOON_UTC_2026_01_01,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some("Pacific/Kiritimati".parse().unwrap()),
        since: Some("20260102".parse().unwrap()),
        until: Some("20260102".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(
        entries.len(),
        1,
        "a UTC+14 local date of 2026-01-02 must survive since=until=20260102"
    );
    assert_eq!(entries[0].date.to_string(), "2026-01-02");
}

#[test]
fn until_bound_follows_local_midnight_in_the_reporting_timezone() {
    let fixture = fs_fixture!({});
    create_db_message(
        &fixture.path("opencode.db"),
        "msg-utc-minus-12",
        "session-a",
        EARLY_UTC_2026_01_02,
    );

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some("Etc/GMT+12".parse().unwrap()),
        until: Some("20260101".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(
        entries.len(),
        1,
        "a UTC-12 local date of 2026-01-01 must survive until=20260101"
    );
    assert_eq!(entries[0].date.to_string(), "2026-01-01");
}

const NOON_UTC_2026_01_01_V2: &str = r#"{"model":{"id":"claude-sonnet-4-20250514","providerID":"anthropic"},"time":{"created":1767268800000},"tokens":{"input":1,"output":1}}"#;

const NOON_UTC_2026_01_01_MS: i64 = 1_767_268_800_000;

#[derive(Clone, Copy, Debug)]
enum MessageTable {
    Message,
    SessionMessage,
}

fn create_message_with_time(path: &Path, table: MessageTable, time_created: i64) {
    match table {
        MessageTable::Message => create_db_message_with_time(
            path,
            "msg-drifted",
            "session-a",
            time_created,
            NOON_UTC_2026_01_01,
        ),
        MessageTable::SessionMessage => {
            create_db_session_message_table(path, true);
            insert_db_session_message(
                path,
                "msg-drifted",
                "session-a",
                "assistant",
                time_created,
                NOON_UTC_2026_01_01_V2,
                true,
            );
        }
    }
}

fn load_local_day(directory: &Path, timezone: &str, day: &str) -> Vec<LoadedEntry> {
    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(timezone.parse().unwrap()),
        since: Some(day.parse().unwrap()),
        until: Some(day.parse().unwrap()),
        ..SharedArgs::default()
    };
    load_entries_from_directory(directory, &shared)
}

#[test]
fn a_time_created_column_far_from_the_payload_does_not_drop_the_row() {
    const DAY: i64 = 24 * 60 * 60 * 1000;
    for (table, timezone, day, date, drift) in [
        (
            MessageTable::Message,
            "UTC",
            "20260101",
            "2026-01-01",
            -4 * DAY,
        ),
        (
            MessageTable::Message,
            "Pacific/Kiritimati",
            "20260102",
            "2026-01-02",
            4 * DAY,
        ),
        (
            MessageTable::SessionMessage,
            "UTC",
            "20260101",
            "2026-01-01",
            4 * DAY,
        ),
        (
            MessageTable::SessionMessage,
            "Etc/GMT+12",
            "20260101",
            "2026-01-01",
            -4 * DAY,
        ),
    ] {
        let fixture = fs_fixture!({});
        create_message_with_time(
            &fixture.path("opencode.db"),
            table,
            NOON_UTC_2026_01_01_MS + drift,
        );

        let entries = load_local_day(fixture.root(), timezone, day);

        let case = format!("{table:?} in {timezone} with time_created {drift} ms away");
        assert_eq!(entries.len(), 1, "{case}");
        assert_eq!(entries[0].date.to_string(), date, "{case}");
    }
}

fn create_opencode_time_index(path: &Path, table: MessageTable) {
    let db = sqlite::open(path).unwrap();
    db.execute(match table {
        MessageTable::Message => {
            "CREATE INDEX message_session_time_created_id_idx ON message (session_id, time_created, id)"
        }
        MessageTable::SessionMessage => {
            "CREATE INDEX session_message_time_created_idx ON session_message (time_created)"
        }
    })
    .unwrap();
}

#[test]
fn opencode_indexed_tables_keep_rows_on_the_local_day_bounds() {
    const TOKYO_2026_01_02_START: i64 = 1_767_279_600_000;
    const TOKYO_2026_01_02_END: i64 = TOKYO_2026_01_02_START + 24 * 60 * 60 * 1000;
    for table in [MessageTable::Message, MessageTable::SessionMessage] {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        if matches!(table, MessageTable::SessionMessage) {
            create_db_session_message_table(&db_path, true);
        }
        for (id, created) in [
            ("msg-before", TOKYO_2026_01_02_START - 1),
            ("msg-first", TOKYO_2026_01_02_START),
            ("msg-last", TOKYO_2026_01_02_END - 1),
            ("msg-after", TOKYO_2026_01_02_END),
        ] {
            match table {
                MessageTable::Message => create_db_message_with_time(
                    &db_path,
                    id,
                    "session-a",
                    created,
                    &format!(
                        r#"{{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{{"created":{created}}},"tokens":{{"input":1,"output":1}}}}"#
                    ),
                ),
                MessageTable::SessionMessage => insert_db_session_message(
                    &db_path,
                    id,
                    "session-a",
                    "assistant",
                    created,
                    &format!(
                        r#"{{"model":{{"id":"claude-sonnet-4-20250514","providerID":"anthropic"}},"time":{{"created":{created}}},"tokens":{{"input":1,"output":1}}}}"#
                    ),
                    true,
                ),
            }
        }
        create_opencode_time_index(&db_path, table);
        let schema_table = match table {
            MessageTable::Message => super::Table::Message,
            MessageTable::SessionMessage => super::Table::SessionMessage,
        };
        let schema = super::TableSchema::probe(
            &sqlite::open(&db_path).unwrap(),
            schema_table,
            &db_path,
            &ccusage_core::Diagnostics::new("opencode"),
        )
        .unwrap();
        assert!(schema.indexed_time, "{table:?}");

        let entries = load_local_day(fixture.root(), "Asia/Tokyo", "20260102");

        let ids = entries
            .iter()
            .map(|entry| entry.message_id.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["msg-first", "msg-last"], "{table:?}");
    }
}

#[test]
fn second_scale_time_created_disables_the_range_pushdown() {
    let fixture = fs_fixture!({});
    create_db_message_with_time(
        &fixture.path("opencode.db"),
        "msg-seconds",
        "session-a",
        1_767_312_000,
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
    );
    create_opencode_time_index(&fixture.path("opencode.db"), MessageTable::Message);

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20260101".parse().unwrap()),
        until: Some("20260103".parse().unwrap()),
        ..SharedArgs::default()
    };
    let entries = load_entries_from_directory(fixture.root(), &shared);

    assert_eq!(
        entries.len(),
        1,
        "an unrecognized time_created scale must fall back to scanning, not drop rows"
    );
    assert_eq!(entries[0].message_id.as_deref(), Some("msg-seconds"));
}

#[test]
fn detects_sources_regardless_of_the_date_window() {
    let db_only = fs_fixture!({});
    create_db_message(
        &db_only.path("opencode.db"),
        "msg-1",
        "session-a",
        r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
    );
    assert!(super::has_source(db_only.root()));

    let files_only = fs_fixture!({
        "storage/message/session-a/msg-1.json": r#"{"id":"msg-1"}"#,
    });
    assert!(super::has_source(files_only.root()));

    let empty = fs_fixture!({});
    assert!(!super::has_source(empty.root()));

    let shared = SharedArgs {
        mode: CostMode::Display,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        since: Some("20200101".parse().unwrap()),
        until: Some("20200102".parse().unwrap()),
        ..SharedArgs::default()
    };
    assert!(load_entries_from_directory(db_only.root(), &shared).is_empty());
    assert!(super::has_source(db_only.root()));
}
