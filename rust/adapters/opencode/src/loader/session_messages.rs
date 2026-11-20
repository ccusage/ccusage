use ccusage_adapter_common::{jsonl, sqlite::read_i64, units::UnitError};
use std::collections::HashMap;

use super::{
    COLUMN_READ_FAILED, DatabaseLoadContext, EntryCollector, TableSchema, UnreadableRow,
    bind_window, pushdown_condition, record_missing_timestamp, row_diagnostics, scan_rows,
    sqlite_setup_failure,
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

pub(super) fn load_session_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    let statement = match prepare_session_message_query(connection, schema, context) {
        Ok(statement) => statement,
        Err(error) => {
            error.record(context.db_path, context.diagnostics);
            return;
        }
    };
    let window = context.window;
    let mut entries = 0;
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        SessionMessageRecord::read,
        |row, record, statement| {
            // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode starts fork session counters at zero, with only fork-local usage. Copied rows must leave the aggregate fallback available.
            if fork_copies
                .get(&record.session_id)
                .is_some_and(|cutoff| record.seq.is_some_and(|seq| seq <= *cutoff))
            {
                return;
            }
            if !window.admits_payload(record.data.as_bytes()) {
                return;
            }
            let SessionMessageRecord {
                id,
                session_id,
                kind,
                data,
                created,
                ..
            } = record;
            let linked = match session_message_value_to_entry_checked(
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
                Ok(Some(linked)) => linked,
                Ok(None) => return,
                Err(SessionMessageError::MissingTimestamp) => {
                    record_missing_timestamp(
                        &row_diagnostics(statement, &diagnostics),
                        context.db_path,
                        row,
                    );
                    return;
                }
                Err(SessionMessageError::Decode(error)) => {
                    jsonl::record_decode_error(
                        data.as_bytes(),
                        error,
                        context.db_path,
                        row,
                        jsonl::Ending::COMPLETE,
                        &row_diagnostics(statement, &diagnostics),
                        Some(|_: &serde_json::Value| true),
                    );
                    return;
                }
            };
            entries += 1;
            if kind == SessionMessageKind::Compaction {
                collector.push_compaction(linked);
            } else {
                collector.push_message(linked);
            }
        },
    );
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

fn prepare_session_message_query<'a>(
    connection: &'a sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
) -> std::result::Result<sqlite::Statement<'a>, UnitError> {
    if !schema.has_all(&["id", "session_id", "type", "data"]) {
        return Err(UnitError::unsupported_schema());
    }
    let window = schema.pushdown(context);
    let select = format!(
        "SELECT id, session_id, type, data, {}, {} FROM session_message",
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
