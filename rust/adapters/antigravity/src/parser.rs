use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::{collections::HashMap, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use ccusage_adapter_common::{SqliteStage, UnitError};
use ccusage_core::{Diagnostics, Skip, SkipKind};

use crate::{LoadedEntry, TimestampMs, TokenUsageRaw, date_at};

const DEFAULT_MODEL: &str = "gemini-internal-model";
const PROVIDER_PREFIXES: [&str; 4] = ["google", "gemini", "vertex_ai", "openrouter/google"];
const API_PROVIDER_GOOGLE_VERTEX: u64 = 3;
const API_PROVIDER_GOOGLE_GEMINI: u64 = 24;
const API_PROVIDER_GOOGLE_EVERGREEN: u64 = 30;

#[derive(Debug, Clone)]
pub(super) struct AntigravityUsageEvent {
    pub(super) timestamp: TimestampMs,

    session_id: String,
    model: String,
    logged_model: Option<String>,
    provider: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_output_tokens: u64,
    total_tokens: u64,
    message_id: Option<String>,
    pub(super) identities: Vec<String>,
    timestamp_rank: u8,
    message_id_rank: u8,
}

#[derive(Debug, Default)]
struct GeneratorMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<TimestampMs>,
    step_indices: Vec<i64>,
}

#[derive(Debug, Default)]
struct StepMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    provider: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<TimestampMs>,
}

#[derive(Debug, Default)]
struct ModelUsage {
    model_id: Option<u64>,
    input_tokens: u64,
    total_output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    visible_output_tokens: u64,
    provider: Option<u64>,
    message_id: Option<String>,
    response_id: Option<String>,
    provider_assigned_message_id: Option<String>,
}

impl ModelUsage {
    fn is_token_bearing(&self) -> bool {
        self.input_tokens > 0
            || self.total_output_tokens > 0
            || self.cache_creation_tokens > 0
            || self.cache_read_tokens > 0
            || self.reasoning_tokens > 0
            || self.visible_output_tokens > 0
    }

    fn same_token_counts(&self, other: &Self) -> bool {
        self.model_id == other.model_id
            && self.input_tokens == other.input_tokens
            && self.total_output_tokens == other.total_output_tokens
            && self.cache_creation_tokens == other.cache_creation_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.reasoning_tokens == other.reasoning_tokens
            && self.visible_output_tokens == other.visible_output_tokens
    }

    fn is_mirror_of(&self, primary: &Self) -> bool {
        self.same_token_counts(primary)
            && self
                .identity_keys()
                .iter()
                .all(|identity| primary.identity_keys().contains(identity))
    }

    fn identity_keys(&self) -> Vec<String> {
        [
            self.response_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("response:{value}")),
            self.provider_assigned_message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("provider:{value}")),
            self.message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("message:{value}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn preferred_message_id(&self) -> (Option<String>, u8) {
        if let Some(value) = self
            .response_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return (Some(value.to_string()), 3);
        }
        if let Some(value) = self
            .provider_assigned_message_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return (Some(value.to_string()), 2);
        }
        (
            self.message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            1,
        )
    }
}

#[derive(Clone, Copy)]
enum ProtoValue<'a> {
    Varint(u64),
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32,
}

#[derive(Clone, Copy)]
struct ProtoField<'a> {
    number: u32,
    value: ProtoValue<'a>,
}

type ProtoResult<T> = std::result::Result<T, &'static str>;

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

#[derive(Debug)]
pub(super) enum ParsedDatabase {
    Events(Vec<AntigravityUsageEvent>),
    EmptySnapshot,
    NoGenerationTable,
}

pub(super) fn parse_sqlite_file(
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
    let mut statement = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
        .map_err(sqlite_failure)?;
    statement.bind((1, table)).map_err(sqlite_failure)?;
    Ok(matches!(
        statement.next().map_err(sqlite_failure)?,
        sqlite::State::Row
    ))
}

