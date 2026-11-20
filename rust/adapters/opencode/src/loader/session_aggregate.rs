use ccusage_adapter_common::sqlite::{read_count, read_f64, read_i64};

use super::{
    DatabaseLoadContext, TableSchema, record_missing_timestamp, row_diagnostics, scan_rows,
    sqlite_setup_failure,
};
use crate::{
    LoadedEntry, debug_log,
    parser::{MissingTimestamp, OpenCodeSessionAggregate, session_value_to_entry},
};

pub(super) fn load_session_aggregate_entries(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
) -> Vec<LoadedEntry> {
    if !schema.has_all(&[
        "id",
        "time_created",
        "cost",
        "tokens_input",
        "tokens_output",
        "tokens_cache_read",
        "tokens_cache_write",
    ]) {
        return Vec::new();
    }
    let table = schema.table.name();
    let sql = format!(
        "SELECT id, time_created, cost, tokens_input, tokens_output, \
         tokens_cache_read, tokens_cache_write, {}, {} FROM {table}",
        schema.column_or("tokens_reasoning", "0"),
        schema.column_or("model", "NULL"),
    );
    let statement = match connection.prepare(sql) {
        Ok(statement) => statement,
        Err(error) => {
            debug_log(
                context.shared,
                format!(
                    "Failed to read OpenCode {table} table: {}",
                    context.db_path.display()
                ),
            );
            sqlite_setup_failure(error).record(context.db_path, context.diagnostics);
            return Vec::new();
        }
    };
    let mut entries = Vec::new();
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    scan_rows(
        statement,
        context,
        schema.table,
        "invalid aggregate row",
        |statement| {
            let session_id = statement.read(0)?;
            let created = read_i64(statement, 1);
            let model = statement.read::<String, _>(8).ok();
            let (model, provider) = parse_session_model(model.as_deref())
                .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
            Ok(Some(OpenCodeSessionAggregate {
                session_id,
                created,
                model,
                provider,
                input_tokens: read_count(statement, 3),
                output_tokens: read_count(statement, 4),
                reasoning_tokens: read_count(statement, 7),
                cache_read_tokens: read_count(statement, 5),
                cache_write_tokens: read_count(statement, 6),
                cost: read_f64(statement, 2),
            }))
        },
        |row, aggregate, statement| match session_value_to_entry(
            aggregate,
            context.tz,
            context.mode,
            context.pricing,
        ) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(MissingTimestamp) => record_missing_timestamp(
                &row_diagnostics(statement, &diagnostics),
                context.db_path,
                row,
            ),
        },
    );
    entries
}

fn parse_session_model(value: Option<&str>) -> Option<(String, String)> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(model) = json
            .as_str()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return Some((model.to_string(), "unknown".to_string()));
        }
        if let Some(object) = json.as_object() {
            let model = object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("modelID").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|model| !model.is_empty())?;
            let provider = object
                .get("providerID")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("provider").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .unwrap_or("unknown");
            return Some((model.to_string(), provider.to_string()));
        }
    }
    Some((value.to_string(), "unknown".to_string()))
}
