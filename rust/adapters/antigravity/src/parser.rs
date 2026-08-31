use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, cli_error, format_date_tz,
    missing_pricing_model_for_candidates,
};

const DEFAULT_MODEL: &str = "gemini-internal-model";
const PROVIDER_PREFIXES: [&str; 4] = ["google", "gemini", "vertex_ai", "openrouter/google"];

#[derive(Debug, Clone)]
pub(super) struct AntigravityUsageEvent {
    pub(super) timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
    pub(super) message_id: Option<String>,
}

#[derive(Debug, Default)]
struct GeneratorMetadata {
    model: Option<String>,
    usage: ModelUsage,
    timestamp: Option<TimestampMs>,
}

#[derive(Debug, Default)]
struct ModelUsage {
    system_input_tokens: u64,
    input_tokens: u64,
    total_output_tokens: u64,
    cache_read_tokens: u64,
    output_tokens: u64,
    thinking_tokens: u64,
    response_id: Option<String>,
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
    let mut statement = connection
        .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
        .map_err(|error| {
            cli_error(format!(
                "Failed to query Antigravity database '{}': {error}",
                path.display()
            ))
        })?;
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut current_model = None;
    let mut events = Vec::new();

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
        if blob.is_empty() {
            continue;
        }
        let metadata = parse_generator_metadata(&blob).map_err(|error| {
            cli_error(format!(
                "Failed to parse Antigravity metadata row {idx} in '{}': {error}",
                path.display()
            ))
        })?;
        let row_model = metadata
            .model
            .as_deref()
            .and_then(normalize_antigravity_model);
        if let Some(model) = row_model {
            current_model = Some(model);
        }
        let model = current_model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let input_tokens = metadata
            .usage
            .system_input_tokens
            .saturating_add(metadata.usage.input_tokens);
        let thinking_tokens = metadata.usage.thinking_tokens;
        let output_tokens = if metadata.usage.output_tokens > 0 {
            metadata.usage.output_tokens
        } else {
            metadata
                .usage
                .total_output_tokens
                .saturating_sub(thinking_tokens)
        };
        let effective_thinking_tokens = thinking_tokens.max(
            metadata
                .usage
                .total_output_tokens
                .saturating_sub(output_tokens),
        );
        let total_output_tokens = metadata
            .usage
            .total_output_tokens
            .max(output_tokens.saturating_add(effective_thinking_tokens));
        if input_tokens == 0 && metadata.usage.cache_read_tokens == 0 && total_output_tokens == 0 {
            continue;
        }
        let timestamp = metadata.timestamp.unwrap_or(fallback_timestamp);
        let total_tokens = input_tokens
            .saturating_add(metadata.usage.cache_read_tokens)
            .saturating_add(total_output_tokens);
        events.push(AntigravityUsageEvent {
            timestamp,
            timestamp_text: crate::format_rfc3339_millis(timestamp),
            session_id: session_id.clone(),
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens: metadata.usage.cache_read_tokens,
            reasoning_tokens: effective_thinking_tokens,
            total_tokens,
            message_id: metadata.usage.response_id,
        });
    }
    Ok(events)
}

fn parse_generator_metadata(blob: &[u8]) -> ProtoResult<GeneratorMetadata> {
    let root = decode_fields(blob)?;
    let chat_model = field_bytes(&root, 1).ok_or("missing chat model field 1")?;
    let chat_model_fields = decode_fields(chat_model)?;
    let usage = field_bytes(&chat_model_fields, 4)
        .map(parse_model_usage)
        .transpose()?
        .unwrap_or_default();
    let timestamp = field_bytes(&chat_model_fields, 9)
        .map(parse_generation_info_timestamp)
        .transpose()?
        .flatten();
    let model = [19, 21, 3]
        .into_iter()
        .find_map(|field| field_text(&chat_model_fields, field));
    Ok(GeneratorMetadata {
        model,
        usage,
        timestamp,
    })
}

fn parse_model_usage(blob: &[u8]) -> ProtoResult<ModelUsage> {
    let fields = decode_fields(blob)?;
    Ok(ModelUsage {
        system_input_tokens: field_varint(&fields, 1).unwrap_or(0),
        input_tokens: field_varint(&fields, 2).unwrap_or(0),
        total_output_tokens: field_varint(&fields, 3).unwrap_or(0),
        cache_read_tokens: field_varint(&fields, 5).unwrap_or(0),
        output_tokens: field_varint(&fields, 9).unwrap_or(0),
        thinking_tokens: field_varint(&fields, 10).unwrap_or(0),
        response_id: field_text(&fields, 11).filter(|value| !value.is_empty()),
    })
}

