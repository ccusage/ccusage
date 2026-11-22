use ccusage_adapter_common::{jsonl, units::UnitError};

use super::{
    COLUMN_READ_FAILED, DatabaseLoadContext, DateWindow, EntryCollector, TableSchema, bind_window,
    pushdown_condition, record_missing_timestamp, row_diagnostics, scan_rows, sqlite_setup_failure,
};
use crate::{
    debug_log,
    parser::{MissingTimestamp, OpenCodeMessage, message_value_to_entry},
};

struct MessageRecord {
    id: String,
    session_id: String,
    data: String,
}

pub(super) fn load_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    let statement = match prepare_message_query(connection, schema.pushdown(context)) {
        Ok(statement) => statement,
        Err(error) => {
            error.record(context.db_path, context.diagnostics);
            debug_log(
                context.shared,
                format!(
                    "Failed to read OpenCode database: {}",
                    context.db_path.display()
                ),
            );
            return;
        }
    };
    let window = context.window;
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        |statement| {
            Ok(Some(MessageRecord {
                id: statement.read(0)?,
                session_id: statement.read(1)?,
                data: statement.read(2)?,
            }))
        },
        |row,
         MessageRecord {
             id,
             session_id,
             data,
         },
         statement| {
            if !window.admits_payload(data.as_bytes()) {
                return;
            }
            let Some(value) = jsonl::decode_record::<OpenCodeMessage>(
                data.as_bytes(),
                context.db_path,
                row,
                Some(&id),
                &diagnostics,
                Some(|_| true),
            ) else {
                return;
            };
            match message_value_to_entry(
                value,
                Some(id),
                Some(session_id),
                context.tz,
                context.mode,
                context.pricing,
            ) {
                Ok(Some(entry)) => collector.push_message((entry, None)),
                Ok(None) => {}
                Err(MissingTimestamp) => record_missing_timestamp(
                    &row_diagnostics(statement, &diagnostics),
                    context.db_path,
                    row,
                ),
            }
        },
    );
}

// NOTE(2026-07-27, #1492): OpenCode indexes message IDs with (session_id, time_created, id). Select IDs from the covering index before fetching payload blobs.
fn prepare_message_query(
    connection: &sqlite::Connection,
    window: DateWindow,
) -> std::result::Result<sqlite::Statement<'_>, UnitError> {
    let select = "SELECT id, session_id, data FROM message";
    let sql = match pushdown_condition(window) {
        Some(condition) => {
            format!("{select} WHERE id IN (SELECT id FROM message WHERE {condition})")
        }
        None => select.to_string(),
    };
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}
