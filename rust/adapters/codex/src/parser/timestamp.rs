use std::path::Path;

use crate::{
    TimestampMs,
    types::{CodexLogEntry, CodexResultFields, CodexTimestamp},
};

use super::codex_timestamp_instant;

pub(super) fn codex_session_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    match value? {
        CodexTimestamp::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        CodexTimestamp::Number(_) => normalize_codex_timestamp(value),
        CodexTimestamp::Invalid => None,
    }
}

pub(super) fn codex_usage_timestamp(
    value: Option<&CodexTimestamp<'_>>,
) -> Option<(TimestampMs, String)> {
    let instant = codex_timestamp_instant(value)?;
    let raw = match value? {
        CodexTimestamp::String(text) => text.trim().to_string(),
        CodexTimestamp::Number(_) => crate::format_rfc3339_millis(instant),
        CodexTimestamp::Invalid => return None,
    };
    Some((instant, raw))
}

pub(super) fn codex_timestamp_from_result(value: &CodexLogEntry<'_>) -> Option<TimestampMs> {
    codex_timestamp_instant(value.timestamp.as_ref())
        .or_else(|| codex_timestamp_instant(value.created_at.as_ref()))
        .or_else(|| codex_timestamp_instant(value.created_at_camel.as_ref()))
        .or_else(|| {
            value
                .data
                .as_ref()
                .and_then(|data| normalize_result_fields_timestamp(data))
        })
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(|result| normalize_result_fields_timestamp(result))
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(|response| normalize_result_fields_timestamp(response))
        })
}

fn normalize_result_fields_timestamp(value: &CodexResultFields<'_>) -> Option<TimestampMs> {
    codex_timestamp_instant(value.timestamp.as_ref())
        .or_else(|| codex_timestamp_instant(value.created_at.as_ref()))
        .or_else(|| codex_timestamp_instant(value.created_at_camel.as_ref()))
}

fn normalize_codex_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    codex_timestamp_instant(value).map(crate::format_rfc3339_millis)
}

pub(super) fn file_modified_timestamp(path: &Path) -> TimestampMs {
    ccusage_adapter_common::timestamps::file_mtime(path).unwrap_or(TimestampMs::UNIX_EPOCH)
}
