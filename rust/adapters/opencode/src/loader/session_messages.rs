use ccusage_adapter_common::{
    Split, jsonl, map_parallel,
    sqlite::{open_read_only, read_i64},
    units::UnitError,
    worker_count,
};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, PoisonError},
};

use super::{
    COLUMN_READ_FAILED, DatabaseLoadContext, DateWindow, EntryCollector, LinkedEntry, TableSchema,
    UnreadableRow, bind_window, pushdown_condition, record_missing_timestamp, record_rows_failure,
    row_diagnostics, scan_rows, sqlite_rows_failure, sqlite_setup_failure,
};
use crate::{
    debug_log,
    parser::{
        SessionMessageError, SessionMessageKind, SessionMessageRow,
        session_message_value_to_entry_checked,
    },
};

#[derive(Clone)]
struct SessionMessageMeta {
    id: String,
    session_id: String,
    kind: SessionMessageKind,
    created: Option<i64>,
    seq: Option<i64>,
}

impl SessionMessageMeta {
    fn read(statement: &sqlite::Statement<'_>) -> std::result::Result<Option<Self>, UnreadableRow> {
        let id = statement.read(0)?;
        let session_id = statement.read(1)?;
        let row_type: String = statement.read(2)?;
        let Some(kind) = SessionMessageKind::from_row_type(&row_type) else {
            return Ok(None);
        };
        Ok(Some(Self {
            id,
            session_id,
            kind,
            created: read_i64(statement, 4),
            seq: statement.read::<Option<i64>, _>(5).ok().flatten(),
        }))
    }

    // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode starts fork session counters at zero, with only fork-local usage. Copied rows must leave the aggregate fallback available.
    fn is_fork_copy(&self, fork_copies: &HashMap<String, i64>) -> bool {
        fork_copies
            .get(&self.session_id)
            .is_some_and(|cutoff| self.seq.is_some_and(|seq| seq <= *cutoff))
    }
}

struct PendingRow {
    row: u32,
    rowid: i64,
    meta: SessionMessageMeta,
}

impl PendingRow {
    fn read(
        statement: &sqlite::Statement<'_>,
        row: u32,
    ) -> std::result::Result<Option<Self>, UnreadableRow> {
        let Some(meta) = SessionMessageMeta::read(statement)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            row,
            rowid: statement.read(3)?,
            meta,
        }))
    }
}

enum RowOutcome {
    Entry(SessionMessageKind, Box<LinkedEntry>),
    Skipped,
    Unreadable,
    MissingTimestamp,
    Decode(SkipKind, String),
}

pub(super) fn load_session_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    if !schema.has_all(&["id", "session_id", "type", "data"]) {
        UnitError::unsupported_schema().record(context.db_path, context.diagnostics);
        return;
    }
    let window = schema.pushdown(context);
    let workers = parallel_workers(context);
    let parallel = (workers > 1)
        .then(|| {
            load_rows_in_parallel(
                schema,
                window,
                fork_copies,
                context,
                collector,
                workers,
                &|| open_read_only(context.db_path),
            )
        })
        .flatten();
    let Some((rows, entries)) = parallel.or_else(|| {
        load_rows_in_order(connection, schema, window, fork_copies, context, collector)
    }) else {
        return;
    };
    if rows > 0 && entries == 0 {
        debug_log(
            context.shared,
            format!(
                "OpenCode v2 rows produced no usage entries: {rows} rows in {}",
                context.db_path.display()
            ),
        );
    }
}

// NOTE(2026-11-21): Below this much database per worker, the extra pass and connections cost more than reading `data` in one scan saves. Past four connections, reads of one file contend in the kernel and get slower.
const BYTES_PER_WORKER: u64 = 32 << 20;
const MAX_WORKERS: usize = 4;

fn parallel_workers(context: &DatabaseLoadContext<'_>) -> usize {
    let bytes = std::fs::metadata(context.db_path).map_or(0, |metadata| metadata.len());
    worker_count(
        context.shared.single_thread,
        usize::try_from(bytes / BYTES_PER_WORKER).unwrap_or(usize::MAX),
    )
    .min(MAX_WORKERS)
}

fn load_rows_in_order(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    window: DateWindow,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) -> Option<(u32, u32)> {
    let statement = match prepare_session_message_query(connection, schema, window, "data") {
        Ok(statement) => statement,
        Err(error) => {
            error.record(context.db_path, context.diagnostics);
            return None;
        }
    };
    let mut entries = 0;
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        SessionMessageMeta::read,
        |row, meta, statement| {
            if meta.is_fork_copy(fork_copies) {
                return;
            }
            let outcome = match statement.read::<String, _>(3) {
                Ok(data) => parse_row(meta, data, context),
                Err(_) => RowOutcome::Unreadable,
            };
            apply_outcome(
                outcome,
                row,
                &|| row_diagnostics(statement, &diagnostics),
                context,
                collector,
                &mut entries,
            );
        },
    );
    Some((rows, entries))
}

