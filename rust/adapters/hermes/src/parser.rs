use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::sync::Arc;

use ccusage_adapter_common::sqlite::{read_count, read_f64};
use ccusage_core::SkipKind;
use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage, cli::CostMode,
    format_date_tz, format_rfc3339_millis, stored_cost_for_mode,
};

pub(super) struct HermesEntry {
    timestamp: TimestampMs,
    timestamp_text: String,
    pub(super) session_id: String,
    model: String,
    provider: String,
    usage: TokenUsageRaw,
    reasoning_tokens: u64,
    message_count: u64,
    cost_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionRowError {
    InvalidIdentity,
    InvalidTimestamp,
}

impl SessionRowError {
    pub(super) const fn kind(self) -> SkipKind {
        match self {
            Self::InvalidIdentity => SkipKind::InvalidData,
            Self::InvalidTimestamp => SkipKind::InvalidTimestamp,
        }
    }

    pub(super) const fn detail(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "Hermes usage row has no valid session identity",
            Self::InvalidTimestamp => "Hermes usage row has an invalid timestamp",
        }
    }
}

pub(super) fn read_session_row(
    statement: &sqlite::Statement<'_>,
) -> Result<Option<HermesEntry>, SessionRowError> {
    let provider_raw = statement.read::<String, _>(2).ok();
    let message_count = read_count(statement, 4);
    let input_tokens = read_count(statement, 5);
    let output_tokens = read_count(statement, 6);
    let cache_read_tokens = read_count(statement, 7);
    let cache_creation_tokens = read_count(statement, 8);
    let reasoning_tokens = read_count(statement, 9);
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
        return Ok(None);
    }

    let session_id = statement
        .read::<String, _>(0)
        .map_err(|_| SessionRowError::InvalidIdentity)?;
    let model = statement
        .read::<String, _>(1)
        .map_err(|_| SessionRowError::InvalidIdentity)?
        .trim()
        .to_string();
    if session_id.is_empty() || model.is_empty() {
        return Err(SessionRowError::InvalidIdentity);
    }
    let started_at = read_f64(statement, 3).ok_or(SessionRowError::InvalidTimestamp)?;
    let timestamp = timestamp_from_number(started_at).ok_or(SessionRowError::InvalidTimestamp)?;

    Ok(Some(HermesEntry {
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
    }))
}

fn read_non_negative_f64(statement: &sqlite::Statement<'_>, index: usize) -> Option<f64> {
    read_f64(statement, index).map(|value| value.max(0.0))
}

fn timestamp_from_number(value: f64) -> Option<TimestampMs> {
    if value <= 0.0 {
        return None;
    }
    let duration = std::time::Duration::try_from_secs_f64(value).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    TimestampMs::from_instant_millis(millis)
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
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> LoadedEntry {
    let (cost, missing_pricing_model) = price_hermes(&entry, mode, pricing);
    let stored_cost = stored_cost_for_mode(entry.cost_usd.filter(|cost| *cost > 0.0), mode);
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
        stored_cost,
        data,
    }
}

fn price_hermes(
    entry: &HermesEntry,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> (f64, Option<String>) {
    let candidates = model_candidates(entry);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let priced = price_entry(
        EntryPricing {
            usage: entry.usage,
            reasoning_tokens: entry.reasoning_tokens,
            residual_tokens: 0,
            candidates: &candidates,
            stored_cost: entry.cost_usd.filter(|cost| *cost > 0.0),
            mode,
            timestamp: Some(entry.timestamp),
        },
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(&entry.model).into_owned()),
    )
}

fn model_candidates(entry: &HermesEntry) -> Vec<(String, MatchKind)> {
    let mut candidates = Vec::new();
    if entry.provider != "hermes" {
        candidates.push((
            format!("{}/{}", entry.provider, entry.model),
            MatchKind::Exact,
        ));
    }
    candidates.push((entry.model.clone(), MatchKind::Fuzzy));
    candidates
}

