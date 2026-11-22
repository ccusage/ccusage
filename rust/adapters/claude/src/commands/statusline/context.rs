use std::{borrow::Cow, ops::ControlFlow, path::Path};

use ccusage_adapter_common::{json::JsonText, jsonl};
use ccusage_core::{
    Color,
    cli::{SharedArgs, StatuslineArgs},
    color, format_number,
    pricing::PricingMap,
};
use serde::Deserialize;

use super::HookContext;

pub(super) fn format_statusline_context(
    input_tokens: u64,
    context_limit: u64,
    args: &StatuslineArgs,
    shared: &SharedArgs,
) -> String {
    let percentage = if context_limit == 0 {
        0
    } else {
        ((input_tokens as f64 / context_limit as f64) * 100.0).round() as u64
    };
    let context_color = statusline_context_color(percentage, args);
    format!(
        "{} ({})",
        format_number(input_tokens),
        color(shared, format!("{percentage}%"), context_color)
    )
}

pub(super) fn statusline_context_color(percentage: u64, args: &StatuslineArgs) -> Color {
    if percentage < u64::from(args.context_thresholds.low()) {
        Color::Green
    } else if percentage < u64::from(args.context_thresholds.medium()) {
        Color::Yellow
    } else {
        Color::Red
    }
}

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;

pub(super) fn calculate_context_tokens_from_transcript(
    path: &Path,
    model_id: Option<&str>,
    offline: bool,
    shared: &SharedArgs,
) -> Option<HookContext> {
    let marker = ccusage_core::fast::LinePrefilter::all(&[br#""input_tokens""#]);
    let usage = jsonl::read_last(path, |line| {
        let bytes = line.raw();
        if marker.matches(bytes)
            && let Some(usage) = context_usage(bytes)
        {
            return ControlFlow::Break(usage);
        }
        ControlFlow::Continue(())
    })
    .ok()??;
    let total_input_tokens = usage
        .input_tokens?
        .checked_add(usage.cache_creation_input_tokens)?
        .checked_add(usage.cache_read_input_tokens)?;
    let context_window_size = model_id
        .filter(|model_id| !model_id.is_empty())
        .and_then(|model_id| {
            PricingMap::load_with_overrides(offline, false, shared.pricing_overrides.iter())
                .context_limit(model_id)
        })
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
    Some(HookContext {
        total_input_tokens,
        context_window_size,
    })
}

#[derive(Deserialize)]
struct ContextRecord<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    message: ContextMessage,
}

#[derive(Deserialize)]
struct ContextMessage {
    usage: TranscriptContextUsage,
}

#[derive(Deserialize)]
struct TranscriptContextUsage {
    input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_input_tokens: u64,
}

fn context_usage(bytes: &[u8]) -> Option<TranscriptContextUsage> {
    let bytes = bytes.trim_ascii();
    let start = bytes.iter().position(|byte| matches!(byte, b'{' | b'['))?;
    let end = bytes.iter().rposition(|byte| matches!(byte, b'}' | b']'))? + 1;
    if end <= start
        || !std::str::from_utf8(&bytes[..start]).ok()?.trim().is_empty()
        || !std::str::from_utf8(&bytes[end..]).ok()?.trim().is_empty()
    {
        return None;
    }
    let text = JsonText::new(&bytes[start..end]);
    let record = text.parse::<ContextRecord<'_>>().ok()?;
    let usage = record.message.usage;
    (record.kind == "assistant" && usage.input_tokens.is_some()).then_some(usage)
}
