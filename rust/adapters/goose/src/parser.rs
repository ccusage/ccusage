use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::MatchKind,
};
use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{LoadedEntry, TokenUsageRaw, date_at};
use ccusage_adapter_common::{
    billing::{host_provider, price_entry_for_provider},
    jsonl,
    sqlite::read_i64,
};
use ccusage_core::SkipKind;

// NOTE(2026-05-19, #1035): Goose embeds the model name in model_config_json on the SQLite session row. Parsing that object recovers the session's model attribution.
#[derive(Debug, Deserialize)]
struct GooseModelConfig {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RowRejection {
    MissingId,
    MissingModel,
    InvalidTimestamp,
}

impl RowRejection {
    pub(super) const fn kind(self) -> SkipKind {
        match self {
            Self::MissingId | Self::MissingModel => SkipKind::InvalidData,
            Self::InvalidTimestamp => SkipKind::InvalidTimestamp,
        }
    }

    pub(super) const fn detail(self) -> &'static str {
        match self {
            Self::MissingId => "Goose session row with usage has no session id",
            Self::MissingModel => "Goose session row with usage names no model",
            Self::InvalidTimestamp => "Goose session row with usage has an invalid created_at",
        }
    }
}

pub(super) fn row_to_entry(
    statement: &sqlite::Statement<'_>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> Result<Option<LoadedEntry>, RowRejection> {
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
        return Ok(None);
    }

    let id = statement
        .read::<String, _>(0)
        .map_err(|_| RowRejection::MissingId)?;
    let model = statement
        .read::<String, _>(1)
        .ok()
        .and_then(|config| parse_goose_model_config(&config))
        .ok_or(RowRejection::MissingModel)?;
    let provider_name = statement.read::<String, _>(2).ok();
    let timestamp = read_timestamp_value(statement, 3)
        .and_then(|created_at| parse_goose_timestamp(&created_at))
        .ok_or(RowRejection::InvalidTimestamp)?;

    let reasoning_tokens = total_tokens.saturating_sub(input_tokens.saturating_add(output_tokens));
    let provider_name = provider_name
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let provider_id = normalize_provider(provider_name, &model);
    let usage = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };

    let mut candidates = vec![(model.clone(), MatchKind::Fuzzy)];
    if provider_id != "goose" {
        let kind = if provider_name.is_some() {
            MatchKind::Exact
        } else {
            MatchKind::Fuzzy
        };
        candidates.push((format!("{provider_id}/{model}"), kind));
    }
    let candidates: Vec<_> = candidates
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Aggregate,
        usage,
        reasoning_tokens,
        residual_tokens: 0,
        reported_model: Some(&model),
        candidates: &candidates,
        stored_cost: None,
        timestamp: Some(timestamp),
    };
    let priced = price_entry_for_provider(entry_pricing, provider_name, pricing);
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| ccusage_core::model_aliases::resolve_model_name(&model).into_owned());

    Ok(Some(LoadedEntry {
        date: date_at(timestamp, tz),
        timestamp,
        project: Arc::from("goose"),
        session_id: Arc::from(id.as_str()),
        project_path: Arc::from("Goose"),
        cost,
        credits: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        extra_total_tokens: reasoning_tokens,
        message_count: None,
        usage,
        message_id: Some(id.clone()),
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }))
}