#[cfg(test)]
mod tests {
    #[test]
    fn zero_rates_stop_candidate_fallback() {
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00Z").unwrap(),
            timestamp_text: String::new(),
            session_id: "s".into(),
            model: "sample".into(),
            provider: "vendor".into(),
            usage: TokenUsageRaw {
                input_tokens: 100,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: None,
        };
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"vendor/sample":{"input_cost_per_token":0,"output_cost_per_token":0},"sample":{"input_cost_per_token":1,"output_cost_per_token":1}}"#);
        assert_eq!(
            price_hermes(&entry, CostMode::Calculate, Some(&pricing)),
            (0.0, None)
        );
    }

    #[test]
    fn uniform_cost_modes_use_recorded_cost_only_in_auto_and_display() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{"sample":{"input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#,
        );
        for (stored, auto, display) in [
            (Some(0.42), 0.42, 0.42),
            (Some(0.0), 0.26, 0.0),
            (None, 0.26, 0.0),
        ] {
            for (mode, expected) in [
                (CostMode::Auto, auto),
                (CostMode::Display, display),
                (CostMode::Calculate, 0.26),
            ] {
                let entry = HermesEntry {
                    timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00Z").unwrap(),
                    timestamp_text: "2026-05-19T00:00:00Z".to_string(),
                    session_id: "s".to_string(),
                    model: "sample".to_string(),
                    provider: "hermes".to_string(),
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        ..TokenUsageRaw::default()
                    },
                    reasoning_tokens: 30,
                    message_count: 1,
                    cost_usd: stored,
                };
                let loaded = to_loaded_entry(entry, &JiffTimeZone::UTC, mode, Some(&pricing));
                assert!(
                    (loaded.cost - expected).abs() < 1e-12,
                    "{mode:?}: {} vs {expected}",
                    loaded.cost
                );
            }
        }
    }

    use super::*;

    #[test]
    fn hermes_start_time_is_always_seconds() {
        assert_eq!(
            timestamp_from_number(10_000_000_000.125)
                .unwrap()
                .as_millis(),
            10_000_000_000_125
        );
        assert_eq!(timestamp_from_number(1_000_000_000_001.0), None);
        assert_eq!(timestamp_from_number(f64::MAX), None);
    }

    #[test]
    fn a_null_actual_cost_leaves_the_estimated_cost() {
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection
            .prepare("SELECT 's1', 'gpt-5', 'openai', 1767312000, 1, 10, 2, 0, 0, 0, 0.25, NULL")
            .unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);

        let entry = read_session_row(&statement).unwrap().unwrap();

        assert_eq!(entry.cost_usd, Some(0.25));
    }

    #[test]
    fn an_infinite_recorded_cost_is_no_cost_instead_of_a_clamped_integer() {
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection
            .prepare("SELECT 's1', 'gpt-5', 'openai', 1767312000, 1, 10, 2, 0, 0, 0, 9e999, 9e999")
            .unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);

        let entry = read_session_row(&statement).unwrap().unwrap();

        assert_eq!(entry.cost_usd, None);
        assert_eq!(entry.usage.input_tokens, 10);
    }

    #[test]
    fn calculates_cost_for_hermes_frontier_models_from_embedded_pricing() {
        ccusage_test_support::install_pricing_snapshots();
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
                price_hermes(&entry, CostMode::Auto, Some(&pricing)).0 > 0.0,
                "{model} should resolve to embedded pricing"
            );
        }
    }

    #[test]
    fn recorded_zero_cost_falls_back_to_pricing_calculation() {
        ccusage_test_support::install_pricing_snapshots();
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
            price_hermes(&entry, CostMode::Auto, Some(&pricing)).0 > 0.0,
            "subscription-included sessions with token usage should still be priced"
        );
    }

    #[test]
    fn recorded_positive_cost_is_trusted() {
        ccusage_test_support::install_pricing_snapshots();
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

        assert_eq!(price_hermes(&entry, CostMode::Auto, Some(&pricing)).0, 0.42);
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

        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "openai/gpt-5.5": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002
                }
            }"#,
        );

        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing.resolve(&[(name.as_str(), *kind)]).is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["openai/gpt-5.5".to_string(), "gpt-5.5".to_string()]
        );
    }

    #[test]
    fn uses_raw_model_for_timestamped_pricing_when_provider_match_is_fuzzy() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),
            timestamp_text: "2026-08-17T01:00:00.000Z".to_string(),
            session_id: "session-deepseek".to_string(),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: None,
        };

        assert!((price_hermes(&entry, CostMode::Auto, Some(&pricing)).0 - 0.44).abs() < 1e-12);
        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing.resolve(&[(name.as_str(), *kind)]).is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["deepseek-v4-flash"]
        );
    }

    #[test]
    fn uses_exact_provider_pricing_from_embedded_fallback_before_timestamped_raw_model() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),
            timestamp_text: "2026-08-17T01:00:00.000Z".to_string(),
            session_id: "session-deepseek-fallback".to_string(),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: None,
        };

        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing.resolve(&[(name.as_str(), *kind)]).is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![
                "deepseek/deepseek-v4-flash".to_string(),
                "deepseek-v4-flash".to_string()
            ]
        );
        let provider_cost = pricing
            .find_exact_with_fallback("deepseek/deepseek-v4-flash")
            .unwrap()
            .input
            * 1_000_000.0;
        let raw_model_cost = ccusage_core::calculate_cost_for_usage_at(
            Some("deepseek-v4-flash"),
            entry.usage,
            None,
            Some(entry.timestamp),
            CostMode::Calculate,
            Some(&pricing),
        );
        assert!((provider_cost - raw_model_cost).abs() > 1e-12);
        assert!(
            (price_hermes(&entry, CostMode::Auto, Some(&pricing)).0 - provider_cost).abs() < 1e-12
        );
    }

    #[test]
    fn accepts_normalized_provider_pricing_and_excludes_longer_fuzzy_matches() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek/deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        let entry = HermesEntry {
            timestamp: crate::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),
            timestamp_text: "2026-08-17T01:00:00.000Z".to_string(),
            session_id: "session-deepseek-fuzzy-fallback".to_string(),
            model: "deepseek.v4.flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
            message_count: 1,
            cost_usd: None,
        };

        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing.resolve(&[(name.as_str(), *kind)]).is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![
                "deepseek/deepseek.v4.flash".to_string(),
                "deepseek.v4.flash".to_string()
            ]
        );
        assert!((price_hermes(&entry, CostMode::Auto, Some(&pricing)).0 - 0.14).abs() < 1e-12);

        let longer_entry = HermesEntry {
            model: "deepseek.v4.flash-preview".to_string(),
            session_id: "session-deepseek-fuzzy-fallback".to_string(),
            ..entry
        };
        assert_eq!(
            model_candidates(&longer_entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing.resolve(&[(name.as_str(), *kind)]).is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["deepseek.v4.flash-preview".to_string()]
        );
    }
}

