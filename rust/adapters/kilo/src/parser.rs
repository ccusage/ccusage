use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage, apply_total_token_fallback,
    cli::CostMode, format_date_tz, stored_cost_for_mode,
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

#[derive(Debug)]
pub(super) struct MissingTimestamp;

pub(super) fn message_value_to_entry(
    value: &KiloMessage,
    row_id: &str,
    row_session_id: &str,
    db_path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> std::result::Result<Option<(String, LoadedEntry)>, MissingTimestamp> {
    if value.role.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(tokens) = value.tokens.as_ref() else {
        return Ok(None);
    };
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
        return Ok(None);
    }
    let Some(model) = value.model_id.clone() else {
        return Ok(None);
    };
    let timestamp = value
        .time
        .as_ref()
        .and_then(|time| time.created)
        .and_then(normalize_timestamp)
        .ok_or(MissingTimestamp)?;
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
            id: Some(message_id.clone()),
        },
        cost_usd,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let candidates = model_candidates(&model, value.provider_id.as_deref());
    let candidates: Vec<_> = candidates
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let priced = price_entry(
        EntryPricing {
            usage,
            reasoning_tokens: 0,
            residual_tokens: extra_total_tokens,
            reported_model: Some(&model),
            candidates: &candidates,
            stored_cost: cost_usd.filter(|cost| mode != CostMode::Auto || *cost > 0.0),
            timestamp: Some(timestamp),
        },
        pricing,
    );
    let stored_cost = stored_cost_for_mode(cost_usd, mode);
    let cost = stored_cost.unwrap_or(priced.cost);
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| ccusage_core::model_aliases::resolve_model_name(&model).into_owned());
    Ok(Some((
        message_id,
        LoadedEntry {
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
            stored_cost,
            message_count: None,
            data,
        },
    )))
}

fn normalize_timestamp(value: i64) -> Option<TimestampMs> {
    (value > 0)
        .then_some(value)
        .and_then(TimestampMs::from_instant_millis)
}

