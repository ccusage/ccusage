use ccusage_core::cost::PriceTable;
use ccusage_core::{
    AdditionalTokens,
    cost::{EntryPricing, UsageGranularity},
    pricing::MatchKind,
};
use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, stored_cost_for_mode,
};
use ccusage_adapter_common::{
    billing::{host_provider, price_entry_for_provider},
    jsonl,
};

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

#[derive(Debug, Default, Deserialize)]
struct KiloCache {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    write: u64,
}

// NOTE(2026-05-19, #1032): The data column of Kilo's SQLite message table holds a JSON payload with time.created in epoch milliseconds. Use that field as the message clock.
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
    let additional = AdditionalTokens::from_total(usage, tokens.reasoning, tokens.total);
    let (report_usage, extra_total_tokens) = additional.for_report(usage);
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
        && additional.total() == 0
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

    let session_id = value
        .session_id
        .clone()
        .unwrap_or_else(|| row_session_id.to_string());
    let message_id = value
        .id
        .clone()
        .unwrap_or_else(|| format!("{}:{row_id}", db_path.display()));
    let cost_usd = value.cost;
    let recorded = ccusage_core::cost::recorded_cost(cost_usd);
    let candidates = model_candidates(&model, value.provider_id.as_deref());
    let candidates: Vec<_> = candidates
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Request,
        usage,
        reasoning_tokens: additional.reasoning,
        residual_tokens: additional.residual,
        reported_model: Some(&model),
        candidates: &candidates,
        stored_cost: recorded,
        timestamp: Some(timestamp),
    };
    let priced = price_entry_for_provider(entry_pricing, value.provider_id.as_deref(), pricing);
    let stored_cost = stored_cost_for_mode(recorded, mode);
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| ccusage_core::model_aliases::resolve_model_name(&model).into_owned());
    Ok(Some((
        message_id.clone(),
        LoadedEntry {
            date: date_at(timestamp, tz),
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
            estimated_pricing,
            stored_cost,
            message_count: None,
            usage: report_usage,
            message_id: Some(message_id),
            version: None,
            recorded_cost: cost_usd,
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
    host_provider(provider).map_or_else(|| provider.replace('-', "_"), str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_core::cost::price_entry;

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
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.usage.input_tokens, 100);
        assert_eq!(entry.usage.output_tokens, 10);
        assert_eq!(entry.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.usage.cache_read_input_tokens, 0);
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
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.usage.output_tokens, 234);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    fn deepseek_pricing() -> ccusage_core::PricingMap {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        pricing
    }

    fn usage() -> TokenUsageRaw {
        TokenUsageRaw {
            input_tokens: 1_000_000,
            ..TokenUsageRaw::default()
        }
    }

    #[test]
    fn uses_raw_model_for_timestamped_pricing_when_provider_match_is_not_exact() {
        let pricing = deepseek_pricing();
        let timestamp = "2026-08-17T01:00:00Z";

        assert!(
            ({
                let candidates = model_candidates("deepseek-v4-flash", Some("deepseek"));
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        granularity: UsageGranularity::Request,
                        usage: usage(),
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: ccusage_core::parse_ts_timestamp(timestamp),
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
        let timestamp = "not-a-timestamp";

        assert_eq!(
            {
                let candidates = model_candidates("deepseek-v4-flash", None);
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        granularity: UsageGranularity::Request,
                        usage: usage(),
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: ccusage_core::parse_ts_timestamp(timestamp),
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
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = deepseek_pricing();
        pricing.load_json(
            r#"{
                "reseller/deepseek-v4-flash": {
                    "input_cost_per_token": 0.000009,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let timestamp = "2026-08-17T01:00:00Z";

        assert_eq!(
            {
                let candidates = model_candidates("deepseek-v4-flash", Some("reseller"));
                let candidates: Vec<_> = candidates
                    .iter()
                    .map(|(name, kind)| (name.as_str(), *kind))
                    .collect();
                price_entry(
                    EntryPricing {
                        granularity: UsageGranularity::Request,
                        usage: usage(),
                        reasoning_tokens: 0,
                        residual_tokens: 0,
                        reported_model: None,
                        candidates: &candidates,
                        stored_cost: None,
                        timestamp: ccusage_core::parse_ts_timestamp(timestamp),
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
                if mode == CostMode::Auto && stored > 0.0 {
                    stored
                } else {
                    0.0
                }
            );
            assert_eq!(entry.missing_pricing_model.as_deref(), expected, "{mode:?}");
        }
    }
}

#[cfg(test)]
mod paid_host_estimates {
    use super::*;

    #[test]
    fn a_hyphenated_paid_host_marks_a_free_vendor_model_as_an_estimate() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (provider, estimated) in [("novita-ai", true), ("ollama", false)] {
            let value: KiloMessage = serde_json::from_value(serde_json::json!({
                "role": "assistant", "modelID": "zai-org/glm-4.7-flash", "providerID": provider,
                "time": {"created": 1_767_312_000_000_i64},
                "tokens": {"input": 1_000_000, "output": 100_000}
            }))
            .unwrap();
            let (_, entry) = message_value_to_entry(
                &value,
                "r",
                "s",
                Path::new("kilo.db"),
                &JiffTimeZone::UTC,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing),
            )
            .unwrap()
            .unwrap();
            assert_eq!(entry.estimated_pricing, estimated, "{provider}");
            assert_eq!(entry.cost > 0.0, estimated, "{provider}: {}", entry.cost);
        }
    }
}

#[cfg(test)]
mod recorded_zero_cost {
    use super::*;

    fn auto_cost(model: &str, provider: &str) -> f64 {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let value: KiloMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant", "modelID": model, "providerID": provider, "cost": 0,
            "time": {"created": 1_767_312_000_000_i64},
            "tokens": {"input": 100_000, "output": 10_000}
        }))
        .unwrap();
        let (_, entry) = message_value_to_entry(
            &value,
            "r",
            "s",
            Path::new("kilo.db"),
            &JiffTimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.missing_pricing_model, None);
        entry.cost
    }

    #[test]
    fn a_recorded_zero_cost_on_a_priced_model_is_priced_from_tokens() {
        let cost = auto_cost("gpt-5.5", "openai");
        assert!((cost - 0.8).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn a_recorded_zero_cost_on_a_free_model_stays_zero() {
        assert_eq!(auto_cost("glm-4.7-flash", "zai"), 0.0);
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;
    use ccusage_core::cost::price_entry;

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

    #[test]
    fn adapter_pricing_33_applies_the_schedule_to_the_provider_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let model = "deepseek-v4-flash";
        let candidates = model_candidates(model, Some("deepseek"));
        let candidates: Vec<_> = candidates
            .iter()
            .map(|(id, kind)| (id.as_str(), *kind))
            .collect();
        let entry = price_entry(
            EntryPricing {
                granularity: UsageGranularity::Request,
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Default::default()
                },
                reasoning_tokens: 0,
                residual_tokens: 0,
                reported_model: Some(model),
                candidates: &candidates,
                stored_cost: None,
                timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z"),
            },
            PriceTable::Calculate(&pricing),
        );
        assert!((entry.cost - 0.00075).abs() < 1e-12);
    }

    #[test]
    fn prices_a_foreign_router_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let created = ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z")
            .unwrap()
            .as_millis();
        for (provider, expected) in [(Some("openrouter"), 0.0015), (Some("deepseek"), 0.00075)] {
            let value = serde_json::from_value::<KiloMessage>(serde_json::json!({
                "id": "msg-deepseek",
                "role": "assistant",
                "providerID": provider,
                "modelID": "deepseek-v4-flash",
                "time": { "created": created },
                "tokens": { "input": 1000, "output": 1000 }
            }))
            .unwrap();
            let (_, entry) = message_value_to_entry(
                &value,
                "row-deepseek",
                "session-a",
                Path::new("/tmp/kilo.db"),
                &jiff::tz::TimeZone::UTC,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing),
            )
            .unwrap()
            .unwrap();

            assert!(
                (entry.cost - expected).abs() < 1e-12,
                "{provider:?}: {}",
                entry.cost
            );
            assert_eq!(entry.missing_pricing_model, None, "{provider:?}");
        }
    }
}
