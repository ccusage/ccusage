use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, TokenUsageRaw, UsageEntry, UsageMessage, apply_total_token_fallback,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

/// A single parsed OpenCode message. Only the fields ccusage consumes are
/// declared; serde skips everything else.
#[derive(Debug, Default, Deserialize)]
pub struct OpenCodeMessage {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    tokens: Option<OpenCodeTokens>,
    #[serde(
        rename = "modelID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(
        rename = "providerID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    provider_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    time: Option<OpenCodeTime>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(
        rename = "sessionID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    cost: Option<f64>,
}

/// Token usage block carried by OpenCode messages.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeTokens {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cache: Option<OpenCodeCache>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total: u64,
}

/// Cache read/write counts nested under OpenCode token usage.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeCache {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    write: u64,
}

/// Creation timestamp block carried by OpenCode messages.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeTime {
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    created: Option<i64>,
}

pub fn message_value_to_entry(
    value: &OpenCodeMessage,
    id: Option<String>,
    session_id: Option<String>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<LoadedEntry> {
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
    let total_tokens = tokens.total;
    let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, total_tokens);
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
        && extra_total_tokens == 0
    {
        return None;
    }
    let model = value.model_id.clone()?;
    let provider = value.provider_id.clone()?;
    let millis = value
        .time
        .as_ref()
        .and_then(|time| time.created)
        .unwrap_or(0);
    let timestamp = crate::TimestampMs::from_millis(millis);
    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let message_id = id.or_else(|| value.id.clone());
    let session_id = session_id.or_else(|| value.session_id.clone());
    let data = UsageEntry {
        session_id: session_id.clone(),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: message_id,
        },
        cost_usd: value.cost,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(extra_total_tokens),
        cache_creation: None,
        ..usage
    };
    let cost =
        calculate_open_code_cost(&model, &provider, cost_usage, data.cost_usd, mode, pricing);
    // If we already have a usable positive cost (calculated or stored), skip
    // the redundant missing-pricing check — it would iterate through the same
    // model candidates and find nothing missing.
    let missing_pricing_model = if cost > 0.0 {
        None
    } else {
        missing_open_code_pricing(&model, &provider, cost_usage, data.cost_usd, mode, pricing)
    };
    let loaded_session_id = data
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("opencode"),
        session_id: Arc::from(loaded_session_id),
        project_path: Arc::from("OpenCode"),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    })
}

fn calculate_open_code_cost(
    model: &str,
    provider: &str,
    usage: TokenUsageRaw,
    cost_usd: Option<f64>,
    _mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    if let Some(cost) = cost_usd.filter(|cost| *cost > 0.0) {
        return cost;
    }
    for candidate in open_code_model_candidates(model, provider) {
        let cost =
            calculate_cost_for_usage(Some(&candidate), usage, None, CostMode::Calculate, pricing);
        if cost > 0.0 {
            return cost;
        }
    }
    0.0
}

fn missing_open_code_pricing(
    model: &str,
    provider: &str,
    usage: TokenUsageRaw,
    cost_usd: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if mode == CostMode::Display || cost_usd.is_some_and(|cost| cost > 0.0) {
        return None;
    }
    missing_pricing_model_for_candidates(
        model,
        open_code_model_candidates(model, provider),
        crate::total_usage_tokens(usage),
        pricing,
    )
}

fn open_code_model_candidates(model: &str, provider: &str) -> Vec<String> {
    let resolved = resolve_open_code_model_name(model);
    let normalized = normalize_open_code_model_name(&resolved);
    let mut base = vec![resolved];
    if normalized != base[0] {
        base.push(normalized);
    }
    let mut candidates = base.clone();
    if provider != "unknown" {
        let provider = provider.replace('-', "_");
        candidates.extend(base.into_iter().map(|model| format!("{provider}/{model}")));
    }
    candidates.dedup();
    candidates
}

fn resolve_open_code_model_name(model: &str) -> String {
    match model {
        "gemini-3-pro-high" => "gemini-3-pro-preview".to_string(),
        "k2p6" => "kimi-k2.6".to_string(),
        _ => model.to_string(),
    }
}

