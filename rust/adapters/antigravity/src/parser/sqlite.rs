use std::{collections::HashMap, path::Path};

use ccusage_adapter_common::{SqliteStage, UnitError};
use ccusage_core::{Diagnostics, Skip, SkipKind};

use crate::TimestampMs;

use super::{
    EventContext, GeneratorMetadata, ModelUsage, ParsedDatabase, StepMetadata, append_usage_event,
    model::{model_from_id, row_model},
    proto::{parse_generator_metadata, parse_step_metadata, parse_trajectory_timestamp},
};

type DatabaseResult<T> = std::result::Result<T, UnitError>;

fn sqlite_failure(error: sqlite::Error) -> UnitError {
    UnitError::sqlite(SqliteStage::Setup, error.code)
}

fn invalid_row(detail: &'static str) -> impl FnOnce(sqlite::Error) -> UnitError {
    move |_| UnitError::new(SkipKind::InvalidData, detail)
}

fn undecoded(detail: &'static str) -> impl FnOnce(&'static str) -> UnitError {
    move |_| UnitError::new(SkipKind::InvalidData, detail)
}

pub(crate) fn parse_sqlite_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> DatabaseResult<ParsedDatabase> {
    let connection = ccusage_adapter_common::sqlite::open_read_only(path)?;
    connection
        .execute("BEGIN DEFERRED TRANSACTION")
        .map_err(sqlite_failure)?;
    if database_snapshot_is_empty(&connection)? {
        return Ok(ParsedDatabase::EmptySnapshot);
    }
    // NOTE(2026-08-31, #1677): Antigravity stores GeneratorMetadata in gen_metadata and can also store CortexStepMetadata in steps. The optional trajectory_metadata_blob supplies a fallback timestamp for undated usage.
    if !table_exists(&connection, "gen_metadata")? {
        if table_exists(&connection, "steps")?
            || table_exists(&connection, "trajectory_metadata_blob")?
        {
            return Err(UnitError::unsupported_schema());
        }
        return Ok(ParsedDatabase::NoGenerationTable);
    }
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let trajectory_timestamp = read_trajectory_timestamp(&connection)?;
    let generation_rows = read_generation_rows(&connection)?;
    let step_rows = read_step_rows(&connection)?;
    let generation_model = generation_rows
        .iter()
        .rev()
        .find_map(|(_, metadata)| row_model(metadata.model.as_deref(), metadata.model_id));

    let mut events = Vec::new();
    let mut identity_timestamps = HashMap::new();
    let timed_steps = step_rows.iter().filter_map(|(idx, metadata)| {
        metadata.timestamp.map(|timestamp| {
            (
                timestamp,
                vec![format!("step:{session_id}:{idx}")],
                metadata.usage.iter().chain(&metadata.retry_usages),
            )
        })
    });
    let timed_generations = generation_rows.iter().filter_map(|(idx, metadata)| {
        metadata.timestamp.map(|timestamp| {
            (
                timestamp,
                generation_identities(&session_id, *idx, &metadata.step_indices),
                metadata.usage.iter().chain(&metadata.retry_usages),
            )
        })
    });
    for (timestamp, identities, usages) in timed_steps.chain(timed_generations) {
        for identity in identities
            .into_iter()
            .chain(usages.flat_map(ModelUsage::identity_keys))
        {
            let known = identity_timestamps
                .entry(identity)
                .or_insert((timestamp, 3));
            known.0 = known.0.min(timestamp);
        }
    }
    let step_diagnostics = diagnostics.for_table("steps");
    let generation_diagnostics = diagnostics.for_table("gen_metadata");
    for (ordinal, (idx, metadata)) in step_rows.into_iter().enumerate() {
        let model = row_model(metadata.model.as_deref(), metadata.model_id)
            .or_else(|| generation_model.clone());
        let timestamp = metadata.timestamp.map(|timestamp| (timestamp, 3));
        let fallback_identities = [format!("step:{session_id}:{idx}")];
        if let Some(usage) = metadata.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: metadata.model_id,
                    provider: metadata.provider,
                    timestamp,
                    trajectory_timestamp,
                    diagnostics: &step_diagnostics,
                    record: (path, Skip::line_number(ordinal)),
                    session_id: &session_id,
                    fallback_identities: &fallback_identities,
                },
            );
        }
        for usage in metadata.retry_usages {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: metadata.model_id,
                    provider: metadata.provider,
                    timestamp,
                    trajectory_timestamp,
                    diagnostics: &step_diagnostics,
                    record: (path, Skip::line_number(ordinal)),
                    session_id: &session_id,
                    fallback_identities: &[],
                },
            );
        }
    }

    let mut current_model = None;
    for (ordinal, (idx, metadata)) in generation_rows.into_iter().enumerate() {
        let resolved_model =
            row_model(metadata.model.as_deref(), metadata.model_id).or_else(|| {
                metadata
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.model_id)
                    .and_then(model_from_id)
            });
        if let Some(model) = resolved_model {
            current_model = Some(model);
        }
        let model = current_model.clone();
        let fallback_identities = generation_identities(&session_id, idx, &metadata.step_indices);
        if let Some(usage) = metadata.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: metadata.model_id,
                    provider: None,
                    timestamp: metadata.timestamp.map(|timestamp| (timestamp, 3)),
                    trajectory_timestamp,
                    diagnostics: &generation_diagnostics,
                    record: (path, Skip::line_number(ordinal)),
                    session_id: &session_id,
                    fallback_identities: &fallback_identities,
                },
            );
        }
        for usage in metadata.retry_usages {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: metadata.model_id,
                    provider: None,
                    timestamp: metadata.timestamp.map(|timestamp| (timestamp, 3)),
                    trajectory_timestamp,
                    diagnostics: &generation_diagnostics,
                    record: (path, Skip::line_number(ordinal)),
                    session_id: &session_id,
                    fallback_identities: &[],
                },
            );
        }
    }
    Ok(ParsedDatabase::Events(events))
}