// NOTE(2026-11-21): A 3.4 GB OpenCode database spends most of its load reading `data` overflow pages in one thread. The ordered metadata pass keeps row numbers and push order unchanged, and rounds keep only a few batches of payloads in memory.
fn load_rows_in_parallel(
    schema: &TableSchema,
    window: DateWindow,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
    workers: usize,
    open: &OpenConnection<'_>,
) -> Option<(u32, u32)> {
    // NOTE(2026-11-21): SQLite resolves `rowid` to a declared column of that name. Such a column need not identify rows, so only the ordered scan reads those tables.
    if schema.declares("rowid") {
        return None;
    }
    let (scan, lanes) = pin_snapshots(context.db_path, workers, open)?;
    let mut statement = prepare_session_message_query(&scan, schema, window, "rowid").ok()?;
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let round_rows = lanes.len() * PENDING_BATCH_ROWS;
    let mut round = Vec::with_capacity(round_rows);
    // NOTE(2026-11-21): The ordered scan stops at the first SQLite error. Metadata skips wait for their round so the ones after a worker error are dropped too, and a full round of them ends the round early.
    let mut unreadable = Vec::new();
    let (mut rows, mut entries) = (0_u32, 0_u32);
    let mut failure = None;
    while failure.is_none() {
        let done = match statement.next() {
            Ok(sqlite::State::Row) => {
                rows = rows.saturating_add(1);
                match PendingRow::read(&statement, rows) {
                    Ok(Some(pending)) if !pending.meta.is_fork_copy(fork_copies) => {
                        round.push(pending);
                    }
                    Ok(_) => {}
                    Err(UnreadableRow) => {
                        unreadable.push((rows, row_diagnostics(&statement, &diagnostics)));
                    }
                }
                false
            }
            Ok(sqlite::State::Done) => true,
            Err(error) => {
                failure = Some((rows, error));
                true
            }
        };
        if done || round.len() == round_rows || unreadable.len() == round_rows {
            if !round.is_empty()
                && let Err(failed) = apply_round(
                    &lanes,
                    &round,
                    &diagnostics,
                    context,
                    collector,
                    &mut entries,
                )
            {
                failure = Some(failed);
            }
            let last = failure.as_ref().map_or(u32::MAX, |(row, _)| *row);
            for (row, diagnostics) in unreadable.drain(..).filter(|(row, _)| *row <= last) {
                diagnostics.record(Skip::record(
                    context.db_path,
                    row,
                    SkipKind::InvalidData,
                    COLUMN_READ_FAILED,
                ));
            }
            round.clear();
        }
        if done {
            break;
        }
    }
    if let Some((row, error)) = failure {
        rows = row;
        record_rows_failure(sqlite_rows_failure(error), context, schema.table);
    }
    Some((rows, entries))
}

type OpenConnection<'a> =
    dyn Fn() -> std::result::Result<sqlite::Connection, UnitError> + Sync + 'a;
type Lane = (usize, Mutex<sqlite::Connection>);

// NOTE(2026-11-21): The metadata scan and each worker read through their own connection and snapshot. A read transaction holds each snapshot, and equal data_version reads on a probe connection before and after taking them prove no other connection committed in between, so all of them match. The same file identity right before and after every open shows they opened one file, short of the path changing twice within one open call, which needs the opened descriptor to rule out. Otherwise the ordered scan reads everything.
fn pin_snapshots(
    path: &Path,
    workers: usize,
    open: &OpenConnection<'_>,
) -> Option<(sqlite::Connection, Vec<Lane>)> {
    let identity = file_identity(path)?;
    let same_file = || file_identity(path).is_some_and(|current| current == identity);
    let open_same = || {
        same_file().then_some(())?;
        let connection = open().ok()?;
        same_file().then_some(connection)
    };
    let probe = open_same()?;
    let version = data_version(&probe)?;
    let pinned = || {
        let connection = open_same()?;
        // NOTE(2026-11-21): Each overflow page is read once. A default-sized cache per connection would only hold memory.
        step(&connection, "PRAGMA cache_size = -64")?;
        step(&connection, "BEGIN")?;
        data_version(&connection)?;
        Some(connection)
    };
    let scan = pinned()?;
    let mut lanes = Vec::with_capacity(workers);
    for lane in 0..workers {
        lanes.push((lane, Mutex::new(pinned()?)));
    }
    (data_version(&probe)? == version).then_some((scan, lanes))
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

// NOTE(2026-11-21): SQLite opens Windows files without FILE_SHARE_DELETE. The open probe keeps the path from being replaced or renamed until every snapshot is pinned.
#[cfg(not(unix))]
fn file_identity(_: &Path) -> Option<()> {
    Some(())
}

fn step<'a>(connection: &'a sqlite::Connection, sql: &str) -> Option<sqlite::Statement<'a>> {
    let mut statement = connection.prepare(sql).ok()?;
    statement.next().ok()?;
    Some(statement)
}

