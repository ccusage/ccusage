use crate::{
    CodexRawUsage,
    types::{CodexInfo, CodexLogEntry, CodexUsageCandidate},
};

use super::CodexTokenCountUsage;

fn usage_from_result(value: &CodexLogEntry<'_>) -> CodexUsageCandidate {
    first_decoded_usage([
        value.usage,
        value
            .data
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |data| data.usage),
        value
            .result
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |result| result.usage),
        value
            .response
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |response| response.usage),
    ])
}

fn first_decoded_usage(
    candidates: impl IntoIterator<Item = CodexUsageCandidate>,
) -> CodexUsageCandidate {
    let mut malformed = false;
    for candidate in candidates {
        match candidate {
            decoded @ CodexUsageCandidate::Decoded(_) => return decoded,
            CodexUsageCandidate::Malformed => malformed = true,
            CodexUsageCandidate::Missing => {}
        }
    }
    if malformed {
        CodexUsageCandidate::Malformed
    } else {
        CodexUsageCandidate::Missing
    }
}

pub(super) fn normalize_headless_codex_usage(
    value: &CodexLogEntry<'_>,
) -> std::result::Result<Option<CodexRawUsage>, ()> {
    normalize_headless_usage_candidate(usage_from_result(value))
}

fn normalize_headless_usage_candidate(
    candidate: CodexUsageCandidate,
) -> std::result::Result<Option<CodexRawUsage>, ()> {
    let usage = match candidate {
        CodexUsageCandidate::Decoded(usage) => normalize_codex_raw_usage(usage),
        CodexUsageCandidate::Malformed => return Err(()),
        CodexUsageCandidate::Missing => return Ok(None),
    };
    if usage.is_empty() {
        return Ok(None);
    }
    Ok(Some(usage))
}

pub(super) fn malformed_token_count_field(info: &CodexInfo<'_>) -> Option<&'static str> {
    if info.last_token_usage == CodexUsageCandidate::Malformed {
        Some("field `payload.info.last_token_usage` did not decode")
    } else if info.total_token_usage == CodexUsageCandidate::Malformed {
        Some("field `payload.info.total_token_usage` did not decode")
    } else {
        None
    }
}

fn subtract_codex_raw_usage(
    current: &CodexRawUsage,
    previous: Option<&CodexRawUsage>,
) -> CodexRawUsage {
    normalize_codex_raw_usage(CodexRawUsage {
        input_tokens: current
            .input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.input_tokens)),
        cached_input_tokens: current
            .cached_input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cached_input_tokens)),
        cache_creation_tokens: current
            .cache_creation_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cache_creation_tokens)),
        output_tokens: current
            .output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.output_tokens)),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.reasoning_output_tokens)),
        total_tokens: current
            .total_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.total_tokens)),
    })
}

pub(super) fn codex_token_count_usage(
    info: Option<&CodexInfo<'_>>,
    previous_totals: &mut Option<CodexRawUsage>,
) -> CodexTokenCountUsage {
    let total_usage = info.and_then(|info| info.total_token_usage.decoded());
    let cumulative_advanced = total_usage
        .as_ref()
        .is_none_or(|total_usage| previous_totals.as_ref() != Some(total_usage));
    let raw_usage = info
        .and_then(|info| info.last_token_usage.decoded())
        .filter(|_| cumulative_advanced)
        .or_else(|| {
            total_usage
                .as_ref()
                .map(|usage| subtract_codex_raw_usage(usage, previous_totals.as_ref()))
        });
    if let Some(total_usage) = total_usage {
        *previous_totals = Some(total_usage);
    }
    CodexTokenCountUsage {
        total_usage,
        cumulative_advanced,
        delta: raw_usage.map(normalize_codex_raw_usage),
    }
}

pub(super) fn normalize_codex_raw_usage(mut usage: CodexRawUsage) -> CodexRawUsage {
    usage.cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    usage.cache_creation_tokens = usage
        .cache_creation_tokens
        .min(usage.input_tokens.saturating_sub(usage.cached_input_tokens));
    usage
}

#[cfg(test)]
mod tests;
