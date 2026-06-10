use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, OpenCodeMessage, PricingMap, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, cli::CostMode, format_date_tz,
    missing_pricing_model_for_candidates,
};

/// Trim a string and discard it if it is empty, matching the original
/// `Value`-based parser's `non_empty_json_string` handling.
fn non_empty(value: Option<&String>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn message_to_entry(
    msg: &OpenCodeMessage,
    id: Option<String>,
    session_id: Option<String>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<LoadedEntry> {
    let tokens = msg.tokens.as_ref()?;
    let usage = TokenUsageRaw {
        input_tokens: tokens.input,
        output_tokens: tokens.output,
        cache_creation_input_tokens: tokens.cache.as_ref().map_or(0, |c| c.write),
        cache_read_input_tokens: tokens.cache.as_ref().map_or(0, |c| c.read),
        speed: None,
        cache_creation: None,
    };
    let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, tokens.total);
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
        && extra_total_tokens == 0
    {
        return None;
    }
    let model = non_empty(msg.model_id.as_ref())?;
    let provider = non_empty(msg.provider_id.as_ref())?;
    let millis = msg.time.as_ref().and_then(|t| t.created).unwrap_or(0);
    let timestamp = crate::TimestampMs::from_millis(millis);
    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let message_id = id.or_else(|| non_empty(msg.id.as_ref()));
    let session_id = session_id.or_else(|| non_empty(msg.session_id.as_ref()));
    let data = UsageEntry {
        session_id: session_id.clone(),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: message_id,
            provider: Some(provider.clone()),
        },
        cost_usd: msg.cost,
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
    let missing_pricing_model =
        missing_open_code_pricing(&model, &provider, cost_usage, data.cost_usd, mode, pricing);
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

/// Recompute cost and missing-pricing for a cached entry from its stored tokens,
/// provider hint, and logged cost, mirroring `message_to_entry` (billable output
/// folds in `extra_total_tokens`).
pub(crate) fn reprice(entry: &mut LoadedEntry, mode: CostMode, pricing: Option<&PricingMap>) {
    let model = entry.data.message.model.clone().unwrap_or_default();
    let provider = entry.data.message.provider.clone().unwrap_or_default();
    let cost_usage = TokenUsageRaw {
        output_tokens: entry
            .data
            .message
            .usage
            .output_tokens
            .saturating_add(entry.extra_total_tokens),
        cache_creation: None,
        ..entry.data.message.usage
    };
    let cost_usd = entry.data.cost_usd;
    entry.cost = calculate_open_code_cost(&model, &provider, cost_usage, cost_usd, mode, pricing);
    entry.missing_pricing_model =
        missing_open_code_pricing(&model, &provider, cost_usage, cost_usd, mode, pricing);
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

    use super::{message_to_entry, open_code_model_candidates};
    use crate::{
        LoadedEntry, OpenCodeCache, OpenCodeMessage, OpenCodeTime, OpenCodeTokens, PricingMap,
        cli::CostMode,
    };

    fn test_message(tokens: OpenCodeTokens) -> OpenCodeMessage {
        OpenCodeMessage {
            id: Some("message-a".to_string()),
            session_id: Some("session-a".to_string()),
            provider_id: Some("openai".to_string()),
            model_id: Some("gpt-test".to_string()),
            time: Some(OpenCodeTime { created: Some(0) }),
            tokens: Some(tokens),
            cost: Some(0.0),
        }
    }

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
        let entry = message_to_entry(
            &test_message(OpenCodeTokens {
                input: 100,
                output: 10,
                total: 0,
                cache: Some(OpenCodeCache { read: 50, write: 0 }),
            }),
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
        let entry = message_to_entry(
            &OpenCodeMessage {
                cost: Some(0.02),
                ..test_message(OpenCodeTokens {
                    input: 100,
                    output: 0,
                    total: 0,
                    cache: None,
                })
            },
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
        let entry = message_to_entry(
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
    fn coerces_string_typed_numeric_fields_without_dropping_record() {
        let entry = message_to_entry(
            &message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 0 },
                "tokens": {
                    "input": "100",
                    "output": 10
                },
                "cost": 0.02
            })),
            None,
            None,
            None,
            CostMode::Auto,
            None,
        )
        .expect("record with a mistyped token field must still be recorded");

        assert_eq!(entry.data.message.usage.input_tokens, 100);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
        assert_eq!(entry.cost, 0.02);
    }

    #[test]
    fn falls_back_to_total_tokens_when_opencode_token_parts_are_missing() {
        let entry = message_to_entry(
            &test_message(OpenCodeTokens {
                input: 0,
                output: 0,
                total: 123,
                cache: None,
            }),
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
        let entry = message_to_entry(
            &OpenCodeMessage {
                provider_id: Some("kimi-for-coding".to_string()),
                model_id: Some("k2p6".to_string()),
                ..test_message(OpenCodeTokens {
                    input: 100,
                    output: 10,
                    total: 0,
                    cache: Some(OpenCodeCache { read: 50, write: 0 }),
                })
            },
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
        let calculated = message_to_entry(
            &OpenCodeMessage {
                id: Some("message-a".to_string()),
                session_id: Some("session-a".to_string()),
                provider_id: Some("github-copilot".to_string()),
                model_id: Some("claude-sonnet-4.5".to_string()),
                time: Some(OpenCodeTime {
                    created: Some(1767312000000),
                }),
                tokens: Some(OpenCodeTokens {
                    input: 100,
                    output: 10,
                    total: 185,
                    cache: Some(OpenCodeCache {
                        read: 50,
                        write: 25,
                    }),
                }),
                cost: Some(0.0),
            },
            None,
            None,
            None,
            CostMode::Auto,
            Some(&pricing),
        )
        .unwrap();
        let display_cost = message_to_entry(
            &OpenCodeMessage {
                id: Some("message-b".to_string()),
                session_id: None,
                provider_id: Some("openai".to_string()),
                model_id: Some("gpt-test".to_string()),
                time: Some(OpenCodeTime { created: Some(0) }),
                tokens: Some(OpenCodeTokens {
                    input: 0,
                    output: 0,
                    total: 123,
                    cache: None,
                }),
                cost: Some(0.02),
            },
            None,
            Some("explicit-session".to_string()),
            None,
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
