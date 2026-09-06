use std::{collections::HashMap, fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage_at, cli::CostMode, cli_error, format_date_tz,
    missing_pricing_model_for_candidates,
};

const DEFAULT_MODEL: &str = "gemini-internal-model";
const PROVIDER_PREFIXES: [&str; 4] = ["google", "gemini", "vertex_ai", "openrouter/google"];
const API_PROVIDER_GOOGLE_VERTEX: u64 = 3;
const API_PROVIDER_GOOGLE_GEMINI: u64 = 24;
const API_PROVIDER_GOOGLE_EVERGREEN: u64 = 30;

#[derive(Debug, Clone)]
pub(super) struct AntigravityUsageEvent {
    pub(super) timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    model: String,
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

/// Parses one Antigravity conversation database in ascending generation order.
pub(super) fn parse_sqlite_file(path: &Path) -> Result<Vec<AntigravityUsageEvent>> {
    let fallback_timestamp = file_modified_timestamp(path);
    let connection =
        sqlite::Connection::open_with_flags(path, sqlite::OpenFlags::new().with_read_only())
            .map_err(|error| {
                cli_error(format!(
                    "Failed to open Antigravity database '{}': {error}",
                    path.display()
                ))
            })?;
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let trajectory_timestamp = read_trajectory_timestamp(&connection, path)?;
    let generation_rows = read_generation_rows(&connection, path)?;
    let step_rows = read_step_rows(&connection, path)?;
    let generation_model = generation_rows.iter().rev().find_map(|(_, metadata)| {
        metadata
            .model
            .as_deref()
            .and_then(normalize_antigravity_model)
            .or_else(|| {
                metadata
                    .model_id
                    .map(model_name_from_id)
                    .and_then(|model| normalize_antigravity_model(&model))
            })
    });

    let mut events = Vec::new();
    let mut identity_timestamps = HashMap::new();
    for (_idx, metadata) in step_rows {
        let model = metadata
            .model
            .as_deref()
            .and_then(normalize_antigravity_model)
            .or_else(|| {
                metadata
                    .model_id
                    .map(model_name_from_id)
                    .and_then(|model| normalize_antigravity_model(&model))
            })
            .or_else(|| generation_model.clone());
        let timestamp = metadata.timestamp.map(|timestamp| (timestamp, 3));
        if let Some(usage) = metadata.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    provider: metadata.provider,
                    timestamp,
                    trajectory_timestamp,
                    fallback_timestamp,
                    session_id: &session_id,
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
                    provider: metadata.provider,
                    timestamp,
                    trajectory_timestamp,
                    fallback_timestamp,
                    session_id: &session_id,
                },
            );
        }
    }

    let mut current_model = None;
    for (_idx, metadata) in generation_rows {
        let row_model = metadata
            .model
            .as_deref()
            .and_then(normalize_antigravity_model)
            .or_else(|| {
                metadata
                    .model_id
                    .map(model_name_from_id)
                    .and_then(|model| normalize_antigravity_model(&model))
            })
            .or_else(|| {
                metadata.usage.as_ref().and_then(|usage| {
                    usage
                        .model_id
                        .map(model_name_from_id)
                        .and_then(|model| normalize_antigravity_model(&model))
                })
            });
        if let Some(model) = row_model {
            current_model = Some(model);
        }
        let model = current_model.clone();
        if let Some(usage) = metadata.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    provider: None,
                    timestamp: metadata.timestamp.map(|timestamp| (timestamp, 3)),
                    trajectory_timestamp,
                    fallback_timestamp,
                    session_id: &session_id,
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
                    provider: None,
                    timestamp: metadata.timestamp.map(|timestamp| (timestamp, 3)),
                    trajectory_timestamp,
                    fallback_timestamp,
                    session_id: &session_id,
                },
            );
        }
    }
    Ok(events)
}

