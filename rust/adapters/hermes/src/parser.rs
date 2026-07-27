use std::{collections::HashSet, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_candidates,
};

pub(super) struct HermesEntry {
    pub(super) timestamp: TimestampMs,
    timestamp_text: String,
    pub(super) session_id: String,
    model: String,
    provider: String,
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    message_count: u64,
    cost_usd: Option<f64>,
}

pub(super) fn read_session_row(statement: &sqlite::Statement<'_>) -> Option<HermesEntry> {
    let session_id = statement.read::<String, _>(0).ok()?;
    let model = statement.read::<String, _>(1).ok()?.trim().to_string();
    if session_id.is_empty() || model.is_empty() {
        return None;
    }
    let provider_raw = statement.read::<String, _>(2).ok();
    let started_at = read_f64(statement, 3)?;
    let timestamp = timestamp_from_number(started_at)?;
    let message_count = read_u64(statement, 4);
    let input_tokens = read_u64(statement, 5);
    let output_tokens = read_u64(statement, 6);
    let cache_read_tokens = read_u64(statement, 7);
    let cache_creation_tokens = read_u64(statement, 8);
    let reasoning_tokens = read_u64(statement, 9);
    let estimated_cost = read_non_negative_f64(statement, 10);
    let actual_cost = read_non_negative_f64(statement, 11);
    let cost_usd = actual_cost.or(estimated_cost);
    if input_tokens == 0
        && output_tokens == 0
        && cache_read_tokens == 0
        && cache_creation_tokens == 0
        && reasoning_tokens == 0
        && cost_usd.unwrap_or(0.0) == 0.0
    {
        return None;
    }
    Some(HermesEntry {
        timestamp,
        timestamp_text: format_rfc3339_millis(timestamp),
        session_id,
        provider: normalize_provider(provider_raw.as_deref(), &model),
        model,
        usage: TokenUsageRaw {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: cache_creation_tokens,
            cache_read_input_tokens: cache_read_tokens,
            speed: None,
            cache_creation: None,
        },
        reasoning_tokens,
        message_count,
        cost_usd,
    })
}

fn read_u64(statement: &sqlite::Statement<'_>, index: usize) -> u64 {
    statement
        .read::<i64, _>(index)
        .ok()
        .and_then(|value| u64::try_from(value.max(0)).ok())
        .or_else(|| {
            statement
                .read::<f64, _>(index)
                .ok()
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| value.trunc() as u64)
        })
        .unwrap_or(0)
}

fn read_f64(statement: &sqlite::Statement<'_>, index: usize) -> Option<f64> {
    statement
        .read::<f64, _>(index)
        .ok()
        .filter(|value| value.is_finite())
        .or_else(|| {
            statement
                .read::<i64, _>(index)
                .ok()
                .map(|value| value as f64)
        })
}

fn read_non_negative_f64(statement: &sqlite::Statement<'_>, index: usize) -> Option<f64> {
    read_f64(statement, index).map(|value| value.max(0.0))
}

fn timestamp_from_number(value: f64) -> Option<TimestampMs> {
    if !value.is_finite() {
        return None;
    }
    let millis = if value > 1e12 { value } else { value * 1000.0 };
    (millis > 0.0).then(|| TimestampMs::from_millis(millis.trunc() as i64))
}

fn normalize_provider(value: Option<&str>, model: &str) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return infer_provider_from_model(model).to_string();
    };
    let normalized = value.to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "anthropic" | "claude" => "anthropic".to_string(),
        "openai" | "openai_codex" => "openai".to_string(),
        "google" | "google_ai" | "gemini" | "vertex" | "vertex_ai" => "google".to_string(),
        "openrouter" => "openrouter".to_string(),
        "xai" => "xai".to_string(),
        "groq" => "groq".to_string(),
        value => value.to_string(),
    }
}

fn infer_provider_from_model(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    if model.starts_with("claude-") || model.starts_with("claude/") {
        "anthropic"
    } else if model.starts_with("gpt")
        || model.starts_with("chatgpt")
        || model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        "openai"
    } else if model.starts_with("gemini-") || model.starts_with("gemini/") {
        "google"
    } else {
        "hermes"
    }
}