fn generation_identities(session_id: &str, idx: i64, step_indices: &[i64]) -> Vec<String> {
    if step_indices.is_empty() {
        vec![format!("gen:{session_id}:{idx}")]
    } else {
        step_indices
            .iter()
            .map(|idx| format!("step:{session_id}:{idx}"))
            .collect()
    }
}

fn database_snapshot_is_empty(connection: &sqlite::Connection) -> DatabaseResult<bool> {
    let mut statement = connection
        .prepare("PRAGMA page_count")
        .map_err(sqlite_failure)?;
    if !matches!(
        statement.next().map_err(sqlite_failure)?,
        sqlite::State::Row
    ) {
        return Err(UnitError::new(
            SkipKind::Invariant,
            "database page count is unavailable",
        ));
    }
    let page_count = statement
        .read::<i64, _>(0)
        .map_err(invalid_row("database page count is not a number"))?;
    Ok(page_count == 0)
}

fn read_generation_rows(
    connection: &sqlite::Connection,
) -> DatabaseResult<Vec<(i64, GeneratorMetadata)>> {
    let mut statement = connection
        .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
        .map_err(sqlite_failure)?;
    let mut rows = Vec::new();
    while let sqlite::State::Row = statement.next().map_err(sqlite_failure)? {
        let idx = statement
            .read::<i64, _>(0)
            .map_err(invalid_row("generation metadata row has invalid fields"))?;
        let blob = statement
            .read::<Vec<u8>, _>(1)
            .map_err(invalid_row("generation metadata row has invalid fields"))?;
        let metadata = parse_generator_metadata(&blob)
            .map_err(undecoded("generation metadata did not decode"))?;
        rows.push((idx, metadata));
    }
    Ok(rows)
}

fn read_step_rows(connection: &sqlite::Connection) -> DatabaseResult<Vec<(i64, StepMetadata)>> {
    if !table_exists(connection, "steps")? {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT idx, metadata FROM steps \
             WHERE metadata IS NOT NULL ORDER BY idx ASC",
        )
        .map_err(sqlite_failure)?;
    let mut rows = Vec::new();
    while let sqlite::State::Row = statement.next().map_err(sqlite_failure)? {
        let idx = statement
            .read::<i64, _>(0)
            .map_err(invalid_row("step row has invalid fields"))?;
        let blob = statement
            .read::<Vec<u8>, _>(1)
            .map_err(invalid_row("step row has invalid fields"))?;
        let metadata =
            parse_step_metadata(&blob).map_err(undecoded("step metadata did not decode"))?;
        rows.push((idx, metadata));
    }
    Ok(rows)
}

fn read_trajectory_timestamp(
    connection: &sqlite::Connection,
) -> DatabaseResult<Option<TimestampMs>> {
    if !table_exists(connection, "trajectory_metadata_blob")? {
        return Ok(None);
    }
    let mut statement = connection
        .prepare("SELECT data FROM trajectory_metadata_blob ORDER BY rowid ASC")
        .map_err(sqlite_failure)?;
    let mut timestamp = None;
    while let sqlite::State::Row = statement.next().map_err(sqlite_failure)? {
        let blob = statement
            .read::<Vec<u8>, _>(0)
            .map_err(invalid_row("trajectory metadata row has invalid fields"))?;
        let candidate = parse_trajectory_timestamp(&blob)
            .map_err(undecoded("trajectory metadata did not decode"))?;
        if timestamp.is_none() {
            timestamp = candidate;
        }
    }
    Ok(timestamp)
}

fn table_exists(connection: &sqlite::Connection, table: &str) -> DatabaseResult<bool> {
    ccusage_adapter_common::sqlite::table_exists(connection, table).map_err(sqlite_failure)
}