fn read_generation_rows(
    connection: &sqlite::Connection,
    path: &Path,
) -> Result<Vec<(i64, GeneratorMetadata)>> {
    let mut statement = connection
        .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
        .map_err(|error| {
            cli_error(format!(
                "Failed to query Antigravity database '{}': {error}",
                path.display()
            ))
        })?;
    let mut rows = Vec::new();
    while let sqlite::State::Row = statement.next().map_err(|error| {
        cli_error(format!(
            "Failed to iterate Antigravity database '{}': {error}",
            path.display()
        ))
    })? {
        let idx = statement.read::<i64, _>(0).map_err(|error| {
            cli_error(format!(
                "Failed to read Antigravity row index in '{}': {error}",
                path.display()
            ))
        })?;
        let blob = statement.read::<Vec<u8>, _>(1).map_err(|error| {
            cli_error(format!(
                "Failed to read Antigravity metadata row {idx} in '{}': {error}",
                path.display()
            ))
        })?;
        let metadata = parse_generator_metadata(&blob).map_err(|error| {
            cli_error(format!(
                "Failed to parse Antigravity metadata row {idx} in '{}': {error}",
                path.display()
            ))
        })?;
        rows.push((idx, metadata));
    }
    Ok(rows)
}

fn read_step_rows(
    connection: &sqlite::Connection,
    path: &Path,
) -> Result<Vec<(i64, StepMetadata)>> {
    if !table_exists(connection, "steps", path)? {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT idx, metadata FROM steps \
             WHERE metadata IS NOT NULL ORDER BY idx ASC",
        )
        .map_err(|error| {
            cli_error(format!(
                "Failed to query Antigravity steps in '{}': {error}",
                path.display()
            ))
        })?;
    let mut rows = Vec::new();
    while let sqlite::State::Row = statement.next().map_err(|error| {
        cli_error(format!(
            "Failed to iterate Antigravity steps in '{}': {error}",
            path.display()
        ))
    })? {
        let idx = statement.read::<i64, _>(0).map_err(|error| {
            cli_error(format!(
                "Failed to read Antigravity step index in '{}': {error}",
                path.display()
            ))
        })?;
        let blob = statement.read::<Vec<u8>, _>(1).map_err(|error| {
            cli_error(format!(
                "Failed to read Antigravity step metadata {idx} in '{}': {error}",
                path.display()
            ))
        })?;
        let metadata = parse_step_metadata(&blob).map_err(|error| {
            cli_error(format!(
                "Failed to parse Antigravity step metadata row {idx} in '{}': {error}",
                path.display()
            ))
        })?;
        rows.push((idx, metadata));
    }
    Ok(rows)
}

fn read_trajectory_timestamp(
    connection: &sqlite::Connection,
    path: &Path,
) -> Result<Option<TimestampMs>> {
    if !table_exists(connection, "trajectory_metadata_blob", path)? {
        return Ok(None);
    }
    let mut statement = connection
        .prepare("SELECT data FROM trajectory_metadata_blob ORDER BY rowid ASC")
        .map_err(|error| {
            cli_error(format!(
                "Failed to query Antigravity trajectory metadata in '{}': {error}",
                path.display()
            ))
        })?;
    let mut timestamp = None;
    while let sqlite::State::Row = statement.next().map_err(|error| {
        cli_error(format!(
            "Failed to iterate Antigravity trajectory metadata in '{}': {error}",
            path.display()
        ))
    })? {
        let blob = statement.read::<Vec<u8>, _>(0).map_err(|error| {
            cli_error(format!(
                "Failed to read Antigravity trajectory metadata in '{}': {error}",
                path.display()
            ))
        })?;
        let candidate = parse_trajectory_timestamp(&blob).map_err(|error| {
            cli_error(format!(
                "Failed to parse Antigravity trajectory metadata in '{}': {error}",
                path.display()
            ))
        })?;
        if timestamp.is_none() {
            timestamp = candidate;
        }
    }
    Ok(timestamp)
}