pub(super) fn to_loaded_entry(
    entry: HermesEntry,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost = calculate_hermes_cost(&entry, pricing);
    let missing_pricing_model = missing_hermes_pricing(&entry, pricing);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(format!("hermes:{}", entry.session_id)),
        },
        cost_usd: entry.cost_usd,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("hermes"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Hermes"),
        cost,
        credits: None,
        extra_total_tokens: entry.reasoning_tokens,
        message_count: Some(entry.message_count),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

fn calculate_hermes_cost(entry: &HermesEntry, pricing: &PricingMap) -> f64 {
    if let Some(cost) = entry.cost_usd.filter(|cost| *cost > 0.0) {
        return cost;
    }
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    for candidate in model_candidates(entry) {
        let cost = calculate_cost_for_usage(
            Some(&candidate),
            usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        );
        if cost.is_finite() && cost > 0.0 {
            return cost;
        }
    }
    0.0
}

fn missing_hermes_pricing(entry: &HermesEntry, pricing: &PricingMap) -> Option<String> {
    if entry.cost_usd.is_some_and(|cost| cost > 0.0) {
        return None;
    }
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    missing_pricing_model_for_candidates(
        &entry.model,
        model_candidates(entry),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

fn model_candidates(entry: &HermesEntry) -> Vec<String> {
    let mut candidates = Vec::new();
    if entry.provider != "hermes" {
        candidates.push(format!("{}/{}", entry.provider, entry.model));
    }
    candidates.push(entry.model.clone());
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_cost_for_hermes_frontier_models_from_embedded_pricing() {
        let pricing = PricingMap::load_embedded();
        for model in ["gpt-5.5", "grok-4.3"] {
            let entry = HermesEntry {
                timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),
                timestamp_text: "2026-05-19T00:00:00.000Z".to_string(),
                session_id: format!("session-{model}"),
                model: model.to_string(),
                provider: "hermes".to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    speed: None,
                    cache_creation: None,
                },
                reasoning_tokens: 50,
                message_count: 1,
                cost_usd: None,
            };

            assert!(
                calculate_hermes_cost(&entry, &pricing) > 0.0,
                "{model} should resolve to embedded pricing"
            );
        }
    }

    #[test]
    fn recorded_zero_cost_falls_back_to_pricing_calculation() {
        let pricing = PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),
            timestamp_text: "2026-05-19T00:00:00.000Z".to_string(),
            session_id: "subscription-included".to_string(),
            model: "gpt-5.5".to_string(),
            provider: "openai".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 244_075,
                output_tokens: 10_019,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 3_339_776,
                speed: None,
                cache_creation: None,
            },
            reasoning_tokens: 3_216,
            message_count: 72,
            cost_usd: Some(0.0),
        };

        assert!(
            calculate_hermes_cost(&entry, &pricing) > 0.0,
            "subscription-included sessions with token usage should still be priced"
        );
    }

    #[test]
    fn recorded_positive_cost_is_trusted() {
        let pricing = PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),
            timestamp_text: "2026-05-19T00:00:00.000Z".to_string(),
            session_id: "metered".to_string(),
            model: "gpt-5.5".to_string(),
            provider: "openai".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000,
                output_tokens: 100,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: Some(0.42),
        };

        assert_eq!(calculate_hermes_cost(&entry, &pricing), 0.42);
    }

    #[test]
    fn tries_provider_qualified_model_candidate_first() {
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),
            timestamp_text: "2026-05-19T00:00:00.000Z".to_string(),
            session_id: "session-provider".to_string(),
            model: "gpt-5.5".to_string(),
            provider: "openai".to_string(),
            usage: TokenUsageRaw::default(),
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: None,
        };

        assert_eq!(
            model_candidates(&entry),
            vec!["openai/gpt-5.5".to_string(), "gpt-5.5".to_string()]
        );
    }
}
