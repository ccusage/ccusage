use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use ccusage_adapter_common::{sqlite::open_read_only, units::UnitError};
use ccusage_core::{Diagnostics, cost::PriceTable};

use super::{
    super::{
        DatabaseLoadContext, DateWindow, EntryCollector, Table, TableSchema,
        tests::fill_session_messages,
    },
    load_rows_in_order, load_rows_in_parallel,
};
use crate::cli::{CostMode, SharedArgs};

type Loaded = (
    Option<(u32, u32)>,
    Vec<(Option<String>, String, i64, u64)>,
    Vec<String>,
);

const SCHEMA: &str = "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, \
                      type TEXT, seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT)";

fn database(journal_mode: &str, table_options: &str) -> ccusage_test_support::Fixture {
    let fixture = ccusage_test_support::Fixture::new();
    let db = sqlite::open(fixture.path("opencode.db")).unwrap();
    db.execute(format!(
        "PRAGMA journal_mode = {journal_mode}; {SCHEMA} {table_options}"
    ))
    .unwrap();
    drop(db);
    fill_session_messages(&fixture.path("opencode.db"), 700);
    fixture
}

fn load(
    path: &Path,
    parallel: Option<&(dyn Fn() -> Result<sqlite::Connection, UnitError> + Sync)>,
) -> Loaded {
    let shared = SharedArgs {
        mode: CostMode::Display,
        debug: true,
        ..SharedArgs::default()
    };
    let diagnostics = Diagnostics::new("opencode");
    let context = DatabaseLoadContext {
        tz: &jiff::tz::TimeZone::UTC,
        mode: CostMode::Display,
        pricing: PriceTable::Display,
        window: DateWindow::UNBOUNDED,
        shared: &shared,
        db_path: path,
        diagnostics: &diagnostics,
    };
    let connection = open_read_only(path).unwrap();
    let schema =
        TableSchema::probe(&connection, Table::SessionMessage, path, &diagnostics).unwrap();
    let mut collector = EntryCollector::default();
    let fork_copies = HashMap::new();
    let rows = match parallel {
        Some(open) => {
            load_rows_in_parallel(&schema, &fork_copies, &context, &mut collector, 4, open)
        }
        None => load_rows_in_order(&connection, &schema, &fork_copies, &context, &mut collector),
    };
    connection
        .execute("BEGIN; ROLLBACK")
        .expect("the read transaction has ended");
    let entries = collector
        .messages
        .iter()
        .chain(&collector.compactions)
        .map(|(entry, _)| {
            (
                entry.message_id.clone(),
                entry.session_id.to_string(),
                entry.timestamp.as_millis(),
                entry.usage.input_tokens,
            )
        })
        .collect();
    (rows, entries, diagnostics.warnings(&shared))
}

fn open_after_a_write_at_the_first_worker(
    path: &Path,
    sql: &'static str,
    commits: bool,
) -> impl Fn() -> Result<sqlite::Connection, UnitError> + Sync {
    let opened = AtomicUsize::new(0);
    let path = path.to_path_buf();
    move || {
        if opened.fetch_add(1, Ordering::Relaxed) == 2 {
            let committed = sqlite::open(&path).and_then(|db| db.execute(sql)).is_ok();
            assert_eq!(committed, commits, "{sql}");
        }
        open_read_only(&path)
    }
}

#[test]
fn parallel_rows_match_the_ordered_scan() {
    let fixture = database("WAL", "");
    let path = fixture.path("opencode.db");
    sqlite::open(&path)
        .unwrap()
        .execute("UPDATE session_message SET data = NULL WHERE id = 'm40'")
        .unwrap();

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((700, 543)));
    assert_eq!(
        ordered.2[0],
        "WARN  opencode: skipped 79 records in 1 input (invalid data)"
    );
    assert_eq!(parallel, ordered);
}

#[test]
fn a_commit_while_workers_take_snapshots_leaves_the_rows_to_the_ordered_scan() {
    for sql in [
        "UPDATE session_message SET rowid = rowid + 100000",
        "DELETE FROM session_message WHERE id = 'm10'",
        "UPDATE session_message SET data = '{broken' WHERE id = 'm12'",
        "DELETE FROM session_message WHERE id = 'm5'; VACUUM",
    ] {
        let fixture = database("WAL", "");
        let path = fixture.path("opencode.db");

        let parallel = load(
            &path,
            Some(&open_after_a_write_at_the_first_worker(&path, sql, true)),
        );

        assert_eq!(parallel, (None, Vec::new(), Vec::new()), "{sql}");
    }
}

#[test]
fn a_rollback_journal_writer_waits_for_the_parallel_read() {
    let fixture = database("DELETE", "");
    let path = fixture.path("opencode.db");
    let ordered = load(&path, None);
    let sql = "DELETE FROM session_message WHERE id = 'm10'";

    let parallel = load(
        &path,
        Some(&open_after_a_write_at_the_first_worker(&path, sql, false)),
    );

    assert_eq!(parallel, ordered);
}

#[test]
fn a_worker_that_cannot_open_applies_no_rows() {
    let fixture = database("WAL", "");
    let path = fixture.path("opencode.db");
    let opened = AtomicUsize::new(0);

    let parallel = load(
        &path,
        Some(&|| match opened.fetch_add(1, Ordering::Relaxed) {
            0..=2 => open_read_only(&path),
            _ => Err(UnitError::unsupported_schema()),
        }),
    );

    assert_eq!(parallel, (None, Vec::new(), Vec::new()));
}

#[test]
fn a_worker_sqlite_error_keeps_the_rows_before_it_like_the_ordered_scan() {
    let fixture = ccusage_test_support::Fixture::new();
    let path = fixture.path("opencode.db");
    let db = sqlite::open(&path).unwrap();
    db.execute(format!("PRAGMA journal_mode = DELETE; {SCHEMA}; BEGIN"))
        .unwrap();
    let mut insert = db
        .prepare(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
             VALUES (?1, 's', 'assistant', ?2, 1767312000000, ?3)",
        )
        .unwrap();
    for index in 0..700_i64 {
        let content = format!("{}MARK{index:04}{}", "x".repeat(5000), "x".repeat(7000));
        let data = format!(
            r#"{{"content":"{content}","model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":1767312000000}},"tokens":{{"input":{index},"output":1}}}}"#
        );
        insert.reset().unwrap();
        insert.bind((1, format!("m{index}").as_str())).unwrap();
        insert.bind((2, index)).unwrap();
        insert.bind((3, data.as_str())).unwrap();
        insert.next().unwrap();
    }
    drop(insert);
    db.execute("COMMIT").unwrap();
    drop(db);
    let mut bytes = std::fs::read(&path).unwrap();
    let marker = bytes
        .windows(8)
        .position(|window| window == b"MARK0600")
        .unwrap();
    let overflow_page = marker / 4096 * 4096;
    bytes[overflow_page..overflow_page + 4].fill(0xff);
    std::fs::write(&path, bytes).unwrap();

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((600, 600)));
    assert_eq!(parallel, ordered);
}

#[test]
fn a_without_rowid_table_applies_no_rows() {
    let fixture = database("WAL", "WITHOUT ROWID");
    let path = fixture.path("opencode.db");

    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(parallel, (None, Vec::new(), Vec::new()));
    assert_eq!(load(&path, None).0, Some((700, 544)));
}