fn table_exists(connection: &sqlite::Connection, table: &str, path: &Path) -> Result<bool> {
    let mut statement = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
        .map_err(|error| {
            cli_error(format!(
                "Failed to inspect Antigravity database '{}': {error}",
                path.display()
            ))
        })?;
    statement.bind((1, table)).map_err(|error| {
        cli_error(format!(
            "Failed to inspect Antigravity database '{}': {error}",
            path.display()
        ))
    })?;
    Ok(matches!(
        statement.next().map_err(|error| {
            cli_error(format!(
                "Failed to inspect Antigravity database '{}': {error}",
                path.display()
            ))
        })?,
        sqlite::State::Row
    ))
}

#[derive(Clone, Copy)]
struct EventContext<'a> {
    model: Option<&'a str>,
    provider: Option<u64>,
    timestamp: Option<(TimestampMs, u8)>,
    trajectory_timestamp: Option<TimestampMs>,
    fallback_timestamp: TimestampMs,
    session_id: &'a str,
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
    let (timestamp, timestamp_rank) = context
        .timestamp
        .or_else(|| {
            usage
                .identity_keys()
                .into_iter()
                .find_map(|identity| identity_timestamps.get(&identity).copied())
        })
        .or_else(|| context.trajectory_timestamp.map(|timestamp| (timestamp, 1)))
        .unwrap_or((context.fallback_timestamp, 0));
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
    let model = usage
        .model_id
        .map(model_name_from_id)
        .and_then(|model| normalize_antigravity_model(&model))
        .or_else(|| context.model.and_then(normalize_antigravity_model))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let provider = usage.provider.or(context.provider);
    let (message_id, message_id_rank) = usage.preferred_message_id();
    let identities = usage.identity_keys();
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
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: context.session_id.to_string(),
        model,
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
    if target.model == DEFAULT_MODEL && duplicate.model != DEFAULT_MODEL {
        target.model = duplicate.model.clone();
    }
    if target.provider.is_none() {
        target.provider = duplicate.provider;
    }
    let use_duplicate_timestamp = duplicate.timestamp_rank > target.timestamp_rank
        || (duplicate.timestamp_rank == target.timestamp_rank
            && duplicate.timestamp < target.timestamp);
    if use_duplicate_timestamp {
        target.timestamp = duplicate.timestamp;
        target.timestamp_text = duplicate.timestamp_text.clone();
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
    let retry_usages = field_bytes_all(&chat_model_fields, 17)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let timestamp = field_bytes(&chat_model_fields, 9)
        .map(parse_generation_info_timestamp)
        .transpose()?
        .flatten();
    let model = [19, 21]
        .into_iter()
        .find_map(|field| field_text(&chat_model_fields, field));
    let model_id = field_varint(&chat_model_fields, 3).filter(|value| *value != 0);
    Ok(GeneratorMetadata {
        model,
        model_id,
        usage,
        retry_usages,
        timestamp,
    })
}

fn parse_step_metadata(blob: &[u8]) -> ProtoResult<StepMetadata> {
    let fields = decode_fields(blob)?;
    let usage = field_bytes(&fields, 9).map(parse_model_usage).transpose()?;
    let retry_usages = field_bytes_all(&fields, 28)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
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
        .saturating_add((nanos / 1_000_000) as i64);
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
        1_000.. => format!("model_placeholder_m{}", model_id - 1_000),
        _ => format!("antigravity-model-{model_id}"),
    }
}

fn normalize_antigravity_model(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let base = lower
        .find('(')
        .map_or(lower.as_str(), |index| lower[..index].trim());
    let normalized = match base {
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
    timezone: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
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
    let cost = calculate_antigravity_cost(
        &event.model,
        event.provider,
        cost_usage,
        event.timestamp,
        mode,
        pricing,
    );
    let missing_pricing_model =
        missing_antigravity_pricing(&event.model, event.provider, cost_usage, mode, pricing);
    let data = UsageEntry {
        session_id: Some(event.session_id.clone()),
        timestamp: event.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(event.model.clone()),
            id: event.message_id,
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
        attribution_plugin: None,
        attribution_skill: None,
    };
    LoadedEntry {
        date: format_date_tz(event.timestamp, timezone),
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
        data,
    }
}

fn calculate_antigravity_cost(
    model: &str,
    provider: Option<u64>,
    usage: TokenUsageRaw,
    timestamp: TimestampMs,
    mode: CostMode,
    pricing: &PricingMap,
) -> f64 {
    match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => model_candidates(model, provider)
            .into_iter()
            .find_map(|candidate| {
                pricing.find(&candidate).map(|_| {
                    calculate_cost_for_usage_at(
                        Some(&candidate),
                        usage,
                        None,
                        Some(timestamp),
                        CostMode::Calculate,
                        Some(pricing),
                    )
                })
            })
            .unwrap_or(0.0),
    }
}

