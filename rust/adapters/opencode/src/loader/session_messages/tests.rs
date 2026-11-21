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

const SQLITE_BUSY: isize = 5;

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
        Some(open) => load_rows_in_parallel(
            &schema,
            DateWindow::UNBOUNDED,
            &fork_copies,
            &context,
            &mut collector,
            4,
            open,
        ),
        None => load_rows_in_order(
            &connection,
            &schema,
            DateWindow::UNBOUNDED,
            &fork_copies,
            &context,
            &mut collector,
        ),
    };
    sqlite::open(path)
        .and_then(|writer| writer.execute("BEGIN EXCLUSIVE; COMMIT"))
        .expect("the loader released its read locks");
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
            let written = sqlite::open(&path).unwrap().execute(sql);
            if commits {
                written.unwrap();
            } else {
                assert_eq!(written.unwrap_err().code, Some(SQLITE_BUSY), "{sql}");
            }
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

fn corrupt_overflow_database(
    fixture: &ccusage_test_support::Fixture,
    corrupt_row: i64,
    before_corruption: &str,
) -> std::path::PathBuf {
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
    for index in 0..800_i64 {
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
    db.execute(format!("COMMIT; {before_corruption}")).unwrap();
    drop(db);
    let mut bytes = std::fs::read(&path).unwrap();
    let marker = format!("MARK{:04}", corrupt_row - 1);
    let marker = bytes
        .windows(marker.len())
        .position(|window| window == marker.as_bytes())
        .unwrap();
    let overflow_page = marker / 4096 * 4096;
    bytes[overflow_page..overflow_page + 4].fill(0xff);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_worker_sqlite_error_keeps_the_rows_before_it_like_the_ordered_scan() {
    let fixture = ccusage_test_support::Fixture::new();
    let path = corrupt_overflow_database(&fixture, 601, "");

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((600, 600)));
    assert_eq!(parallel, ordered);
}

#[test]
fn a_worker_sqlite_error_drops_metadata_skips_after_it_like_the_ordered_scan() {
    let fixture = ccusage_test_support::Fixture::new();
    let path = corrupt_overflow_database(
        &fixture,
        650,
        "UPDATE session_message SET session_id = NULL WHERE id IN ('m100', 'm699')",
    );

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((649, 648)));
    assert!(
        ordered
            .2
            .iter()
            .any(|line| line.ends_with("session_message:m100: database column read failed"))
    );
    assert!(
        !ordered
            .2
            .iter()
            .any(|line| line.contains("session_message:m699"))
    );
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

#[test]
fn a_declared_rowid_column_leaves_the_rows_to_the_ordered_scan() {
    let fixture = database("WAL", "");
    let path = fixture.path("opencode.db");
    sqlite::open(&path)
        .unwrap()
        .execute("ALTER TABLE session_message ADD COLUMN RowId INTEGER NOT NULL DEFAULT 1")
        .unwrap();

    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(parallel, (None, Vec::new(), Vec::new()));
    assert_eq!(load(&path, None).0, Some((700, 544)));
}

#[cfg(unix)]
#[test]
fn a_path_replaced_while_workers_take_snapshots_leaves_the_rows_to_the_ordered_scan() {
    let fixture = database("WAL", "");
    let first = fixture.path("opencode.db");
    let second = fixture.path("replacement.db");
    std::fs::copy(&first, &second).unwrap();
    let path = fixture.path("link.db");
    std::os::unix::fs::symlink(&first, &path).unwrap();
    let opened = AtomicUsize::new(0);

    let parallel = load(
        &path,
        Some(&|| {
            if opened.fetch_add(1, Ordering::Relaxed) == 2 {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&second, &path).unwrap();
            }
            open_read_only(&path)
        }),
    );

    assert_eq!(parallel, (None, Vec::new(), Vec::new()));
}

#[test]
fn a_covering_index_keeps_the_metadata_scan_in_table_order() {
    let fixture = database("WAL", "");
    let path = fixture.path("opencode.db");
    sqlite::open(&path)
        .unwrap()
        .execute(
            "CREATE INDEX session_message_covering \
             ON session_message (id, session_id, type, time_created, seq)",
        )
        .unwrap();

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((700, 544)));
    assert_eq!(parallel, ordered);
}

#[test]
fn a_full_round_of_metadata_skips_waits_for_the_payloads_before_it() {
    let fixture = ccusage_test_support::Fixture::new();
    let unreadable = (0..1500)
        .map(|index| {
            format!(
                "INSERT INTO session_message (id, session_id, type, seq, data) \
                 VALUES ('null{index}', NULL, 'assistant', {}, '{{}}');",
                1000 + index
            )
        })
        .collect::<String>();
    let path = corrupt_overflow_database(&fixture, 650, &unreadable);

    let ordered = load(&path, None);
    let parallel = load(&path, Some(&|| open_read_only(&path)));

    assert_eq!(ordered.0, Some((649, 649)));
    assert_eq!(parallel, ordered);
}

#[cfg(unix)]
#[test]
fn a_path_switched_away_and_back_while_workers_open_leaves_the_rows_to_the_ordered_scan() {
    let fixture = database("WAL", "");
    let first = fixture.path("opencode.db");
    let second = fixture.path("replacement.db");
    std::fs::copy(&first, &second).unwrap();
    let path = fixture.path("link.db");
    std::os::unix::fs::symlink(&first, &path).unwrap();
    let retarget = |target: &Path| {
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(target, &path).unwrap();
    };
    let opened = AtomicUsize::new(0);

    let parallel = load(
        &path,
        Some(&|| {
            match opened.fetch_add(1, Ordering::Relaxed) {
                2 => retarget(&second),
                3 => retarget(&first),
                _ => {}
            }
            open_read_only(&path)
        }),
    );

    assert_eq!(parallel, (None, Vec::new(), Vec::new()));
}