fn parse_generation_info_timestamp(blob: &[u8]) -> ProtoResult<Option<TimestampMs>> {
    let generation_info = decode_fields(blob)?;
    let Some(timestamp_message) = field_bytes(&generation_info, 4) else {
        return Ok(None);
    };
    let timestamp_fields = decode_fields(timestamp_message)?;
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
    fields.iter().find_map(|field| match field {
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

fn field_text(fields: &[ProtoField<'_>], number: u32) -> Option<String> {
    field_bytes(fields, number)
        .and_then(|value| std::str::from_utf8(value).ok())
        .map(ToString::to_string)
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
        "gemini-3-flash-agent-a" | "gemini-3-flash-agent-b" => "gemini-3.5-flash-high",
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
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: event.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: event.output_tokens.saturating_add(event.reasoning_tokens),
        cache_creation: None,
        ..usage
    };
    let extra_total_tokens = event.total_tokens.saturating_sub(
        event
            .input_tokens
            .saturating_add(event.output_tokens)
            .saturating_add(event.cache_read_tokens),
    );
    let cost = calculate_antigravity_cost(&event.model, cost_usage, mode, pricing);
    let missing_pricing_model =
        missing_antigravity_pricing(&event.model, cost_usage, mode, pricing);
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
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: &PricingMap,
) -> f64 {
    match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => model_candidates(model)
            .into_iter()
            .find_map(|candidate| {
                pricing.find(&candidate).map(|_| {
                    calculate_cost_for_usage(
                        Some(&candidate),
                        usage,
                        None,
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
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    missing_pricing_model_for_candidates(
        model,
        model_candidates(model),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

fn model_candidates(model: &str) -> Vec<String> {
    let mut candidates = PROVIDER_PREFIXES
        .into_iter()
        .map(|prefix| format!("{prefix}/{model}"))
        .collect::<Vec<_>>();
    candidates.push(model.to_string());
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

    pub(crate) fn metadata_blob(
        model: Option<&str>,
        token_buckets: (u64, u64, u64, u64, u64),
        timestamp: (u64, u64),
        response_id: &str,
    ) -> Vec<u8> {
        let (system_input_tokens, input_tokens, cache_read_tokens, output_tokens, thinking_tokens) =
            token_buckets;
        let (seconds, nanos) = timestamp;
        let mut usage = Vec::new();
        field_varint(1, system_input_tokens, &mut usage);
        field_varint(2, input_tokens, &mut usage);
        field_varint(3, output_tokens.saturating_add(thinking_tokens), &mut usage);
        field_varint(5, cache_read_tokens, &mut usage);
        field_varint(9, output_tokens, &mut usage);
        field_varint(10, thinking_tokens, &mut usage);
        field_bytes(11, response_id.as_bytes(), &mut usage);

        let mut timestamp_message = Vec::new();
        field_varint(1, seconds, &mut timestamp_message);
        field_varint(2, nanos, &mut timestamp_message);
        let mut generation_info = Vec::new();
        field_bytes(4, &timestamp_message, &mut generation_info);

        let mut chat_model = Vec::new();
        field_bytes(4, &usage, &mut chat_model);
        field_bytes(9, &generation_info, &mut chat_model);
        if let Some(model) = model {
            field_bytes(19, model.as_bytes(), &mut chat_model);
        }

        let mut metadata = Vec::new();
        field_bytes(1, &chat_model, &mut metadata);
        metadata
    }

    pub(crate) fn create_database(path: &Path, rows: &[(i64, Vec<u8>)]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let connection = sqlite::open(path).unwrap();
        connection
            .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);")
            .unwrap();
        connection
            .execute("CREATE TABLE trajectory_metadata_blob (data BLOB);")
            .unwrap();
        let mut statement = connection
            .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
            .unwrap();
        for (idx, data) in rows {
            statement.bind((1, *idx)).unwrap();
            statement.bind((2, data.as_slice())).unwrap();
            statement.next().unwrap();
            statement.reset().unwrap();
        }
    }
}
