use crate::TimestampMs;

use super::{GeneratorMetadata, ModelUsage, StepMetadata};

#[derive(Clone, Copy)]
pub(super) enum ProtoValue<'a> {
    Varint(u64),
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32,
}

#[derive(Clone, Copy)]
pub(super) struct ProtoField<'a> {
    pub(super) number: u32,
    pub(super) value: ProtoValue<'a>,
}

type ProtoResult<T> = std::result::Result<T, &'static str>;

pub(super) fn parse_generator_metadata(blob: &[u8]) -> ProtoResult<GeneratorMetadata> {
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

pub(super) fn parse_step_metadata(blob: &[u8]) -> ProtoResult<StepMetadata> {
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

pub(super) fn parse_trajectory_timestamp(blob: &[u8]) -> ProtoResult<Option<TimestampMs>> {
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

pub(super) fn field_varint(fields: &[ProtoField<'_>], number: u32) -> Option<u64> {
    fields.iter().rev().find_map(|field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Varint(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

pub(super) fn field_bytes<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Bytes(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

pub(super) fn field_bytes_all<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Vec<&'a [u8]> {
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

pub(super) fn field_text(fields: &[ProtoField<'_>], number: u32) -> Option<String> {
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
