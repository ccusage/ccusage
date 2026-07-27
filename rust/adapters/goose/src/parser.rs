use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, TokenUsageRaw, UsageEntry, UsageMessage, calculate_cost_for_usage,
    cli::CostMode, format_date_tz, missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

/// Goose stores the per-session model selection as a JSON blob in the
/// `model_config_json` column. Only the human-readable model name is consumed.
#[derive(Debug, Deserialize)]
struct GooseModelConfig {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model_name: Option<String>,
}

pub(super) fn row_to_entry(
    statement: &sqlite::Statement<'_>,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    let id = statement.read::<String, _>(0).ok()?;
    let model_config = statement.read::<String, _>(1).ok()?;
    let provider_name = statement.read::<String, _>(2).ok();
    let created_at = read_timestamp_value(statement, 3)?;
    let timestamp = parse_goose_timestamp(&created_at)?;
    let model = parse_goose_model_config(&model_config)?;

    let input_tokens = read_token_value(statement, 8)
        .or_else(|| read_token_value(statement, 5))
        .unwrap_or(0);
    let output_tokens = read_token_value(statement, 9)
        .or_else(|| read_token_value(statement, 6))
        .unwrap_or(0);
    let total_tokens = read_token_value(statement, 7)
        .or_else(|| read_token_value(statement, 4))
        .unwrap_or(input_tokens.saturating_add(output_tokens));
    if input_tokens == 0 && output_tokens == 0 && total_tokens == 0 {
        return None;
    }

    let reasoning_tokens = total_tokens.saturating_sub(input_tokens.saturating_add(output_tokens));
    let provider_id = normalize_provider(provider_name.as_deref(), &model);
    let usage = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };
    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let data = UsageEntry {
        session_id: Some(id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: Some(id.clone()),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost = calculate_goose_cost(&model, &provider_id, usage, reasoning_tokens, pricing);
    let missing_pricing_model =
        missing_goose_pricing(&model, &provider_id, usage, reasoning_tokens, pricing);

    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("goose"),
        session_id: Arc::from(id.as_str()),
        project_path: Arc::from("Goose"),
        cost,
        credits: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        extra_total_tokens: reasoning_tokens,
        message_count: None,
        data,
    })
}

fn read_token_value(statement: &sqlite::Statement<'_>, index: usize) -> Option<u64> {
    statement
        .read::<i64, _>(index)
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value as u64)
}

fn read_timestamp_value(statement: &sqlite::Statement<'_>, index: usize) -> Option<String> {
    statement.read::<String, _>(index).ok().or_else(|| {
        statement
            .read::<i64, _>(index)
            .ok()
            .map(|value| value.to_string())
    })
}

fn parse_goose_model_config(value: &str) -> Option<String> {
    let config = serde_json::from_str::<GooseModelConfig>(value).ok()?;
    config.model_name
}

fn parse_goose_timestamp(value: &str) -> Option<crate::TimestampMs> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(number) = trimmed.parse::<i64>() {
        let millis = if number > 1_000_000_000_000 {
            number
        } else {
            number.checked_mul(1_000)?
        };
        return (millis > 0).then(|| crate::TimestampMs::from_millis(millis));
    }
    if let Some(timestamp) = crate::parse_ts_timestamp(trimmed) {
        return Some(timestamp);
    }
    if trimmed.len() == 19
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(7) == Some(&b'-')
        && (trimmed.as_bytes().get(10) == Some(&b' ') || trimmed.as_bytes().get(10) == Some(&b'T'))
    {
        let normalized = format!("{}T{}Z", &trimmed[..10], &trimmed[11..]);
        return crate::parse_ts_timestamp(&normalized);
    }
    if trimmed.len() == 10
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(7) == Some(&b'-')
    {
        return crate::parse_ts_timestamp(&format!("{trimmed}T00:00:00Z"));
    }
    None
}

fn normalize_provider(provider: Option<&str>, model: &str) -> String {
    let provider = provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    if let Some(provider) = provider {
        return provider.replace('-', "_");
    }
    if model.starts_with("claude-") {
        return "anthropic".to_string();
    }
    if model.starts_with("gpt-") || model.starts_with("chatgpt-") || model.starts_with('o') {
        return "openai".to_string();
    }
    if model.starts_with("gemini-") {
        return "google".to_string();
    }
    if model.to_ascii_lowercase().starts_with("qwen") {
        return "openrouter".to_string();
    }
    "goose".to_string()
}

fn calculate_goose_cost(
    model: &str,
    provider_id: &str,
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    pricing: &PricingMap,
) -> f64 {
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(reasoning_tokens),
        cache_creation: None,
        ..usage
    };
    let raw = calculate_cost_for_usage(
        Some(model),
        cost_usage,
        None,
        CostMode::Calculate,
        Some(pricing),
    );
    if raw > 0.0 || provider_id == "goose" {
        return raw;
    }
    let candidate = format!("{provider_id}/{model}");
    calculate_cost_for_usage(
        Some(&candidate),
        cost_usage,
        None,
        CostMode::Calculate,
        Some(pricing),
    )
}

fn missing_goose_pricing(
    model: &str,
    provider_id: &str,
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    pricing: &PricingMap,
) -> Option<String> {
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(reasoning_tokens),
        cache_creation: None,
        ..usage
    };
    let mut candidates = vec![model.to_string()];
    if provider_id != "goose" {
        candidates.push(format!("{provider_id}/{model}"));
    }
    missing_pricing_model_for_candidates(
        model,
        candidates,
        crate::total_usage_tokens(cost_usage),
        Some(pricing),
    )
}
