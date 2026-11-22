use crate::{CodexRawUsage, CodexTokenUsageEvent, TimestampMs, types::CodexLogEntry};

use super::{
    RecordLocation,
    model::{codex_model_from_result, resolve_codex_usage_model},
    timestamp::codex_timestamp_from_result,
    usage::{normalize_codex_raw_usage, normalize_headless_codex_usage},
};

pub(super) fn add_codex_exec_event(
    session_id: &str,
    value: &CodexLogEntry<'_>,
    fallback_timestamp: impl FnOnce() -> TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent),
    location: &RecordLocation<'_>,
) {
    let raw_usage = match normalize_headless_codex_usage(value) {
        Ok(Some(raw_usage)) => raw_usage,
        Ok(None) => return,
        Err(()) => {
            location.invalid_usage("headless usage did not decode");
            return;
        }
    };
    let parsed_model = codex_model_from_result(value);
    visit_codex_exec_usage_event(
        session_id,
        raw_usage,
        parsed_model,
        codex_timestamp_from_result(value).unwrap_or_else(fallback_timestamp),
        current_model,
        current_model_is_fallback,
        visit,
    )
}

fn visit_codex_exec_usage_event(
    session_id: &str,
    raw_usage: CodexRawUsage,
    parsed_model: Option<String>,
    timestamp: TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent),
) {
    let raw_usage = normalize_codex_raw_usage(raw_usage);
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        timestamp,
        current_model,
        current_model_is_fallback,
    );
    visit(CodexTokenUsageEvent {
        session_id: session_id.to_string(),
        response_id: None,
        record_response_id: None,
        timestamp,
        last_activity: crate::format_rfc3339_millis(timestamp),
        model,
        input_tokens: raw_usage.input_tokens,
        cached_input_tokens: raw_usage.cached_input_tokens,
        cache_creation_tokens: raw_usage.cache_creation_tokens,
        output_tokens: raw_usage.output_tokens,
        reasoning_output_tokens: raw_usage.reasoning_output_tokens,
        total_tokens: raw_usage.total_tokens,
        is_fallback_model,
        service_tier: None,
    })
}