fn read_token_value(statement: &sqlite::Statement<'_>, index: usize) -> Option<u64> {
    read_i64(statement, index)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn read_timestamp_value(statement: &sqlite::Statement<'_>, index: usize) -> Option<String> {
    statement.read::<Option<String>, _>(index).ok().flatten()
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
        return ccusage_adapter_common::timestamps::epoch_secs_or_millis(number);
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
    if let Some(provider) = provider {
        return host_provider(provider).map_or_else(|| provider.replace('-', "_"), str::to_owned);
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

#[cfg(test)]
mod tests {
    #[test]
    fn zero_rates_stop_candidate_fallback() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut row = connection.prepare(r#"SELECT 's', '{"model_name":"sample"}', 'vendor', '2026-05-19 00:00:00', 100, 100, 0, 100, 100, 0"#).unwrap();
        assert_eq!(row.next().unwrap(), sqlite::State::Row);
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"sample":{"litellm_provider": "openai", "input_cost_per_token":1,"output_cost_per_token":1},"vendor/sample":{"input_cost_per_token":0,"output_cost_per_token":0}}"#);
        let loaded = super::row_to_entry(
            &row,
            &jiff::tz::TimeZone::UTC,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.cost, 0.0);
        assert_eq!(loaded.missing_pricing_model, None);
    }

    #[test]
    fn mixed_epoch_units_use_one_boundary() {
        for (raw, millis) in [
            ("1767312000", 1_767_312_000_000),
            ("10000000000", 10_000_000_000),
            ("1000000000000", 1_000_000_000_000),
        ] {
            assert_eq!(
                super::parse_goose_timestamp(raw).unwrap().as_millis(),
                millis,
                "{raw}"
            );
        }
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let model = parse_goose_model_config(r#"{"model_name":"gpt-5.2"}"#).unwrap();
        assert_eq!(model, "gpt-5.2");
        assert_eq!(normalize_provider(None, &model), "openai");
        assert_eq!(normalize_provider(Some("open-ai"), &model), "open_ai");
    }

    #[test]
    fn a_session_total_is_priced_at_the_base_tier_not_the_long_context_tier() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"gemini-2.5-pro":{"litellm_provider":"gemini","input_cost_per_token":1.25e-6,"output_cost_per_token":1e-5,"input_cost_per_token_above_200k_tokens":2.5e-6,"output_cost_per_token_above_200k_tokens":1.5e-5}}"#,
        );
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut row = connection
            .prepare(r#"SELECT 's', '{"model_name":"gemini-2.5-pro"}', NULL, '2026-05-19 00:00:00', NULL, NULL, NULL, 5000000, 4900000, 100000"#)
            .unwrap();
        assert_eq!(row.next().unwrap(), sqlite::State::Row);

        let loaded = row_to_entry(&row, &JiffTimeZone::UTC, PriceTable::Calculate(&pricing))
            .unwrap()
            .unwrap();

        assert!((loaded.cost - 7.125).abs() < 1e-9, "{}", loaded.cost);
    }

    #[test]
    fn inferred_openrouter_does_not_override_the_vendor_price() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "qwen-test":{"litellm_provider":"dashscope","input_cost_per_token":1,"output_cost_per_token":1},
                "openrouter/qwen-test":{"litellm_provider":"openrouter","input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
        );
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut row = connection
            .prepare(
                r#"SELECT 's', '{"model_name":"qwen-test"}', ?, '2026-11-17T10:00:00Z', 100, 100, 0, 100, 100, 0"#,
            )
            .unwrap();
        for (provider, expected) in [
            (None, 100.0),
            (Some(" "), 100.0),
            (Some("openrouter"), 200.0),
        ] {
            if let Some(provider) = provider {
                row.bind((1, provider)).unwrap();
            } else {
                row.bind((1, sqlite::Value::Null)).unwrap();
            }
            assert_eq!(row.next().unwrap(), sqlite::State::Row);
            let loaded = row_to_entry(&row, &JiffTimeZone::UTC, PriceTable::Calculate(&pricing))
                .unwrap()
                .unwrap();
            assert_eq!(loaded.cost, expected, "{provider:?}");
            row.reset().unwrap();
        }
    }

    #[test]
    fn prices_a_foreign_router_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        for (provider, expected) in [("openrouter", 0.0015), ("deepseek", 0.00075)] {
            let mut row = connection
                .prepare(format!(
                    r#"SELECT 's', '{{"model_name":"deepseek-v4-flash"}}', '{provider}', '2026-11-17T10:00:00Z', 2000, 1000, 1000, 2000, 1000, 1000"#
                ))
                .unwrap();
            assert_eq!(row.next().unwrap(), sqlite::State::Row);
            let loaded = row_to_entry(&row, &JiffTimeZone::UTC, PriceTable::Calculate(&pricing))
                .unwrap()
                .unwrap();
            assert!(
                (loaded.cost - expected).abs() < 1e-12,
                "{provider}: {}",
                loaded.cost
            );
            assert_eq!(loaded.missing_pricing_model, None, "{provider}");
        }
    }
}
