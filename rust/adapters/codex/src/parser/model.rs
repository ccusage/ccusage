use std::borrow::Cow;

use crate::{
    TimestampMs,
    types::{CodexInfo, CodexLogEntry, CodexModelMetadata, CodexPayload, CodexResultFields},
};

const CODEX_AUTO_REVIEW_MODEL: &str = "codex-auto-review";
const CODEX_AUTO_REVIEW_DEFAULT_MODEL: &str = "gpt-5";
// NOTE(2026-09-17, #1753): The server can reroute `codex-auto-review` without a public model release. The rollout omits the resolved model, so a new timeline entry needs routing evidence.
const CODEX_AUTO_REVIEW_FALLBACKS: &[CodexAutoReviewFallback] = &[
    CodexAutoReviewFallback::new((2026, 7, 30), "gpt-5.6-luna"),
    CodexAutoReviewFallback::new((2026, 3, 5), "gpt-5.4"),
    CodexAutoReviewFallback::new((2026, 2, 5), "gpt-5.3-codex"),
    CodexAutoReviewFallback::new((2025, 12, 11), "gpt-5.2-codex"),
    CodexAutoReviewFallback::new((2025, 11, 13), "gpt-5.1-codex"),
    CodexAutoReviewFallback::new((2025, 9, 15), "gpt-5-codex"),
    CodexAutoReviewFallback::new((2025, 8, 7), CODEX_AUTO_REVIEW_DEFAULT_MODEL),
];

struct CodexAutoReviewFallback {
    released_on: (i32, u32, u32),
    model: &'static str,
}

impl CodexAutoReviewFallback {
    const fn new(released_on: (i32, u32, u32), model: &'static str) -> Self {
        Self { released_on, model }
    }
}

pub(super) fn resolve_codex_usage_model(
    parsed_model: Option<String>,
    timestamp: TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
) -> (String, bool) {
    if let Some(model) = parsed_model {
        *current_model = Some(model);
        *current_model_is_fallback = false;
    }
    let model = current_model.get_or_insert_with(|| {
        *current_model_is_fallback = true;
        "gpt-5".to_string()
    });
    match codex_log_model_fallback(model, timestamp) {
        Some(fallback) => (fallback.to_string(), true),
        None => (model.clone(), *current_model_is_fallback),
    }
}

fn codex_log_model_fallback(model: &str, timestamp: TimestampMs) -> Option<&'static str> {
    if model != CODEX_AUTO_REVIEW_MODEL {
        return None;
    }
    let parts = timestamp.utc_parts();
    let date = (parts.year, parts.month, parts.day);
    let routed = CODEX_AUTO_REVIEW_FALLBACKS
        .iter()
        .filter(|fallback| fallback.released_on <= date)
        .max_by_key(|fallback| fallback.released_on);
    Some(routed.map_or(CODEX_AUTO_REVIEW_DEFAULT_MODEL, |fallback| fallback.model))
}

pub(super) fn codex_model_from_payload(value: &CodexPayload<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

pub(super) fn codex_model_from_info(value: &CodexInfo<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

pub(super) fn codex_model_from_result(value: &CodexLogEntry<'_>) -> Option<String> {
    codex_model_from_entry(value)
        .or_else(|| value.data.as_ref().and_then(codex_model_from_result_fields))
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(codex_model_from_result_fields)
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(codex_model_from_result_fields)
        })
}

fn codex_model_from_result_fields(value: &CodexResultFields<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_entry(value: &CodexLogEntry<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_parts(
    model: Option<&Cow<'_, str>>,
    model_name: Option<&Cow<'_, str>>,
    metadata: Option<&CodexModelMetadata<'_>>,
) -> Option<String> {
    non_empty_cow_string(model)
        .or_else(|| non_empty_cow_string(model_name))
        .or_else(|| metadata.and_then(|metadata| non_empty_cow_string(metadata.model.as_ref())))
}

fn non_empty_cow_string(value: Option<&Cow<'_, str>>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

#[cfg(test)]
mod tests;
