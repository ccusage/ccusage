use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::MatchKind,
};
use std::sync::Arc;

use ccusage_adapter_common::{
    billing::price_entry_for_provider,
    sqlite::{read_count, read_f64},
};
use ccusage_core::SkipKind;
use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, stored_cost_for_mode,
};

pub(super) struct HermesEntry {
    timestamp: TimestampMs,

    pub(super) session_id: String,
    model: String,
    provider: String,
    usage: TokenUsageRaw,
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
    // NOTE(2026-05-19, #1026): Hermes stores session costs in state.db, with separate actual and estimated columns. A missing actual cost leaves the estimate usable, while a recorded zero is meaningful.
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
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let (cost, missing_pricing_model) = price_hermes(&entry, pricing);
    let stored_cost = stored_cost_for_mode(ccusage_core::cost::recorded_cost(entry.cost_usd), mode);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("hermes"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Hermes"),
        cost,
        credits: None,
        extra_total_tokens: 0,
        message_count: Some(entry.message_count),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        stored_cost,
        usage: entry.usage,
        message_id: Some(format!("hermes:{}", entry.session_id)),
        version: None,
        recorded_cost: entry.cost_usd,
    }
}

fn price_hermes(entry: &HermesEntry, pricing: PriceTable<'_>) -> (f64, Option<String>) {
    let candidates = model_candidates(entry);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    // NOTE(2026-11-20, #2364): Hermes reads `reasoning_tokens` from the output token details and bills only `output_tokens` (NousResearch/hermes-agent `agent/usage_pricing.py`). Reasoning is a subset of output, so it is neither priced nor totaled again.
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Aggregate,
        usage: entry.usage,
        reasoning_tokens: 0,
        residual_tokens: 0,
        reported_model: Some(&entry.model),
        candidates: &candidates,
        stored_cost: ccusage_core::cost::recorded_cost(entry.cost_usd),
        timestamp: Some(entry.timestamp),
    };
    let provider = (entry.provider != "hermes").then_some(entry.provider.as_str());
    let priced = price_entry_for_provider(entry_pricing, provider, pricing);
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
        ccusage_test_support::install_pricing_snapshots();
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00Z").unwrap(),

            session_id: "s".into(),
            model: "sample".into(),
            provider: "zai".into(),
            usage: TokenUsageRaw {
                input_tokens: 100,
                ..TokenUsageRaw::default()
            },
            message_count: 1,
            cost_usd: None,
        };
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"zai/sample":{"litellm_provider":"zai","input_cost_per_token":0,"output_cost_per_token":0},"sample":{"input_cost_per_token":1,"output_cost_per_token":1}}"#);
        assert_eq!(
            price_hermes(&entry, PriceTable::Calculate(&pricing)),
            (0.0, None)
        );
    }

    #[test]
    fn uniform_cost_modes_use_recorded_cost_only_in_auto_and_display() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"sample":{"input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#,
        );
        for (stored, auto, display) in [
            (Some(0.42), 0.42, 0.42),
            (Some(0.0), 0.2, 0.0),
            (None, 0.2, 0.0),
        ] {
            for (mode, expected) in [
                (CostMode::Auto, auto),
                (CostMode::Display, display),
                (CostMode::Calculate, 0.2),
            ] {
                let entry = HermesEntry {
                    timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00Z").unwrap(),

                    session_id: "s".to_string(),
                    model: "sample".to_string(),
                    provider: "hermes".to_string(),
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        ..TokenUsageRaw::default()
                    },
                    message_count: 1,
                    cost_usd: stored,
                };
                let loaded = to_loaded_entry(
                    entry,
                    &JiffTimeZone::UTC,
                    mode,
                    PriceTable::for_map(mode, &pricing),
                );
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
    fn a_session_total_is_priced_at_the_base_tier_not_the_long_context_tier() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"gemini-2.5-pro":{"litellm_provider":"gemini","input_cost_per_token":1.25e-6,"output_cost_per_token":1e-5,"input_cost_per_token_above_200k_tokens":2.5e-6,"output_cost_per_token_above_200k_tokens":1.5e-5}}"#,
        );
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection
            .prepare("SELECT 's1', 'gemini-2.5-pro', 'google', 1767312000, 1, 5000000, 0, 0, 0, 0, NULL, NULL")
            .unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);
        let entry = read_session_row(&statement).unwrap().unwrap();

        let loaded = to_loaded_entry(
            entry,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
        );

        assert!((loaded.cost - 6.25).abs() < 1e-9, "{}", loaded.cost);
    }

    #[test]
    fn reasoning_tokens_are_part_of_output_and_not_counted_again() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"gpt-5":{"litellm_provider":"openai","input_cost_per_token":1.25e-6,"output_cost_per_token":1e-5}}"#,
        );
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection
            .prepare("SELECT 's1', 'gpt-5', 'openai', 1767312000, 1, 1000000, 500000, 0, 0, 300000, NULL, NULL")
            .unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);
        let entry = read_session_row(&statement).unwrap().unwrap();

        let loaded = to_loaded_entry(
            entry,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
        );

        assert_eq!(
            ccusage_core::total_usage_tokens(loaded.usage) + loaded.extra_total_tokens,
            1_500_000
        );
        assert!((loaded.cost - 6.25).abs() < 1e-9, "{}", loaded.cost);
    }

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
        let pricing = ccusage_core::PricingMap::load_embedded();
        for model in ["gpt-5.5", "grok-4.3"] {
            let entry = HermesEntry {
                timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),

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
                message_count: 1,
                cost_usd: None,
            };

            assert!(
                price_hermes(&entry, PriceTable::Auto(&pricing)).0 > 0.0,
                "{model} should resolve to embedded pricing"
            );
        }
    }

    #[test]
    fn recorded_zero_cost_falls_back_to_pricing_calculation() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),

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
            message_count: 72,
            cost_usd: Some(0.0),
        };

        assert!(
            price_hermes(&entry, PriceTable::Auto(&pricing)).0 > 0.0,
            "subscription-included sessions with token usage should still be priced"
        );
    }

    #[test]
    fn recorded_positive_cost_is_trusted() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),

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
            message_count: 1,
            cost_usd: Some(0.42),
        };

        assert_eq!(price_hermes(&entry, PriceTable::Auto(&pricing)).0, 0.42);
    }

    #[test]
    fn tries_provider_qualified_model_candidate_first() {
        ccusage_test_support::install_pricing_snapshots();
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),

            session_id: "session-provider".to_string(),
            model: "gpt-5.5".to_string(),
            provider: "openai".to_string(),
            usage: TokenUsageRaw::default(),
            message_count: 1,
            cost_usd: None,
        };

        let mut pricing = ccusage_core::PricingMap::default();
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
                    || pricing
                        .resolve_at(None, &[(name.as_str(), *kind)], None)
                        .is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["openai/gpt-5.5".to_string(), "gpt-5.5".to_string()]
        );
    }

    #[test]
    fn scheduled_price_is_the_same_for_direct_provider_and_bare_ids() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek/deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),

            session_id: "session-deepseek".to_string(),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            message_count: 1,
            cost_usd: None,
        };

        assert!((price_hermes(&entry, PriceTable::Auto(&pricing)).0 - 0.44).abs() < 1e-12);
        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing
                        .resolve_at(None, &[(name.as_str(), *kind)], None)
                        .is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["deepseek/deepseek-v4-flash", "deepseek-v4-flash"]
        );
    }

    #[test]
    fn applies_the_schedule_to_an_embedded_direct_provider_row() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),

            session_id: "session-deepseek-fallback".to_string(),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            message_count: 1,
            cost_usd: None,
        };

        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing
                        .resolve_at(None, &[(name.as_str(), *kind)], None)
                        .is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![
                "deepseek/deepseek-v4-flash".to_string(),
                "deepseek-v4-flash".to_string()
            ]
        );
        let raw_model_cost = ccusage_core::cost::price_resolved_usage(
            "deepseek-v4-flash",
            entry.usage,
            entry.timestamp,
            pricing.find_handle("deepseek-v4-flash").as_ref(),
        )
        .0;
        assert!((raw_model_cost - 0.44).abs() < 1e-12);
        assert!(
            (price_hermes(&entry, PriceTable::Auto(&pricing)).0 - raw_model_cost).abs() < 1e-12
        );
    }

    #[test]
    fn accepts_normalized_provider_pricing_and_excludes_longer_fuzzy_matches() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek/deepseek-v4-flash": {
                    "litellm_provider": "deepseek",
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap(),

            session_id: "session-deepseek-fuzzy-fallback".to_string(),
            model: "deepseek.v4.flash".to_string(),
            provider: "deepseek".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            message_count: 1,
            cost_usd: None,
        };

        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing
                        .resolve_at(None, &[(name.as_str(), *kind)], None)
                        .is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![
                "deepseek/deepseek.v4.flash".to_string(),
                "deepseek.v4.flash".to_string()
            ]
        );
        assert!((price_hermes(&entry, PriceTable::Auto(&pricing)).0 - 0.44).abs() < 1e-12);

        let longer_entry = HermesEntry {
            model: "deepseek.v4.flash-preview".to_string(),
            session_id: "session-deepseek-fuzzy-fallback".to_string(),
            ..entry
        };
        assert_eq!(
            model_candidates(&longer_entry)
                .into_iter()
                .filter(|(name, kind)| *kind == MatchKind::Fuzzy
                    || pricing
                        .resolve_at(None, &[(name.as_str(), *kind)], None)
                        .is_some())
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["deepseek.v4.flash-preview".to_string()]
        );
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn prices_a_foreign_router_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (provider, expected) in [("openrouter", 0.0015), ("deepseek", 0.00075)] {
            let entry = HermesEntry {
                timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

                session_id: "session-deepseek".to_string(),
                model: "deepseek-v4-flash".to_string(),
                provider: provider.to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..TokenUsageRaw::default()
                },
                message_count: 1,
                cost_usd: None,
            };

            let (cost, missing) = price_hermes(&entry, PriceTable::Calculate(&pricing));
            assert!((cost - expected).abs() < 1e-12, "{provider}: {cost}");
            assert_eq!(missing, None, "{provider}");
        }
    }

    #[test]
    fn override_on_the_reported_id_beats_provider_pricing() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (provider, reported, configured) in [
            ("deepseek", "deepseek.v4.flash", "deepseek-v4-flash"),
            ("zai", "glm-4.7", "glm-4.7"),
        ] {
            let id = configured.to_string();
            let rates = ccusage_core::cli::PricingOverride {
                input_cost_per_token: Some(50e-6),
                output_cost_per_token: Some(50e-6),
                ..Default::default()
            };
            let qualified_id = format!("{provider}/{configured}");
            let provider_override = ccusage_core::cli::PricingOverride {
                input_cost_per_token: Some(99e-6),
                output_cost_per_token: Some(99e-6),
                ..Default::default()
            };
            let pricing =
                pricing.with_user_overrides([(&id, &rates), (&qualified_id, &provider_override)]);
            let entry = HermesEntry {
                timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

                session_id: "s".into(),
                model: reported.into(),
                provider: provider.into(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Default::default()
                },
                message_count: 1,
                cost_usd: None,
            };
            assert_eq!(
                price_hermes(&entry, PriceTable::Auto(&pricing)),
                (0.1, None)
            );
        }
    }

    #[test]
    fn a_configured_alias_prices_a_provider_request_at_the_alias_target() {
        ccusage_test_support::install_pricing_snapshots();
        let _aliases = ccusage_core::model_aliases::set_model_aliases_for_tests([(
            "alias-source-model",
            "alias-target-model",
        )]);
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "openai/alias-source-model": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
                "alias-target-model": {"input_cost_per_token": 7e-6, "output_cost_per_token": 9e-6}
            }"#,
        );
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),
            session_id: "s".into(),
            model: "alias-source-model".into(),
            provider: "openai".into(),
            usage: TokenUsageRaw {
                input_tokens: 1_000,
                output_tokens: 1_000,
                ..Default::default()
            },
            message_count: 1,
            cost_usd: None,
        };
        let (cost, missing) = price_hermes(&entry, PriceTable::Calculate(&pricing));
        assert!((cost - 0.016).abs() < 1e-12, "{cost}");
        assert_eq!(missing, None);
    }

    #[test]
    fn a_provider_qualified_override_beats_a_configured_alias() {
        ccusage_test_support::install_pricing_snapshots();
        let _aliases = ccusage_core::model_aliases::set_model_aliases_for_tests([(
            "alias-source-model",
            "alias-target-model",
        )]);
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"alias-target-model": {"input_cost_per_token": 7e-6, "output_cost_per_token": 9e-6}}"#,
        );
        let id = "openai/alias-source-model".to_owned();
        let rates = ccusage_core::cli::PricingOverride {
            input_cost_per_token: Some(1e-6),
            output_cost_per_token: Some(2e-6),
            ..Default::default()
        };
        let pricing = pricing.with_user_overrides([(&id, &rates)]);
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),
            session_id: "s".into(),
            model: "alias-source-model".into(),
            provider: "openai".into(),
            usage: TokenUsageRaw {
                input_tokens: 1_000,
                output_tokens: 1_000,
                ..Default::default()
            },
            message_count: 1,
            cost_usd: None,
        };
        let (cost, missing) = price_hermes(&entry, PriceTable::Calculate(&pricing));
        assert!((cost - 0.003).abs() < 1e-12, "{cost}");
        assert_eq!(missing, None);
    }

    #[test]
    fn a_bare_override_prices_a_provider_request_above_the_context_threshold() {
        ccusage_test_support::install_pricing_snapshots();
        let id = "glm-4.7".to_owned();
        let rates = ccusage_core::cli::PricingOverride {
            input_cost_per_token: Some(50e-6),
            output_cost_per_token: Some(50e-6),
            ..Default::default()
        };
        let pricing =
            ccusage_core::PricingMap::load_embedded().with_user_overrides([(&id, &rates)]);
        let entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),
            session_id: "s".into(),
            model: "glm-4.7".into(),
            provider: "zai".into(),
            usage: TokenUsageRaw {
                input_tokens: 150_000,
                output_tokens: 150_000,
                ..Default::default()
            },
            message_count: 1,
            cost_usd: None,
        };
        let (cost, missing) = price_hermes(&entry, PriceTable::Calculate(&pricing));
        assert!((cost - 15.0).abs() < 1e-9, "{cost}");
        assert_eq!(missing, None);
    }

    #[test]
    fn adapter_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let model = "deepseek.v4.flash";
        let mut entry = HermesEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

            session_id: "s".into(),
            model: model.into(),
            provider: normalize_provider(Some("deepseek"), model),
            usage: TokenUsageRaw {
                input_tokens: 1000,
                output_tokens: 1000,
                ..Default::default()
            },
            message_count: 1,
            cost_usd: None,
        };
        assert_eq!(
            model_candidates(&entry),
            vec![
                ("deepseek/deepseek.v4.flash".into(), MatchKind::Exact),
                ("deepseek.v4.flash".into(), MatchKind::Fuzzy),
            ]
        );
        assert!((price_hermes(&entry, PriceTable::Calculate(&pricing)).0 - 0.00075).abs() < 1e-12);
        entry.model = "deepseek.v4.pro".into();
        assert_eq!(
            model_candidates(&entry),
            vec![
                ("deepseek/deepseek.v4.pro".into(), MatchKind::Exact),
                ("deepseek.v4.pro".into(), MatchKind::Fuzzy),
            ]
        );
        assert!((price_hermes(&entry, PriceTable::Calculate(&pricing)).0 - 0.00264).abs() < 1e-12);
    }

    #[test]
    fn prices_same_identity_and_overrides_in_provider_candidate_order() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let id = "glm-4.7".to_owned();
        let patch = ccusage_core::cli::PricingOverride {
            input_cost_per_token: Some(50e-6),
            output_cost_per_token: Some(50e-6),
            ..Default::default()
        };
        let with_override = pricing.with_user_overrides([(&id, &patch)]);
        for (case, model, provider, map, expected) in [
            (
                "23_direct_v3_2",
                "deepseek-v3.2",
                "deepseek",
                &pricing,
                0.0007,
            ),
            (
                "34_scheduled_flash",
                "deepseek-v4-flash",
                "deepseek",
                &pricing,
                0.00075,
            ),
            ("46_glm_override", "glm-4.7", "zai", &with_override, 0.10),
            (
                "113_paid_listing_before_free_suffix",
                "novita/glm-4.7-flash",
                "openrouter",
                &pricing,
                0.0004747,
            ),
            (
                "55_router_listing_behind_the_path",
                "openrouter/z-ai/glm-4.7-flash",
                "openrouter",
                &pricing,
                0.00066125,
            ),
            (
                "54_free_identity",
                "glm-4.7-flash",
                "openrouter",
                &pricing,
                0.0,
            ),
        ] {
            let entry = HermesEntry {
                timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

                session_id: "s".into(),
                model: model.into(),
                provider: normalize_provider(Some(provider), model),
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Default::default()
                },
                message_count: 1,
                cost_usd: None,
            };
            let (cost, missing) = price_hermes(&entry, PriceTable::Calculate(map));
            assert!((cost - expected).abs() < 1e-12, "{case}: {cost}");
            assert_eq!(missing, None, "{case}");
        }
    }
}
