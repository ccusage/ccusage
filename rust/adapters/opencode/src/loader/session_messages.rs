use ccusage_adapter_common::{
    jsonl,
    sqlite::{open_read_only, read_i64},
    units::UnitError,
    worker_count,
};
use ccusage_core::{Diagnostics, Skip, SkipKind, parallel::spawn_scoped_worker};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use super::{
    COLUMN_READ_FAILED, DatabaseLoadContext, EntryCollector, LinkedEntry, TableSchema,
    UnreadableRow, bind_window, pushdown_condition, record_missing_timestamp, row_diagnostics,
    scan_rows, sqlite_setup_failure,
};
use crate::{
    debug_log,
    parser::{
        SessionMessageError, SessionMessageKind, SessionMessageRow,
        session_message_value_to_entry_checked,
    },
};

struct SessionMessageRecord {
    id: String,
    session_id: String,
    kind: SessionMessageKind,
    data: String,
    created: Option<i64>,
    seq: Option<i64>,
}

impl SessionMessageRecord {
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
            data: statement.read(3)?,
            created: read_i64(statement, 4),
            seq: statement.read::<Option<i64>, _>(5).ok().flatten(),
        }))
    }
}

/// A usage row found by the metadata pass, whose `data` a worker reads later by `rowid`.
struct PendingRow {
    row: u32,
    rowid: i64,
    id: String,
    session_id: String,
    kind: SessionMessageKind,
    created: Option<i64>,
    seq: Option<i64>,
}

impl PendingRow {
    fn read(statement: &sqlite::Statement<'_>) -> std::result::Result<Option<Self>, UnreadableRow> {
        let id = statement.read(0)?;
        let session_id = statement.read(1)?;
        let row_type: String = statement.read(2)?;
        let Some(kind) = SessionMessageKind::from_row_type(&row_type) else {
            return Ok(None);
        };
        Ok(Some(Self {
            row: 0,
            rowid: statement.read(3)?,
            id,
            session_id,
            kind,
            created: read_i64(statement, 4),
            seq: statement.read::<Option<i64>, _>(5).ok().flatten(),
        }))
    }
}

enum RowOutcome {
    Entry(SessionMessageKind, Box<LinkedEntry>),
    Skipped,
    Unreadable,
    MissingTimestamp,
    Decode(serde_json::Error, String),
}

pub(super) fn load_session_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    let parallel = (!context.shared.single_thread)
        .then(|| load_rows_in_parallel(connection, schema, fork_copies, context, collector))
        .flatten();
    let Some((rows, entries)) = parallel
        .or_else(|| load_rows_in_order(connection, schema, fork_copies, context, collector))
    else {
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

fn load_rows_in_order(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) -> Option<(u32, u32)> {
    let statement = match prepare_session_message_query(connection, schema, context, "data") {
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
        SessionMessageRecord::read,
        |row, record, statement| {
            if is_fork_copy(fork_copies, &record.session_id, record.seq) {
                return;
            }
            let outcome = parse_row(
                record.kind,
                record.data,
                record.id,
                record.session_id,
                record.created,
                context,
            );
            apply_outcome(
                outcome,
                row,
                || row_diagnostics(statement, &diagnostics),
                context,
                collector,
                &mut entries,
            );
        },
    );
    Some((rows, entries))
}

// NOTE(2026-11-21): A 3.4 GB OpenCode database spends most of its load reading `data` overflow pages in one thread. The ordered metadata pass keeps row numbers and push order unchanged.
fn load_rows_in_parallel(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) -> Option<(u32, u32)> {
    // A WITHOUT ROWID table cannot prepare this query, so the ordered scan reads it instead.
    let statement = prepare_session_message_query(connection, schema, context, "rowid").ok()?;
    let mut pending = Vec::new();
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        PendingRow::read,
        |row, mut record, _| {
            if !is_fork_copy(fork_copies, &record.session_id, record.seq) {
                record.row = row;
                pending.push(record);
            }
        },
    );
    let batches = pending.chunks(PENDING_BATCH_ROWS).collect::<Vec<_>>();
    let results = read_batches_in_parallel(&batches, context);
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let mut entries = 0;
    for (chunk, outcomes) in batches.into_iter().zip(results) {
        let outcomes = match outcomes.or_else(|_| read_pending_rows(connection, chunk, context)) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                error.record(context.db_path, context.diagnostics);
                continue;
            }
        };
        for (pending, outcome) in chunk.iter().zip(outcomes) {
            apply_outcome(
                outcome,
                pending.row,
                || diagnostics.for_row(Some(&pending.id)),
                context,
                collector,
                &mut entries,
            );
        }
    }
    Some((rows, entries))
}

// Newer rows carry far larger payloads, so workers pull small batches instead of fixed ranges.
const PENDING_BATCH_ROWS: usize = 256;

