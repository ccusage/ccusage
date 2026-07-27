use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, cli::CostMode, format_date_tz,
    missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

/// A single parsed Kilo message row payload. Only the fields ccusage consumes
/// are declared; serde skips everything else.
#[derive(Debug, Default, Deserialize)]
pub(super) struct KiloMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    tokens: Option<KiloTokens>,
    #[serde(
        rename = "modelID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    time: Option<KiloTime>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    cost: Option<f64>,
    #[serde(
        rename = "providerID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    provider_id: Option<String>,
}

/// Token usage block carried by Kilo assistant messages.
#[derive(Debug, Default, Deserialize)]
struct KiloTokens {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cache: Option<KiloCache>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total: u64,
}

/// Cache read/write counts nested under Kilo token usage.
#[derive(Debug, Default, Deserialize)]
struct KiloCache {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    write: u64,
}

/// Creation timestamp block carried by Kilo messages.
#[derive(Debug, Default, Deserialize)]
struct KiloTime {
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    created: Option<i64>,
}

pub(super) fn message_value_to_entry(
    value: &KiloMessage,
    row_id: &str,
    row_session_id: &str,
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    if value.role.as_deref() != Some("assistant") {
        return None;
    }
    let tokens = value.tokens.as_ref()?;
    let cache = tokens.cache.as_ref();
    let usage = TokenUsageRaw {
        input_tokens: tokens.input,
        output_tokens: tokens.output,
        cache_creation_input_tokens: cache.map_or(0, |cache| cache.write),
        cache_read_input_tokens: cache.map_or(0, |cache| cache.read),
        speed: None,
        cache_creation: None,
    };
    let reasoning_tokens = tokens.reasoning;
    let total_tokens = tokens.total;
    let (usage, extra_total_tokens) =
        apply_total_token_fallback(usage, reasoning_tokens, total_tokens);
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
        && extra_total_tokens == 0
    {
        return None;
    }
    let model = value.model_id.clone()?;
    let timestamp = value
        .time
        .as_ref()
        .and_then(|time| time.created)
        .and_then(normalize_timestamp)?;
    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let session_id = value
        .session_id
        .clone()
        .unwrap_or_else(|| row_session_id.to_string());
    let message_id = value
        .id
        .clone()
        .unwrap_or_else(|| format!("{}:{row_id}", db_path.display()));
    let cost_usd = value.cost;
    let data = UsageEntry {
        session_id: Some(session_id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: Some(message_id),
        },
        cost_usd,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let provider = value.provider_id.clone();
    let cost_data = UsageEntry {
        message: UsageMessage {
            usage: TokenUsageRaw {
                output_tokens: data
                    .message
                    .usage
                    .output_tokens
                    .saturating_add(extra_total_tokens),
                cache_creation: None,
                ..data.message.usage
            },
            ..data.message.clone()
        },
        ..data.clone()
    };
    let cost = calculate_kilo_cost(&cost_data, provider.as_deref(), mode, pricing);
    let missing_pricing_model =
        missing_kilo_pricing(&cost_data, provider.as_deref(), mode, pricing);
    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("kilo"),
        session_id: Arc::from(session_id),
        project_path: Arc::from("Kilo"),
        cost,
        extra_total_tokens,
        credits: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        message_count: None,
        data,
    })
}

fn normalize_timestamp(value: i64) -> Option<TimestampMs> {
    if value <= 0 {
        return None;
    }
    let millis = if value < 1_000_000_000_000 {
        value.checked_mul(1000)?
    } else {
        value
    };
    Some(TimestampMs::from_millis(millis))
}

fn calculate_kilo_cost(
    data: &UsageEntry,
    provider: Option<&str>,
    mode: CostMode,
    pricing: &PricingMap,
) -> f64 {
    match mode {
        CostMode::Display => data.cost_usd.unwrap_or(0.0),
        CostMode::Auto => data
            .cost_usd
            .unwrap_or_else(|| calculate_kilo_cost_from_tokens(data, provider, pricing)),
        CostMode::Calculate => calculate_kilo_cost_from_tokens(data, provider, pricing),
    }
}

fn calculate_kilo_cost_from_tokens(
    data: &UsageEntry,
    provider: Option<&str>,
    pricing: &PricingMap,
) -> f64 {
    let Some(model) = data.message.model.as_deref() else {
        return 0.0;
    };
    for candidate in model_candidates(model, provider) {
        if pricing.find(&candidate).is_some() {
            return calculate_cost_for_usage(
                Some(&candidate),
                data.message.usage,
                None,
                CostMode::Calculate,
                Some(pricing),
            );
        }
    }
    0.0
}

fn missing_kilo_pricing(
    data: &UsageEntry,
    provider: Option<&str>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<String> {
    if mode == CostMode::Display || data.cost_usd.is_some_and(|cost| cost > 0.0) {
        return None;
    }
    let model = data.message.model.as_deref()?;
    missing_pricing_model_for_candidates(
        model,
        model_candidates(model, provider),
        crate::total_usage_tokens(data.message.usage),
        Some(pricing),
    )
}

fn model_candidates(model: &str, provider: Option<&str>) -> Vec<String> {
    let mut candidates = Vec::with_capacity(2);
    if let Some(provider) = provider
        .map(normalize_provider)
        .filter(|provider| provider != "unknown" && provider != "kilo")
    {
        candidates.push(format!("{provider}/{model}"));
    }
    candidates.push(model.to_string());
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn normalize_provider(provider: &str) -> String {
    provider.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn keeps_kilo_record_when_cache_field_is_not_an_object() {
        let value = serde_json::from_value::<KiloMessage>(serde_json::json!({
            "id": "msg-1",
            "role": "assistant",
            "providerID": "openai",
            "modelID": "gpt-5",
            "time": { "created": 1767312000000_i64 },
            "tokens": { "input": 100, "output": 10, "cache": 0 }
        }))
        .unwrap();
        let entry = message_value_to_entry(
            &value,
            "row-1",
            "session-a",
            Path::new("/tmp/kilo.db"),
            None,
            CostMode::Auto,
            &PricingMap::load_embedded(),
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 100);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
        assert_eq!(entry.data.message.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn falls_back_to_total_tokens_when_kilo_parts_are_missing() {
        let value = serde_json::from_value::<KiloMessage>(serde_json::json!({
            "id": "msg-1",
            "role": "assistant",
            "providerID": "openai",
            "modelID": "gpt-5",
            "time": { "created": 1767312000000_i64 },
            "tokens": { "total": 234 }
        }))
        .unwrap();
        let entry = message_value_to_entry(
            &value,
            "row-1",
            "session-a",
            Path::new("/tmp/kilo.db"),
            None,
            CostMode::Auto,
            &PricingMap::load_embedded(),
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.output_tokens, 234);
        assert_eq!(entry.extra_total_tokens, 0);
    }
}