fn data_version(connection: &sqlite::Connection) -> Option<i64> {
    step(connection, "PRAGMA data_version")?.read(0).ok()
}

// NOTE(2026-11-21): Newer OpenCode rows carry larger payloads, and adjacent rows carry similar ones. One batch per worker per round keeps workers even and holds only one round of outcomes.
const PENDING_BATCH_ROWS: usize = 256;

fn apply_round(
    lanes: &[Lane],
    round: &[PendingRow],
    diagnostics: &Diagnostics,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
    entries: &mut u32,
) -> std::result::Result<(), (u32, sqlite::Error)> {
    let mut read = map_parallel(lanes, false, Split::ByCount, |(lane, connection)| {
        let connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
        let mut statement =
            match connection.prepare("SELECT data FROM session_message WHERE rowid = ?1") {
                Ok(statement) => statement,
                Err(error) => return vec![Err(error)].into_iter(),
            };
        let mut outcomes = Vec::new();
        for pending in round
            .chunks(PENDING_BATCH_ROWS)
            .nth(*lane)
            .unwrap_or_default()
        {
            let outcome = statement
                .reset()
                .and_then(|()| statement.bind((1, pending.rowid)))
                .and_then(|()| statement.next())
                .map(|state| match (state, statement.read::<String, _>(0)) {
                    (sqlite::State::Row, Ok(data)) => {
                        parse_row(pending.meta.clone(), data, context)
                    }
                    _ => RowOutcome::Unreadable,
                });
            let failed = outcome.is_err();
            outcomes.push(outcome);
            if failed {
                break;
            }
        }
        outcomes.into_iter()
    });
    for (index, pending) in round.iter().enumerate() {
        match read[index / PENDING_BATCH_ROWS].next() {
            Some(Ok(outcome)) => apply_outcome(
                outcome,
                pending.row,
                &|| diagnostics.for_row(Some(&pending.meta.id)),
                context,
                collector,
                entries,
            ),
            Some(Err(error)) => return Err((pending.row - 1, error)),
            None => break,
        }
    }
    Ok(())
}

fn parse_row(
    meta: SessionMessageMeta,
    data: String,
    context: &DatabaseLoadContext<'_>,
) -> RowOutcome {
    if !context.window.admits_payload(data.as_bytes()) {
        return RowOutcome::Skipped;
    }
    let kind = meta.kind;
    match session_message_value_to_entry_checked(
        SessionMessageRow {
            kind,
            data: &data,
            id: meta.id,
            session_id: meta.session_id,
            created: meta.created,
        },
        context.tz,
        context.mode,
        context.pricing,
    ) {
        Ok(Some(linked)) => RowOutcome::Entry(kind, Box::new(linked)),
        Ok(None) => RowOutcome::Skipped,
        Err(SessionMessageError::MissingTimestamp) => RowOutcome::MissingTimestamp,
        Err(SessionMessageError::Decode(error)) => {
            let (kind, detail) = jsonl::classify_decode_error(
                data.as_bytes(),
                &error,
                Some(|_: &serde_json::Value| true),
            );
            RowOutcome::Decode(kind, detail)
        }
    }
}

fn apply_outcome(
    outcome: RowOutcome,
    row: u32,
    row_diagnostics: &dyn Fn() -> Diagnostics,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
    entries: &mut u32,
) {
    let skip = match outcome {
        RowOutcome::Entry(kind, linked) => {
            *entries += 1;
            if kind == SessionMessageKind::Compaction {
                collector.push_compaction(*linked);
            } else {
                collector.push_message(*linked);
            }
            return;
        }
        RowOutcome::Skipped => return,
        RowOutcome::MissingTimestamp => {
            return record_missing_timestamp(&row_diagnostics(), context.db_path, row);
        }
        RowOutcome::Unreadable => Skip::record(
            context.db_path,
            row,
            SkipKind::InvalidData,
            COLUMN_READ_FAILED,
        ),
        RowOutcome::Decode(kind, detail) => Skip::record(context.db_path, row, kind, detail),
    };
    row_diagnostics().record(skip);
}

fn prepare_session_message_query<'a>(
    connection: &'a sqlite::Connection,
    schema: &TableSchema,
    window: DateWindow,
    payload: &str,
) -> std::result::Result<sqlite::Statement<'a>, UnitError> {
    let select = format!(
        "SELECT id, session_id, type, {payload}, {}, {} FROM session_message NOT INDEXED",
        schema.column_or("time_created", "NULL"),
        schema.column_or("seq", "NULL"),
    );
    let sql = match pushdown_condition(window) {
        Some(condition) => format!("{select} WHERE {condition}"),
        None => select,
    };
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}

#[cfg(test)]
mod tests;