#[derive(Clone, Copy)]
struct EventContext<'a> {
    model: Option<&'a str>,
    model_id: Option<u64>,
    provider: Option<u64>,
    timestamp: Option<(TimestampMs, u8)>,
    trajectory_timestamp: Option<TimestampMs>,
    diagnostics: &'a Diagnostics,
    record: (&'a Path, u32),
    session_id: &'a str,
    fallback_identities: &'a [String],
}

fn append_usage_event(
    events: &mut Vec<AntigravityUsageEvent>,
    identity_timestamps: &mut HashMap<String, (TimestampMs, u8)>,
    usage: ModelUsage,
    context: EventContext<'_>,
) {
    if !usage.is_token_bearing() {
        return;
    }
    let mut identities = usage.identity_keys();
    if identities.is_empty() {
        identities.extend(context.fallback_identities.iter().cloned());
    }
    let Some((timestamp, timestamp_rank)) = context
        .timestamp
        .or_else(|| {
            identities
                .iter()
                .chain(context.fallback_identities)
                .find_map(|identity| identity_timestamps.get(identity).copied())
        })
        .or_else(|| context.trajectory_timestamp.map(|timestamp| (timestamp, 1)))
    else {
        context.diagnostics.record(Skip::record(
            context.record.0,
            context.record.1,
            SkipKind::InvalidTimestamp,
            "Antigravity usage record has no valid generation, step or trajectory timestamp",
        ));
        return;
    };
    let total_output_tokens = usage.total_output_tokens.max(
        usage
            .visible_output_tokens
            .saturating_add(usage.reasoning_tokens),
    );
    let output_tokens = usage
        .visible_output_tokens
        .max(total_output_tokens.saturating_sub(usage.reasoning_tokens));
    let reasoning_tokens = usage
        .reasoning_tokens
        .max(total_output_tokens.saturating_sub(output_tokens));
    let id_model = usage.model_id.and_then(model_from_id);
    let row_model = context.model.and_then(normalize_antigravity_model);
    let same_model_id = context.model_id == usage.model_id;
    let model = id_model
        .clone()
        .filter(|model| is_resolved_model(model))
        .or_else(|| {
            row_model
                .as_ref()
                .filter(|model| same_model_id && is_resolved_model(model))
                .cloned()
        })
        .or(id_model)
        .or_else(|| row_model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let logged_model = context
        .model
        .filter(|raw| *raw != model && (same_model_id || row_model.as_ref() == Some(&model)))
        .map(str::to_owned);
    let provider = usage.provider.or(context.provider);
    let (message_id, message_id_rank) = usage.preferred_message_id();
    let timestamp_value = timestamp;
    for identity in &identities {
        let should_update =
            identity_timestamps
                .get(identity)
                .is_none_or(|(old_timestamp, old_rank)| {
                    timestamp_rank > *old_rank
                        || (timestamp_rank == *old_rank && timestamp_value < *old_timestamp)
                });
        if should_update {
            identity_timestamps.insert(identity.clone(), (timestamp_value, timestamp_rank));
        }
    }
    events.push(AntigravityUsageEvent {
        timestamp,

        session_id: context.session_id.to_string(),
        model,
        logged_model,
        provider,
        input_tokens: usage.input_tokens,
        output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        reasoning_tokens,
        total_output_tokens,
        total_tokens: usage
            .input_tokens
            .saturating_add(usage.cache_creation_tokens)
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(total_output_tokens),
        message_id,
        identities,
        timestamp_rank,
        message_id_rank,
    });
}

pub(super) fn merge_usage_event(
    target: &mut AntigravityUsageEvent,
    duplicate: AntigravityUsageEvent,
) {
    target.input_tokens = target.input_tokens.max(duplicate.input_tokens);
    target.output_tokens = target.output_tokens.max(duplicate.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .max(duplicate.cache_creation_tokens);
    target.cache_read_tokens = target.cache_read_tokens.max(duplicate.cache_read_tokens);
    target.reasoning_tokens = target.reasoning_tokens.max(duplicate.reasoning_tokens);
    target.total_output_tokens = target
        .total_output_tokens
        .max(duplicate.total_output_tokens)
        .max(target.output_tokens.saturating_add(target.reasoning_tokens));
    target.total_tokens = target
        .input_tokens
        .saturating_add(target.cache_creation_tokens)
        .saturating_add(target.cache_read_tokens)
        .saturating_add(target.total_output_tokens);
    if model_resolution(&duplicate.model) > model_resolution(&target.model) {
        target.model = duplicate.model.clone();
        target.logged_model = duplicate.logged_model;
    } else if target.model == duplicate.model && target.logged_model.is_none() {
        target.logged_model = duplicate.logged_model;
    }
    if target.provider.is_none() {
        target.provider = duplicate.provider;
    }
    let use_duplicate_timestamp = duplicate.timestamp_rank > target.timestamp_rank
        || (duplicate.timestamp_rank == target.timestamp_rank
            && duplicate.timestamp < target.timestamp);
    if use_duplicate_timestamp {
        target.timestamp = duplicate.timestamp;

        target.timestamp_rank = duplicate.timestamp_rank;
    }
    if duplicate.message_id_rank > target.message_id_rank {
        target.message_id = duplicate.message_id;
        target.message_id_rank = duplicate.message_id_rank;
    }
    for identity in duplicate.identities {
        if !target.identities.contains(&identity) {
            target.identities.push(identity);
        }
    }
}

fn parse_generator_metadata(blob: &[u8]) -> ProtoResult<GeneratorMetadata> {
    let root = decode_fields(blob)?;
    let chat_model = field_bytes(&root, 1).ok_or("missing chat model field 1")?;
    let chat_model_fields = decode_fields(chat_model)?;
    let usage = field_bytes(&chat_model_fields, 4)
        .map(parse_model_usage)
        .transpose()?;
    let mut retry_usages = field_bytes_all(&chat_model_fields, 17)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    drop_primary_mirror(usage.as_ref(), &mut retry_usages);
    let timestamp = field_bytes(&chat_model_fields, 9)
        .map(parse_generation_info_timestamp)
        .transpose()?
        .flatten();
    let model = [19, 21]
        .into_iter()
        .find_map(|field| field_text(&chat_model_fields, field));
    let model_id = field_varint(&chat_model_fields, 3).filter(|value| *value != 0);
    let step_indices = parse_step_indices(&root);
    Ok(GeneratorMetadata {
        model,
        model_id,
        usage,
        retry_usages,
        timestamp,
        step_indices,
    })
}

// NOTE(2026-11-09, #2027): Antigravity's retry_infos repeats the primary usage attempt. Dropping a single mirror preserves the usage of distinct retries.
fn drop_primary_mirror(primary: Option<&ModelUsage>, retries: &mut Vec<ModelUsage>) {
    if let Some(primary) = primary
        && let Some(position) = retries.iter().position(|retry| retry.is_mirror_of(primary))
    {
        retries.remove(position);
    }
}

fn parse_step_indices(root: &[ProtoField<'_>]) -> Vec<i64> {
    let mut indices = Vec::new();
    for field in root {
        if field.number != 2 {
            continue;
        }
        match field.value {
            ProtoValue::Varint(value) => {
                if let Ok(index) = i64::try_from(value) {
                    indices.push(index);
                }
            }
            ProtoValue::Bytes(packed) => {
                let mut packed = packed;
                while !packed.is_empty() {
                    match read_varint(&mut packed) {
                        Ok(value) => {
                            if let Ok(index) = i64::try_from(value) {
                                indices.push(index);
                            } else {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            ProtoValue::Fixed32 | ProtoValue::Fixed64 => {}
        }
    }
    indices
}

fn parse_step_metadata(blob: &[u8]) -> ProtoResult<StepMetadata> {
    let fields = decode_fields(blob)?;
    let usage = field_bytes(&fields, 9).map(parse_model_usage).transpose()?;
    let mut retry_usages = field_bytes_all(&fields, 28)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    drop_primary_mirror(usage.as_ref(), &mut retry_usages);
    let model_info = field_bytes(&fields, 24)
        .map(parse_model_info)
        .transpose()?
        .unwrap_or_default();
    let timestamp = field_bytes(&fields, 8)
        .or_else(|| field_bytes(&fields, 1))
        .map(parse_timestamp_message)
        .transpose()?
        .flatten();
    Ok(StepMetadata {
        model: model_info.model,
        model_id: model_info.model_id,
        provider: model_info.provider,
        usage,
        retry_usages,
        timestamp,
    })
}

#[derive(Debug, Default)]
struct ModelInfo {
    model: Option<String>,
    model_id: Option<u64>,
    provider: Option<u64>,
}

fn parse_model_info(blob: &[u8]) -> ProtoResult<ModelInfo> {
    let fields = decode_fields(blob)?;
    Ok(ModelInfo {
        model: field_text(&fields, 12).or_else(|| field_text(&fields, 8)),
        model_id: field_varint(&fields, 1).filter(|value| *value != 0),
        provider: field_varint(&fields, 7).filter(|value| *value != 0),
    })
}

fn parse_retry_info(blob: &[u8]) -> ProtoResult<Option<ModelUsage>> {
    let fields = decode_fields(blob)?;
    field_bytes(&fields, 2).map(parse_model_usage).transpose()
}

fn parse_model_usage(blob: &[u8]) -> ProtoResult<ModelUsage> {
    let fields = decode_fields(blob)?;
    Ok(ModelUsage {
        model_id: field_varint(&fields, 1).filter(|value| *value != 0),
        input_tokens: field_varint(&fields, 2).unwrap_or(0),
        total_output_tokens: field_varint(&fields, 3).unwrap_or(0),
        cache_creation_tokens: field_varint(&fields, 4).unwrap_or(0),
        cache_read_tokens: field_varint(&fields, 5).unwrap_or(0),
        reasoning_tokens: field_varint(&fields, 9).unwrap_or(0),
        visible_output_tokens: field_varint(&fields, 10).unwrap_or(0),
        provider: field_varint(&fields, 6).filter(|value| *value != 0),
        message_id: field_text(&fields, 7),
        response_id: field_text(&fields, 11),
        provider_assigned_message_id: field_text(&fields, 12),
    })
}

fn parse_generation_info_timestamp(blob: &[u8]) -> ProtoResult<Option<TimestampMs>> {
    let generation_info = decode_fields(blob)?;
    let Some(timestamp_message) = field_bytes(&generation_info, 4) else {
        return Ok(None);
    };
    parse_timestamp_message(timestamp_message)
}

fn parse_trajectory_timestamp(blob: &[u8]) -> ProtoResult<Option<TimestampMs>> {
    let fields = decode_fields(blob)?;
    field_bytes(&fields, 2)
        .map(parse_timestamp_message)
        .transpose()
        .map(Option::flatten)
}

fn parse_timestamp_message(blob: &[u8]) -> ProtoResult<Option<TimestampMs>> {
    let timestamp_fields = decode_fields(blob)?;
    let Some(seconds) = field_varint(&timestamp_fields, 1)
        .and_then(|value| i64::try_from(value).ok())
        .filter(|seconds| *seconds > 0)
    else {
        return Ok(None);
    };
    let nanos = field_varint(&timestamp_fields, 2)
        .unwrap_or(0)
        .min(999_999_999);
    let milliseconds = seconds
        .saturating_mul(1_000)
        .saturating_add(i64::try_from(nanos / 1_000_000).unwrap_or(i64::MAX));
    Ok(Some(TimestampMs::from_millis(milliseconds)))
}

fn decode_fields(mut blob: &[u8]) -> ProtoResult<Vec<ProtoField<'_>>> {
    let mut fields = Vec::new();
    while !blob.is_empty() {
        let tag = read_varint(&mut blob)?;
        let number = u32::try_from(tag >> 3).map_err(|_| "protobuf field number overflow")?;
        if number == 0 {
            return Err("protobuf field number is zero");
        }
        let wire = tag & 7;
        let value = match wire {
            0 => ProtoValue::Varint(read_varint(&mut blob)?),
            1 => {
                take_bytes(&mut blob, 8)?;
                ProtoValue::Fixed64
            }
            2 => ProtoValue::Bytes(take_length_delimited(&mut blob)?),
            5 => {
                take_bytes(&mut blob, 4)?;
                ProtoValue::Fixed32
            }
            _ => return Err("unsupported protobuf wire type"),
        };
        fields.push(ProtoField { number, value });
    }
    Ok(fields)
}

fn read_varint(blob: &mut &[u8]) -> ProtoResult<u64> {
    let mut value = 0_u64;
    for shift in (0..10).map(|index| index * 7) {
        let byte = *blob.first().ok_or("truncated protobuf varint")?;
        *blob = &blob[1..];
        let payload = u64::from(byte & 0x7f);
        if shift == 63 && payload > 1 {
            return Err("protobuf varint overflow");
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        if shift == 63 {
            return Err("protobuf varint overflow");
        }
    }
    Err("protobuf varint overflow")
}

fn take_bytes<'a>(blob: &mut &'a [u8], length: usize) -> ProtoResult<&'a [u8]> {
    if blob.len() < length {
        return Err("truncated protobuf fixed-width value");
    }
    let (value, rest) = blob.split_at(length);
    *blob = rest;
    Ok(value)
}

fn take_length_delimited<'a>(blob: &mut &'a [u8]) -> ProtoResult<&'a [u8]> {
    let length = usize::try_from(read_varint(blob)?).map_err(|_| "protobuf length overflow")?;
    take_bytes(blob, length)
}

fn field_varint(fields: &[ProtoField<'_>], number: u32) -> Option<u64> {
    fields.iter().rev().find_map(|field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Varint(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

fn field_bytes<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Bytes(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

fn field_bytes_all<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Vec<&'a [u8]> {
    fields
        .iter()
        .filter_map(|field| match field {
            ProtoField {
                number: field_number,
                value: ProtoValue::Bytes(value),
            } if *field_number == number => Some(*value),
            _ => None,
        })
        .collect()
}

fn field_text(fields: &[ProtoField<'_>], number: u32) -> Option<String> {
    fields
        .iter()
        .rev()
        .find_map(|field| match field {
            ProtoField {
                number: field_number,
                value: ProtoValue::Bytes(value),
            } if *field_number == number => Some(*value),
            _ => None,
        })
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn model_name_from_id(model_id: u64) -> String {
    match model_id {
        246 => "gemini-2.5-pro".to_string(),
        312 => "gemini-2.5-flash".to_string(),
        313 | 329 => "gemini-2.5-flash-thinking".to_string(),
        330 => "gemini-2.5-flash-lite".to_string(),
        281 | 282 => "claude-4-sonnet".to_string(),
        290 | 291 => "claude-4-opus".to_string(),
        333 | 334 => "claude-4.5-sonnet".to_string(),
        340 | 341 => "claude-4.5-haiku".to_string(),
        342 => "model_openai_gpt_oss_120b_medium".to_string(),
        1_000.. => format!("{UNRESOLVED_PLACEHOLDER_PREFIX}{}", model_id - 1_000),
        _ => format!("{UNRESOLVED_ID_PREFIX}{model_id}"),
    }
}

fn model_from_id(model_id: u64) -> Option<String> {
    normalize_antigravity_model(&model_name_from_id(model_id))
}

fn row_model(model: Option<&str>, model_id: Option<u64>) -> Option<String> {
    model
        .filter(|model| !model.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| model_id.and_then(model_from_id))
}

const UNRESOLVED_PLACEHOLDER_PREFIX: &str = "model_placeholder_m";
const UNRESOLVED_ID_PREFIX: &str = "antigravity-model-";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ModelResolution {
    Default,
    Placeholder,
    Resolved,
}

fn model_resolution(model: &str) -> ModelResolution {
    if model == DEFAULT_MODEL {
        ModelResolution::Default
    } else if model.starts_with(UNRESOLVED_PLACEHOLDER_PREFIX)
        || model.starts_with(UNRESOLVED_ID_PREFIX)
    {
        ModelResolution::Placeholder
    } else {
        ModelResolution::Resolved
    }
}

fn is_resolved_model(model: &str) -> bool {
    model_resolution(model) == ModelResolution::Resolved
}

fn normalize_antigravity_model(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(model) = gemini_flash_effort_variant(&lower) {
        return Some(model);
    }
    let base = lower
        .find('(')
        .map_or(lower.as_str(), |index| lower[..index].trim());
    let normalized = match base {
        "gemini 3.8 flash" | "gemini 3.8 flash thinking" => "gemini-3.8-flash",
        "gemini 3.7 flash" | "gemini 3.7 flash thinking" => "gemini-3.7-flash",
        "gemini 3.7 pro" | "gemini 3.7 pro thinking" => "gemini-3.7-pro",
        "gemini 3.6 flash" | "gemini 3 flash" => "gemini-3.6-flash",
        "gemini 3.6 pro" => "gemini-3.6-pro",
        "gemini 3 pro" | "gemini 3 pro thinking" => "gemini-3-pro",
        "gemini 2.5 flash" => "gemini-2.5-flash",
        "gemini 2.5 pro" => "gemini-2.5-pro",
        "gemini 2.0 flash" | "gemini 2 flash" => "gemini-2.0-flash",
        "gemini 2.0 pro" => "gemini-2.0-pro",
        "gemini 1.5 flash" => "gemini-1.5-flash",
        "gemini 1.5 pro" => "gemini-1.5-pro",
        "model_placeholder_m318" => "gemini-3.8-flash-high",
        "model_placeholder_m319" => "gemini-3.8-flash-medium",
        "model_placeholder_m320" => "gemini-3.8-flash-low",
        "model_placeholder_m298" => "gemini-3.7-flash-high",
        "model_placeholder_m299" => "gemini-3.7-flash-medium",
        "model_placeholder_m300" => "gemini-3.7-flash-low",
        "model_placeholder_m71" => "gemini-3.6-flash-high",
        "model_placeholder_m72" => "gemini-3.6-flash-medium",
        "model_placeholder_m73" => "gemini-3.6-flash-low",
        "model_placeholder_m417" | "model_placeholder_m418" | "model_placeholder_m419" => {
            "gemini-4-argon"
        }
        "model_placeholder_m402" => "claude-opus-5-5",
        "model_placeholder_m50" => "gemini-3.1-flash-lite",
        "model_placeholder_m26" => "claude-opus-4-6",
        "model_placeholder_m35" => "claude-sonnet-4-6",
        "model_placeholder_m36" | "model_placeholder_m37" | "model_placeholder_m16" => {
            "gemini-3.1-pro"
        }
        "model_placeholder_m18" | "model_placeholder_m84" | "model_placeholder_m47" => {
            "gemini-3-flash-preview"
        }
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
        "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
        "gemini-3-flash-agent"
        | "gemini-3-flash-agent-a"
        | "gemini-3-flash-agent-b"
        | "gemini-3-flash-a"
        | "gemini-3-flash-b" => "gemini-3.5-flash-high",
        "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
        "gemini-3.5-flash-low" => "gemini-3.5-flash-medium",
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro",
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro",
        "claude 3.7 sonnet" | "claude 3.7 sonnet thinking" => "claude-3-7-sonnet",
        "claude 3.5 sonnet" => "claude-3-5-sonnet",
        "claude 3.5 haiku" => "claude-3-5-haiku",
        "claude 3 opus" => "claude-3-opus",
        _ => {
            let converted = base.replace(' ', "-");
            if converted.starts_with("gemini-")
                || converted.starts_with("claude-")
                || converted.starts_with("gpt-")
            {
                return Some(converted);
            }
            return Some(trimmed.to_string());
        }
    };
    Some(normalized.to_string())
}

pub(super) fn event_to_loaded(
    event: AntigravityUsageEvent,
    timezone: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        cache_creation_input_tokens: event.cache_creation_tokens,
        cache_read_input_tokens: event.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: event.total_output_tokens,
        cache_creation: None,
        ..usage
    };
    let extra_total_tokens = event
        .total_output_tokens
        .saturating_sub(event.output_tokens);
    let (cost, missing_pricing_model) = price_antigravity(
        &event.model,
        event.logged_model.as_deref(),
        event.provider,
        cost_usage,
        event.timestamp,
        pricing,
    );
    LoadedEntry {
        date: date_at(event.timestamp, timezone),
        timestamp: event.timestamp,
        project: Arc::from("antigravity"),
        session_id: Arc::from(event.session_id),
        project_path: Arc::from("Antigravity"),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(event.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        usage,
        message_id: event.message_id,
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

fn price_antigravity(
    model: &str,
    logged_model: Option<&str>,
    provider: Option<u64>,
    usage: TokenUsageRaw,
    timestamp: TimestampMs,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>) {
    let exact = gemini_flash_pricing_alias(model).is_some();
    let kind = if exact {
        MatchKind::Exact
    } else {
        MatchKind::Fuzzy
    };
    let candidates: Vec<_> = model_candidates(model, provider)
        .into_iter()
        .flat_map(|name| {
            let alias = crate::model_aliases::resolve_model_name(&name).into_owned();
            let extra = (exact && alias != name).then_some((alias, MatchKind::Exact));
            std::iter::once((name, kind)).chain(extra)
        })
        .collect();
    let candidates: Vec<_> = logged_model
        .map(|name| (name, MatchKind::Exact))
        .into_iter()
        .chain(candidates.iter().map(|(name, kind)| (name.as_str(), *kind)))
        .collect();
    let priced = price_entry(
        EntryPricing {
            usage,
            reasoning_tokens: 0,
            residual_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(timestamp),
        },
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(model).into_owned()),
    )
}

const GEMINI_FLASH_EFFORT_FAMILIES: [&str; 3] =
    ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-3.6-flash"];
const GEMINI_FLASH_EFFORTS: [&str; 3] = ["high", "medium", "low"];

fn gemini_flash_pricing_alias(model: &str) -> Option<&'static str> {
    let (family, effort) = model.rsplit_once('-')?;
    if !GEMINI_FLASH_EFFORTS.contains(&effort) {
        return None;
    }
    GEMINI_FLASH_EFFORT_FAMILIES
        .into_iter()
        .find(|candidate| *candidate == family)
}

fn gemini_flash_effort_variant(display_name: &str) -> Option<String> {
    let (base, rest) = display_name.split_once(" (")?;
    let effort = rest.strip_suffix(')')?;
    if base.contains('-') {
        return None;
    }
    let variant = format!("{}-{effort}", base.replace(' ', "-"));
    gemini_flash_pricing_alias(&variant)
        .is_some()
        .then_some(variant)
}

fn model_candidates(model: &str, provider: Option<u64>) -> Vec<String> {
    let mut models = vec![model];
    models.extend(gemini_flash_pricing_alias(model));

    let use_google_prefixes = matches!(
        provider,
        Some(
            API_PROVIDER_GOOGLE_VERTEX | API_PROVIDER_GOOGLE_GEMINI | API_PROVIDER_GOOGLE_EVERGREEN
        )
    );
    let mut candidates = models
        .into_iter()
        .flat_map(|model| {
            std::iter::once(model.to_string()).chain(
                use_google_prefixes
                    .then_some(PROVIDER_PREFIXES)
                    .into_iter()
                    .flatten()
                    .map(move |prefix| format!("{prefix}/{model}")),
            )
        })
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

#[cfg(test)]
pub(super) mod test_support;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let model = normalize_antigravity_model("gemini-3.6-flash-high").unwrap();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"gemini-3.6-flash":{"input_cost_per_token":0.00000075,"output_cost_per_token":0.00000375},"gemini/gemini-3.6-flash-lite":{"input_cost_per_token":0.0000001,"output_cost_per_token":0.0000004}}"#);
        let (cost, missing) = price_antigravity(
            &model,
            None,
            None,
            TokenUsageRaw {
                input_tokens: 1000,
                ..Default::default()
            },
            ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),
            PriceTable::Calculate(&pricing),
        );
        assert!((cost - 0.00075).abs() < 1e-12);
        assert_eq!(missing, None);
    }
}