#[cfg(test)]
mod resolver_differential {
    use super::*;
    use ccusage_core::pricing::MatchKind;
    #[test]
    fn ordered_resolver_preserves_legacy_names_and_rates() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::load_embedded();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002},"vendor/sample":{"input_cost_per_token":0.000003,"output_cost_per_token":0.000004}}"#);
        for model in [
            "claude-sonnet-4-20250514",
            "claude-sonnet-4",
            "claude-opus-5",
            "anthropic/claude-sonnet-4",
            "gpt-5",
            "gpt-5.5",
            "openai/gpt-5",
            "gemini-2.5-pro",
            "gemini-3.8-flash-high",
            "gemini-3.7-flash-medium",
            "gemini-3.6-flash-low",
            "gemini-3.8-flash",
            "qwen3-coder",
            "qwen3-coder-plus",
            "qwen3-coder-flash",
            "glm-5.3",
            "GLM-5.3",
            "glm-4.5",
            "deepseek-v4-flash",
            "deepseek/deepseek-v4-flash",
            "grok-4.3",
            "sample",
            "vendor/sample",
            "unpriced-model-xyz",
        ] {
            for provider in [
                "anthropic",
                "openai",
                "google",
                "qwen",
                "alibaba",
                "zai",
                "openrouter",
                "vendor",
                "unknown",
            ] {
                let qualified = format!("{provider}/{model}");
                let old = pricing
                    .find_exact_with_fallback(&qualified)
                    .and_then(|_| {
                        pricing
                            .find(&qualified)
                            .map(|rates| (qualified.as_str(), rates))
                    })
                    .or_else(|| pricing.find(model).map(|rates| (model, rates)));
                let new =
                    pricing.resolve(&[(&qualified, MatchKind::Exact), (model, MatchKind::Fuzzy)]);
                assert!(
                    old.is_none_or(|(_, rates)| ccusage_core::calculate_cost_from_pricing(
                        TokenUsageRaw {
                            input_tokens: 1,
                            output_tokens: 1,
                            cache_creation_input_tokens: 1,
                            cache_read_input_tokens: 1,
                            ..TokenUsageRaw::default()
                        },
                        rates
                    ) > 0.0),
                    "{model} / {provider} must exercise the legacy positive-cost path"
                );
                assert_eq!(
                    format!("{old:?}"),
                    format!("{new:?}"),
                    "{model} / {provider}"
                );
            }
        }
    }
}