fn model_candidates(model: &str, provider: Option<&str>) -> Vec<(String, MatchKind)> {
    let mut candidates = Vec::with_capacity(2);
    if let Some(provider) = provider
        .map(normalize_provider)
        .filter(|provider| provider != "unknown" && provider != "kilo")
    {
        candidates.push((format!("{provider}/{model}"), MatchKind::StrictExact));
    }
    candidates.push((model.to_string(), MatchKind::Fuzzy));
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
    fn kilo_creation_time_is_always_milliseconds() {
        assert_eq!(
            normalize_timestamp(1_767_312_000).unwrap().as_millis(),
            1_767_312_000
        );
        assert_eq!(normalize_timestamp(i64::MAX), None);
    }

    #[test]
    fn keeps_kilo_record_when_cache_field_is_not_an_object() {
        ccusage_test_support::install_pricing_snapshots();
        let value = serde_json::from_value::<KiloMessage>(serde_json::json!({
            "id": "msg-1",
            "role": "assistant",
            "providerID": "openai",
            "modelID": "gpt-5",
            "time": { "created": 1767312000000_i64 },
            "tokens": { "input": 100, "output": 10, "cache": 0 }
        }))
        .unwrap();
        let (_, entry) = message_value_to_entry(
            &value,
            "row-1",
            "session-a",
            Path::new("/tmp/kilo.db"),
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 100);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
        assert_eq!(entry.data.message.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn falls_back_to_total_tokens_when_kilo_parts_are_missing() {
        ccusage_test_support::install_pricing_snapshots();
        let value = serde_json::from_value::<KiloMessage>(serde_json::json!({
            "id": "msg-1",
            "role": "assistant",
            "providerID": "openai",
            "modelID": "gpt-5",
            "time": { "created": 1767312000000_i64 },
            "tokens": { "total": 234 }
        }))
        .unwrap();
        let (_, entry) = message_value_to_entry(
            &value,
            "row-1",
            "session-a",
            Path::new("/tmp/kilo.db"),
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.data.message.usage.output_tokens, 234);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    fn deepseek_pricing() -> ccusage_core::PricingMap {
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        pricing
    }

    fn usage_entry(timestamp: &str) -> UsageEntry {
        UsageEntry {
            session_id: Some("session-a".to_string()),
            timestamp: timestamp.to_string(),
            version: None,
            message: UsageMessage {
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    ..TokenUsageRaw::default()
                },
                model: Some("deepseek-v4-flash".to_string()),
                id: Some("message-a".to_string()),
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        }
    }

    #[test]
    fn uses_raw_model_for_timestamped_pricing_when_provider_match_is_not_exact() {
        let pricing = deepseek_pricing();
        let data = usage_entry("2026-08-17T01:00:00Z");

        assert!(
            ({
                let candidates =
                    model_candidates(data.message.model.as_deref().unwrap(), Some("deepseek"));
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        usage: data.message.usage,
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: crate::parse_ts_timestamp(&data.timestamp),
                    },
                    PriceTable::for_map(CostMode::Calculate, &pricing),
                )
                .cost
            } - 0.44)
                < 1e-12
        );
    }

    #[test]
    fn falls_back_to_static_pricing_when_kilo_timestamp_is_invalid() {
        let pricing = deepseek_pricing();
        let data = usage_entry("not-a-timestamp");

        assert_eq!(
            {
                let candidates = model_candidates(data.message.model.as_deref().unwrap(), None);
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        usage: data.message.usage,
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: crate::parse_ts_timestamp(&data.timestamp),
                    },
                    PriceTable::for_map(CostMode::Calculate, &pricing),
                )
                .cost
            },
            0.14
        );
    }

    #[test]
    fn uses_exact_provider_qualified_pricing_when_available() {
        let mut pricing = deepseek_pricing();
        pricing.load_json(
            r#"{
                "reseller/deepseek-v4-flash": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let data = usage_entry("2026-08-17T01:00:00Z");

        assert_eq!(
            {
                let candidates =
                    model_candidates(data.message.model.as_deref().unwrap(), Some("reseller"));
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        usage: data.message.usage,
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: crate::parse_ts_timestamp(&data.timestamp),
                    },
                    PriceTable::for_map(CostMode::Calculate, &pricing),
                )
                .cost
            },
            9.0
        );
    }
}

#[cfg(test)]
mod cost_mode_warnings {
    use super::*;

    #[test]
    fn missing_pricing_tracks_whether_the_mode_uses_recorded_cost() {
        for (mode, stored, expected) in [
            (CostMode::Auto, 0.0, Some("kilo-unpriced-xyz")),
            (CostMode::Auto, -0.01, Some("kilo-unpriced-xyz")),
            (CostMode::Auto, 0.42, None),
            (CostMode::Calculate, 0.42, Some("kilo-unpriced-xyz")),
        ] {
            let value: KiloMessage = serde_json::from_value(serde_json::json!({
                "role": "assistant", "modelID": "kilo-unpriced-xyz", "cost": stored,
                "time": {"created": 1_767_312_000_000_i64},
                "tokens": {"input": 100, "output": 50}
            }))
            .unwrap();
            let (_, entry) = message_value_to_entry(
                &value,
                "r",
                "s",
                Path::new("kilo.db"),
                &JiffTimeZone::UTC,
                mode,
                ccusage_core::cost::PriceTable::for_map(mode, &ccusage_core::PricingMap::default()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                entry.cost,
                if mode == CostMode::Auto { stored } else { 0.0 }
            );
            assert_eq!(entry.missing_pricing_model.as_deref(), expected, "{mode:?}");
        }
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            model_candidates("gpt-5.2", Some("open-ai")),
            vec![
                ("open_ai/gpt-5.2".into(), MatchKind::StrictExact),
                ("gpt-5.2".into(), MatchKind::Fuzzy),
            ]
        );
    }
}