type BatchOutcomes = std::result::Result<Vec<RowOutcome>, UnitError>;

fn read_batches_in_parallel(
    batches: &[&[PendingRow]],
    context: &DatabaseLoadContext<'_>,
) -> Vec<BatchOutcomes> {
    let next = AtomicUsize::new(0);
    let mut results = thread::scope(|scope| {
        (0..worker_count(false, batches.len()))
            .map(|_| {
                spawn_scoped_worker(scope, || {
                    let own = open_read_only(context.db_path);
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(batch) = batches.get(index) else {
                            break done;
                        };
                        let outcomes = own
                            .as_ref()
                            .map_err(Clone::clone)
                            .and_then(|own| read_pending_rows(own, batch, context));
                        done.push((index, outcomes));
                    }
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect::<Vec<_>>()
    });
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, outcomes)| outcomes).collect()
}

fn read_pending_rows(
    connection: &sqlite::Connection,
    rows: &[PendingRow],
    context: &DatabaseLoadContext<'_>,
) -> std::result::Result<Vec<RowOutcome>, UnitError> {
    let mut statement = connection
        .prepare("SELECT data FROM session_message WHERE rowid = ?1")
        .map_err(sqlite_setup_failure)?;
    Ok(rows
        .iter()
        .map(|pending| {
            let found = statement
                .reset()
                .and_then(|()| statement.bind((1, pending.rowid)))
                .and_then(|()| statement.next());
            match found {
                Ok(sqlite::State::Row) => match statement.read::<String, _>(0) {
                    Ok(data) => parse_row(
                        pending.kind,
                        data,
                        pending.id.clone(),
                        pending.session_id.clone(),
                        pending.created,
                        context,
                    ),
                    Err(_) => RowOutcome::Unreadable,
                },
                // OpenCode deleted the row after the metadata pass listed it.
                Ok(sqlite::State::Done) => RowOutcome::Skipped,
                Err(_) => RowOutcome::Unreadable,
            }
        })
        .collect())
}

// NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode starts fork session counters at zero, with only fork-local usage. Copied rows must leave the aggregate fallback available.
fn is_fork_copy(fork_copies: &HashMap<String, i64>, session_id: &str, seq: Option<i64>) -> bool {
    fork_copies
        .get(session_id)
        .is_some_and(|cutoff| seq.is_some_and(|seq| seq <= *cutoff))
}

fn parse_row(
    kind: SessionMessageKind,
    data: String,
    id: String,
    session_id: String,
    created: Option<i64>,
    context: &DatabaseLoadContext<'_>,
) -> RowOutcome {
    if !context.window.admits_payload(data.as_bytes()) {
        return RowOutcome::Skipped;
    }
    match session_message_value_to_entry_checked(
        SessionMessageRow {
            kind,
            data: &data,
            id,
            session_id,
            created,
        },
        context.tz,
        context.mode,
        context.pricing,
    ) {
        Ok(Some(linked)) => RowOutcome::Entry(kind, Box::new(linked)),
        Ok(None) => RowOutcome::Skipped,
        Err(SessionMessageError::MissingTimestamp) => RowOutcome::MissingTimestamp,
        Err(SessionMessageError::Decode(error)) => RowOutcome::Decode(error, data),
    }
}

fn apply_outcome(
    outcome: RowOutcome,
    row: u32,
    row_diagnostics: impl FnOnce() -> Diagnostics,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
    entries: &mut u32,
) {
    match outcome {
        RowOutcome::Entry(kind, linked) => {
            *entries += 1;
            if kind == SessionMessageKind::Compaction {
                collector.push_compaction(*linked);
            } else {
                collector.push_message(*linked);
            }
        }
        RowOutcome::Skipped => {}
        RowOutcome::Unreadable => row_diagnostics().record(Skip::record(
            context.db_path,
            row,
            SkipKind::InvalidData,
            COLUMN_READ_FAILED,
        )),
        RowOutcome::MissingTimestamp => {
            record_missing_timestamp(&row_diagnostics(), context.db_path, row);
        }
        RowOutcome::Decode(error, data) => jsonl::record_decode_error(
            data.as_bytes(),
            error,
            context.db_path,
            row,
            jsonl::Ending::COMPLETE,
            &row_diagnostics(),
            Some(|_: &serde_json::Value| true),
        ),
    }
}

fn prepare_session_message_query<'a>(
    connection: &'a sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
    payload: &str,
) -> std::result::Result<sqlite::Statement<'a>, UnitError> {
    if !schema.has_all(&["id", "session_id", "type", "data"]) {
        return Err(UnitError::unsupported_schema());
    }
    let window = schema.pushdown(context);
    let select = format!(
        "SELECT id, session_id, type, {payload}, {}, {} FROM session_message",
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