fn normalize_open_code_model_name(model: &str) -> String {
    for family in ["claude-haiku-", "claude-opus-", "claude-sonnet-"] {
        if let Some(rest) = model.strip_prefix(family) {
            if let Some((major, minor_and_suffix)) = rest.split_once('.')
                && major.chars().all(|ch| ch.is_ascii_digit())
                && minor_and_suffix
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
            {
                return format!("{family}{major}-{minor_and_suffix}");
            }
            let mut chars = rest.chars();
            if let (Some(major), Some(minor)) = (chars.next(), chars.next())
                && major.is_ascii_digit()
                && minor.is_ascii_digit()
            {
                return format!("{family}{major}-{minor}{}", chars.collect::<String>());
            }
        }
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OpenCodeMessage, message_value_to_entry, open_code_model_candidates};
    use crate::{LoadedEntry, PricingMap, cli::CostMode};

    fn message(value: serde_json::Value) -> OpenCodeMessage {
        serde_json::from_value(value).unwrap()
    }

    fn entry_snapshot(entry: &LoadedEntry) -> serde_json::Value {
        json!({
            "date": entry.date,
            "timestamp": entry.timestamp.as_millis(),
            "sessionId": entry.session_id.as_ref(),
            "project": entry.project.as_ref(),
            "projectPath": entry.project_path.as_ref(),
            "cost": entry.cost,
            "extraTotalTokens": entry.extra_total_tokens,
            "model": entry.model.as_deref(),
            "data": {
                "sessionId": entry.data.session_id.as_deref(),
                "timestamp": entry.data.timestamp,
                "version": entry.data.version.as_deref(),
                "message": {
                    "id": entry.data.message.id.as_deref(),
                    "model": entry.data.message.model.as_deref(),
                    "usage": {
                        "inputTokens": entry.data.message.usage.input_tokens,
                        "outputTokens": entry.data.message.usage.output_tokens,
                        "cacheCreationInputTokens": entry.data.message.usage.cache_creation_input_tokens,
                        "cacheReadInputTokens": entry.data.message.usage.cache_read_input_tokens,
                    },
                },
                "costUSD": entry.data.cost_usd,
            },
        })
    }

    #[test]
    fn calculates_cost_when_opencode_stores_zero_cost() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                }
            }"#,
        );
        let entry = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50 }
                },
                "cost": 0
            })),
            None,
            None,
            None,
            CostMode::Auto,
            Some(&pricing),
        )
        .unwrap();

        assert_eq!(entry.cost, 0.000205);
    }

    #[test]
    fn keeps_positive_opencode_cost() {
        let entry = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": {
                    "input": 100
                },
                "cost": 0.02
            })),
            None,
            None,
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();

        assert_eq!(entry.cost, 0.02);
    }

    #[test]
    fn keeps_opencode_record_when_cache_field_is_not_an_object() {
        let entry = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": 0
                },
                "cost": 0.02
            })),
            None,
            None,
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 100);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
        assert_eq!(entry.data.message.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 0);
        assert_eq!(entry.cost, 0.02);
    }

    #[test]
    fn falls_back_to_total_tokens_when_opencode_token_parts_are_missing() {
        let entry = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": {
                    "total": 123
                }
            })),
            None,
            None,
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.output_tokens, 123);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn creates_open_code_provider_and_normalized_model_candidates() {
        assert_eq!(
            open_code_model_candidates("claude-sonnet-4.5", "github-copilot"),
            vec![
                "claude-sonnet-4.5",
                "claude-sonnet-4-5",
                "github_copilot/claude-sonnet-4.5",
                "github_copilot/claude-sonnet-4-5",
            ]
        );
    }

    #[test]
    fn calculates_cost_for_k2p6_when_opencode_stores_zero_cost() {
        let pricing = PricingMap::load_embedded();
        let entry = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "kimi-for-coding",
                "modelID": "k2p6",
                "time": { "created": 0 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50 }
                },
                "cost": 0
            })),
            None,
            None,
            None,
            CostMode::Auto,
            Some(&pricing),
        )
        .unwrap();

        assert_eq!(entry.cost, 0.000143);
    }

    #[test]
    fn snapshots_message_to_entry_variants_and_model_candidates() {
        let tz = crate::parse_tz(Some("UTC"));
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "github_copilot/claude-sonnet-4-5": {
                    "input_cost_per_token": 0.125,
                    "output_cost_per_token": 0.25,
                    "cache_read_input_token_cost": 0.0625
                }
            }"#,
        );
        let calculated = message_value_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "github-copilot",
                "modelID": "claude-sonnet-4.5",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50, "write": 25 },
                    "total": 185
                },
                "cost": 0
            })),
            None,
            None,
            tz.as_ref(),
            CostMode::Auto,
            Some(&pricing),
        )
        .unwrap();
        let display_cost = message_value_to_entry(
            &message(json!({
                "id": "message-b",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": { "total": 123 },
                "cost": 0.02
            })),
            None,
            Some("explicit-session".to_string()),
            tz.as_ref(),
            CostMode::Display,
            None,
        )
        .unwrap();

        insta::assert_json_snapshot!(json!({
            "calculated": entry_snapshot(&calculated),
            "displayCost": entry_snapshot(&display_cost),
            "candidates": {
                "anthropic": open_code_model_candidates("claude-sonnet-4.5", "anthropic"),
                "copilot": open_code_model_candidates("claude-sonnet-4.5", "github-copilot"),
                "geminiAlias": open_code_model_candidates("gemini-3-pro-high", "google"),
                "unknownProvider": open_code_model_candidates("gpt-test", "unknown"),
            }
        }));
    }
}
