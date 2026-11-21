use ccusage_adapter_common::{
    jsonl, map_parallel_with_state,
    sqlite::{open_read_only, read_i64},
    units::UnitError,
};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use std::collections::HashMap;

use super::{
    COLUMN_READ_FAILED, DatabaseLoadContext, EntryCollector, LinkedEntry, TableSchema,
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
    // NOTE(2026-11-21): A WITHOUT ROWID table cannot prepare this query. The ordered scan prepares its own query and records any other setup error.
    let statement = prepare_session_message_query(connection, schema, context, "rowid").ok()?;
    let mut pending = Vec::new();
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        |statement| {
            let Some(meta) = SessionMessageMeta::read(statement)? else {
                return Ok(None);
            };
            Ok(Some((statement.read::<i64, _>(3)?, meta)))
        },
        |row, (rowid, meta), _| {
            if !meta.is_fork_copy(fork_copies) {
                pending.push(PendingRow { row, rowid, meta });
            }
        },
    );
    let batches = pending.chunks(PENDING_BATCH_ROWS).collect::<Vec<_>>();
    let results = if batches.len() <= 1 {
        batches
            .iter()
            .map(|batch| read_pending_rows(connection, batch, context))
            .collect()
    } else {
        map_parallel_with_state(
            &batches,
            context.shared.single_thread,
            || open_read_only(context.db_path),
            |own, batch| {
                own.as_ref()
                    .map_err(Clone::clone)
                    .and_then(|own| read_pending_rows(own, batch, context))
            },
        )
    };
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let mut entries = 0;
    for (batch, outcomes) in batches.into_iter().zip(results) {
        let outcomes = match outcomes.or_else(|_| read_pending_rows(connection, batch, context)) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                record_rows_failure(error, context, schema.table);
                break;
            }
        };
        for (pending, outcome) in batch.iter().zip(outcomes) {
            apply_outcome(
                outcome,
                pending.row,
                || diagnostics.for_row(Some(&pending.meta.id)),
                context,
                collector,
                &mut entries,
            );
        }
    }
    Some((rows, entries))
}

// NOTE(2026-11-21): Newer OpenCode rows carry far larger payloads than older ones. Workers pull small batches so one worker does not end up with all the large rows.
const PENDING_BATCH_ROWS: usize = 256;

fn read_pending_rows(
    connection: &sqlite::Connection,
    rows: &[PendingRow],
    context: &DatabaseLoadContext<'_>,
) -> std::result::Result<Vec<RowOutcome>, UnitError> {
    // NOTE(2026-11-21): The metadata pass and this read run in separate snapshots, and OpenCode's text-keyed table reuses or renumbers rowids after deletes or VACUUM. Matching the id as well keeps a reused rowid from attaching another message's payload.
    let mut statement = connection
        .prepare("SELECT data FROM session_message WHERE rowid = ?1 AND id = ?2")
        .map_err(sqlite_setup_failure)?;
    rows.iter()
        .map(|pending| {
            statement
                .reset()
                .and_then(|()| statement.bind((1, pending.rowid)))
                .and_then(|()| statement.bind((2, pending.meta.id.as_str())))
                .and_then(|()| statement.next())
                .map_err(sqlite_rows_failure)
                .map(|state| match state {
                    sqlite::State::Row => match statement.read::<String, _>(0) {
                        Ok(data) => parse_row(pending.meta.clone(), data, context),
                        Err(_) => RowOutcome::Unreadable,
                    },
                    sqlite::State::Done => RowOutcome::Skipped,
                })
        })
        .collect()
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
    row_diagnostics: impl FnOnce() -> Diagnostics,
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

#[cfg(test)]
mod tests {
    use ccusage_core::{Diagnostics, cost::PriceTable};

    use super::{
        super::{DatabaseLoadContext, DateWindow},
        PendingRow, RowOutcome, SessionMessageKind, SessionMessageMeta, read_pending_rows,
    };
    use crate::cli::{CostMode, SharedArgs};

    #[test]
    fn a_reused_rowid_does_not_attach_another_message_payload() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("opencode.db");
        let connection = sqlite::open(&path).unwrap();
        connection
            .execute(
                "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, data TEXT); \
                 INSERT INTO session_message (rowid, id, session_id, type, data) VALUES \
                 (1, 'replacement', 'session', 'assistant', \
                  '{\"time\":{\"created\":1767312000000},\"model\":{\"id\":\"gpt-test\",\"providerID\":\"openai\"},\"tokens\":{\"input\":3,\"output\":1}}')",
            )
            .unwrap();
        let shared = SharedArgs::default();
        let diagnostics = Diagnostics::new("opencode");
        let context = DatabaseLoadContext {
            tz: &jiff::tz::TimeZone::UTC,
            mode: CostMode::Display,
            pricing: PriceTable::Display,
            window: DateWindow::UNBOUNDED,
            shared: &shared,
            db_path: &path,
            diagnostics: &diagnostics,
        };
        let pending = |id: &str| PendingRow {
            row: 1,
            rowid: 1,
            meta: SessionMessageMeta {
                id: id.to_string(),
                session_id: "session".to_string(),
                kind: SessionMessageKind::Assistant,
                created: Some(1_767_312_000_000),
                seq: None,
            },
        };

        let outcomes = read_pending_rows(
            &connection,
            &[pending("deleted"), pending("replacement")],
            &context,
        )
        .unwrap();

        assert!(matches!(outcomes[0], RowOutcome::Skipped));
        assert!(matches!(outcomes[1], RowOutcome::Entry(..)));
    }
}