fn missing_antigravity_pricing(
    model: &str,
    provider: Option<u64>,
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    let total_tokens = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_creation_token_count())
        .saturating_add(usage.cache_read_input_tokens);
    missing_pricing_model_for_candidates(
        model,
        model_candidates(model, provider),
        total_tokens,
        Some(pricing),
    )
}

fn model_candidates(model: &str, provider: Option<u64>) -> Vec<String> {
    let mut candidates = vec![model.to_string()];
    if matches!(
        provider,
        Some(
            API_PROVIDER_GOOGLE_VERTEX | API_PROVIDER_GOOGLE_GEMINI | API_PROVIDER_GOOGLE_EVERGREEN
        )
    ) {
        candidates.extend(
            PROVIDER_PREFIXES
                .into_iter()
                .map(|prefix| format!("{prefix}/{model}")),
        );
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}

#[cfg(test)]
pub(super) mod test_support {
    use std::path::Path;

    #[derive(Clone, Copy, Debug, Default)]
    pub(crate) struct UsageFixture {
        pub(crate) model_id: Option<u64>,
        pub(crate) input_tokens: u64,
        pub(crate) total_output_tokens: u64,
        pub(crate) cache_creation_tokens: u64,
        pub(crate) cache_read_tokens: u64,
        pub(crate) reasoning_tokens: u64,
        pub(crate) visible_output_tokens: u64,
        pub(crate) provider: Option<u64>,
        pub(crate) message_id: Option<&'static str>,
        pub(crate) response_id: Option<&'static str>,
        pub(crate) provider_assigned_message_id: Option<&'static str>,
    }

    fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
        while value >= 0x80 {
            output.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }

    fn field_varint(number: u64, value: u64, output: &mut Vec<u8>) {
        encode_varint(number << 3, output);
        encode_varint(value, output);
    }

    fn field_bytes(number: u64, value: &[u8], output: &mut Vec<u8>) {
        encode_varint((number << 3) | 2, output);
        encode_varint(value.len() as u64, output);
        output.extend_from_slice(value);
    }

    fn model_usage_blob(usage: UsageFixture) -> Vec<u8> {
        let mut output = Vec::new();
        if let Some(model_id) = usage.model_id {
            field_varint(1, model_id, &mut output);
        }
        for (field, value) in [
            (2, usage.input_tokens),
            (3, usage.total_output_tokens),
            (4, usage.cache_creation_tokens),
            (5, usage.cache_read_tokens),
            (9, usage.reasoning_tokens),
            (10, usage.visible_output_tokens),
        ] {
            if value > 0 {
                field_varint(field, value, &mut output);
            }
        }
        if let Some(provider) = usage.provider {
            field_varint(6, provider, &mut output);
        }
        if let Some(message_id) = usage.message_id {
            field_bytes(7, message_id.as_bytes(), &mut output);
        }
        if let Some(response_id) = usage.response_id {
            field_bytes(11, response_id.as_bytes(), &mut output);
        }
        if let Some(provider_assigned_message_id) = usage.provider_assigned_message_id {
            field_bytes(12, provider_assigned_message_id.as_bytes(), &mut output);
        }
        output
    }

    fn retry_info_blob(usage: UsageFixture) -> Vec<u8> {
        let usage_blob = model_usage_blob(usage);
        let mut retry_info = Vec::new();
        field_bytes(2, &usage_blob, &mut retry_info);
        retry_info
    }

    fn timestamp_blob(seconds: u64, nanos: u64) -> Vec<u8> {
        let mut timestamp = Vec::new();
        field_varint(1, seconds, &mut timestamp);
        field_varint(2, nanos, &mut timestamp);
        timestamp
    }

    pub(crate) fn metadata_blob(
        model: Option<&str>,
        usage: Option<UsageFixture>,
        timestamp: Option<(u64, u64)>,
        retries: &[UsageFixture],
    ) -> Vec<u8> {
        let mut chat_model = Vec::new();
        if let Some(usage) = usage {
            if let Some(model_id) = usage.model_id {
                field_varint(3, model_id, &mut chat_model);
            }
            let usage_blob = model_usage_blob(usage);
            field_bytes(4, &usage_blob, &mut chat_model);
        }
        if let Some((seconds, nanos)) = timestamp {
            let timestamp_message = timestamp_blob(seconds, nanos);
            let mut generation_info = Vec::new();
            field_bytes(4, &timestamp_message, &mut generation_info);
            field_bytes(9, &generation_info, &mut chat_model);
        }
        for retry in retries {
            let retry_info = retry_info_blob(*retry);
            field_bytes(17, &retry_info, &mut chat_model);
        }
        if let Some(model) = model {
            field_bytes(19, model.as_bytes(), &mut chat_model);
        }

        let mut metadata = Vec::new();
        field_bytes(1, &chat_model, &mut metadata);
        metadata
    }

    pub(crate) fn step_metadata_blob(
        model: Option<&str>,
        usage: Option<UsageFixture>,
        timestamp: Option<(u64, u64)>,
        retries: &[UsageFixture],
        provider: Option<u64>,
    ) -> Vec<u8> {
        let mut metadata = Vec::new();
        if let Some((seconds, nanos)) = timestamp {
            let timestamp_message = timestamp_blob(seconds, nanos);
            field_bytes(8, &timestamp_message, &mut metadata);
        }
        if let Some(usage) = usage {
            let usage_blob = model_usage_blob(usage);
            field_bytes(9, &usage_blob, &mut metadata);
        }
        if model.is_some() || provider.is_some() {
            let mut model_info = Vec::new();
            if let Some(usage) = usage
                && let Some(model_id) = usage.model_id
            {
                field_varint(1, model_id, &mut model_info);
            }
            if let Some(provider) = provider {
                field_varint(7, provider, &mut model_info);
            }
            if let Some(model) = model {
                field_bytes(12, model.as_bytes(), &mut model_info);
            }
            field_bytes(24, &model_info, &mut metadata);
        }
        for retry in retries {
            let retry_info = retry_info_blob(*retry);
            field_bytes(28, &retry_info, &mut metadata);
        }
        metadata
    }

    pub(crate) fn trajectory_metadata_blob(seconds: u64, nanos: u64) -> Vec<u8> {
        let timestamp = timestamp_blob(seconds, nanos);
        let mut metadata = Vec::new();
        field_bytes(2, &timestamp, &mut metadata);
        metadata
    }

    pub(crate) fn create_database(
        path: &Path,
        generation_rows: &[(i64, Vec<u8>)],
        step_rows: &[(i64, Vec<u8>)],
        trajectory_rows: &[Vec<u8>],
    ) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let connection = sqlite::open(path).unwrap();
        connection
            .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);")
            .unwrap();
        connection
            .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
            .unwrap();
        connection
            .execute("CREATE TABLE trajectory_metadata_blob (data BLOB);")
            .unwrap();
        let mut generation_statement = connection
            .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
            .unwrap();
        for (idx, data) in generation_rows {
            generation_statement.bind((1, *idx)).unwrap();
            generation_statement.bind((2, data.as_slice())).unwrap();
            generation_statement.next().unwrap();
            generation_statement.reset().unwrap();
        }
        let mut step_statement = connection
            .prepare("INSERT INTO steps (idx, metadata) VALUES (?1, ?2)")
            .unwrap();
        for (idx, data) in step_rows {
            step_statement.bind((1, *idx)).unwrap();
            step_statement.bind((2, data.as_slice())).unwrap();
            step_statement.next().unwrap();
            step_statement.reset().unwrap();
        }
        let mut trajectory_statement = connection
            .prepare("INSERT INTO trajectory_metadata_blob (data) VALUES (?1)")
            .unwrap();
        for data in trajectory_rows {
            trajectory_statement.bind((1, data.as_slice())).unwrap();
            trajectory_statement.next().unwrap();
            trajectory_statement.reset().unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{UsageFixture, step_metadata_blob};
    use super::{
        API_PROVIDER_GOOGLE_GEMINI, ProtoField, ProtoValue, field_bytes, field_bytes_all,
        field_text, field_varint, missing_antigravity_pricing, model_candidates,
        parse_step_metadata,
    };
    use crate::{PricingMap, TokenUsageRaw, cli::CostMode};

    #[test]
    fn protobuf_scalars_use_last_duplicate_and_messages_keep_merge_order() {
        let first_text = b"first";
        let last_text = b"last";
        let first_message = b"message-first";
        let second_message = b"message-second";
        let fields = [
            ProtoField {
                number: 1,
                value: ProtoValue::Varint(1),
            },
            ProtoField {
                number: 1,
                value: ProtoValue::Varint(2),
            },
            ProtoField {
                number: 2,
                value: ProtoValue::Bytes(first_text),
            },
            ProtoField {
                number: 2,
                value: ProtoValue::Bytes(last_text),
            },
            ProtoField {
                number: 3,
                value: ProtoValue::Bytes(first_message),
            },
            ProtoField {
                number: 3,
                value: ProtoValue::Bytes(second_message),
            },
        ];

        assert_eq!(field_varint(&fields, 1), Some(2));
        assert_eq!(field_text(&fields, 2).as_deref(), Some("last"));
        assert_eq!(field_bytes(&fields, 3), Some(first_message.as_slice()));
        assert_eq!(
            field_bytes_all(&fields, 3),
            vec![first_message.as_slice(), second_message.as_slice()]
        );
    }

    #[test]
    fn parses_production_step_retry_info_from_field_28() {
        let retry = UsageFixture {
            input_tokens: 11,
            total_output_tokens: 22,
            visible_output_tokens: 20,
            response_id: Some("production-step-retry"),
            ..UsageFixture::default()
        };
        let metadata = parse_step_metadata(&step_metadata_blob(
            None,
            None,
            None,
            &[retry],
            Some(API_PROVIDER_GOOGLE_GEMINI),
        ))
        .unwrap();

        assert_eq!(metadata.retry_usages.len(), 1);
        assert_eq!(metadata.retry_usages[0].input_tokens, 11);
        assert_eq!(
            metadata.retry_usages[0].response_id.as_deref(),
            Some("production-step-retry")
        );
    }

    #[test]
    fn only_google_providers_receive_google_model_candidates() {
        let bare = "gemini-unpriced".to_string();
        assert_eq!(
            model_candidates("gemini-unpriced", Some(26)),
            vec![bare.clone()]
        );
        assert_eq!(
            model_candidates("gemini-unpriced", None),
            vec![bare.clone()]
        );
        assert_eq!(
            model_candidates("gemini-unpriced", Some(API_PROVIDER_GOOGLE_GEMINI)),
            vec![
                bare,
                "google/gemini-unpriced".to_string(),
                "gemini/gemini-unpriced".to_string(),
                "vertex_ai/gemini-unpriced".to_string(),
                "openrouter/google/gemini-unpriced".to_string(),
            ]
        );
    }

    #[test]
    fn missing_pricing_token_total_saturates() {
        let pricing = PricingMap::default();
        let result = std::panic::catch_unwind(|| {
            missing_antigravity_pricing(
                "antigravity-unpriced",
                None,
                TokenUsageRaw {
                    input_tokens: u64::MAX,
                    output_tokens: u64::MAX,
                    cache_creation_input_tokens: u64::MAX,
                    cache_read_input_tokens: u64::MAX,
                    speed: None,
                    cache_creation: None,
                },
                CostMode::Calculate,
                &pricing,
            )
        });

        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }
}
