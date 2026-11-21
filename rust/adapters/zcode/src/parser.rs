use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::MatchKind,
};
use std::{collections::BTreeMap, sync::Arc};

use ccusage_adapter_common::billing::price_entry_for_provider;
use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw,
    cli::{CostMode, PricingOverride},
    date_at, total_usage_tokens,
};

pub(super) struct ZcodeUsageRow {
    pub(super) id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) started_at: Option<i64>,
    pub(super) model_id: Option<String>,
    pub(super) provider_id: Option<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) computed_total_tokens: u64,
    pub(super) directory: Option<String>,
    pub(super) version: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum RowRejection {
    InvalidIdentity,
    InvalidTimestamp,
}

pub(super) fn row_to_entry(
    row: ZcodeUsageRow,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    pricing_overrides: &BTreeMap<String, PricingOverride>,
) -> std::result::Result<Option<(String, LoadedEntry)>, RowRejection> {
    let cache_read = row.cache_read_input_tokens.min(row.input_tokens);
    let cache_creation = row
        .cache_creation_input_tokens
        .min(row.input_tokens.saturating_sub(cache_read));
    let usage = TokenUsageRaw {
        input_tokens: row
            .input_tokens
            .saturating_sub(cache_read)
            .saturating_sub(cache_creation),
        output_tokens: row.output_tokens,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    };
    let additional =
        ccusage_core::AdditionalTokens::from_total(usage, 0, row.computed_total_tokens);
    if total_usage_tokens(usage) == 0 && additional.residual == 0 {
        return Ok(None);
    }
    let (Some(id), Some(session_id), Some(raw_model)) = (
        non_blank(row.id),
        non_blank(row.session_id),
        non_blank(row.model_id),
    ) else {
        return Err(RowRejection::InvalidIdentity);
    };
    let Some(started_at) = row.started_at.filter(|started_at| *started_at > 0) else {
        return Err(RowRejection::InvalidTimestamp);
    };

    let raw_model = raw_model.trim().to_string();
    let candidates =
        ordered_pricing_candidates(&raw_model, row.provider_id.as_deref(), pricing_overrides);
    let timestamp = TimestampMs::from_millis(started_at);
    let project_path = row
        .directory
        .filter(|directory| !directory.trim().is_empty())
        .unwrap_or_else(|| "ZCode".to_string());
    // NOTE(2026-08-31, #1675): Z.ai bills new content at the standard input rate even when the ledger labels it cache creation. Both buckets therefore use the input rate for pricing.
    let is_zai = is_zai_provider(row.provider_id.as_deref())
        || (row.provider_id.is_none() && is_zai_model(&raw_model.to_ascii_lowercase()));
    let cost_usage = if is_zai {
        TokenUsageRaw {
            input_tokens: usage
                .input_tokens
                .saturating_add(usage.cache_creation_input_tokens),
            cache_creation_input_tokens: 0,
            ..usage
        }
    } else {
        usage
    };
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| {
            let kind = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                MatchKind::Exact
            } else {
                MatchKind::Fuzzy
            };
            (name.as_str(), kind)
        })
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Request,
        usage: cost_usage,
        reasoning_tokens: 0,
        residual_tokens: additional.residual,
        reported_model: Some(&raw_model),
        candidates: &candidates,
        stored_cost: None,
        timestamp: Some(timestamp),
    };
    let priced = price_entry_for_provider(entry_pricing, row.provider_id.as_deref(), pricing);
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model =
        (mode != CostMode::Display && priced.matched.is_none() && total_usage_tokens(usage) > 0)
            .then(|| ccusage_core::model_aliases::resolve_model_name(&raw_model).into_owned());
    let (usage, extra_total_tokens) = additional.for_report(usage);

    Ok(Some((
        id.clone(),
        LoadedEntry {
            usage,
            message_id: Some(id),
            version: row.version,
            recorded_cost: None,
            date: date_at(timestamp, tz),
            timestamp,
            project: Arc::from("zcode"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost,
            extra_total_tokens,
            credits: None,
            message_count: None,
            model: Some(raw_model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            estimated_pricing,
            stored_cost: None,
        },
    )))
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn pricing_candidates(raw_model: &str, provider_id: Option<&str>) -> Vec<String> {
    let raw_model = raw_model.trim();
    let lower_model = raw_model.to_ascii_lowercase();
    let mut candidates = vec![raw_model.to_string(), lower_model.clone()];
    let is_implicit_zai = provider_id.is_none() && is_zai_model(&lower_model);
    if is_zai_provider(provider_id) || is_implicit_zai {
        candidates.push(format!("zai/{raw_model}"));
        candidates.push(format!("zai/{lower_model}"));
    }
    candidates.dedup();
    candidates
}

fn ordered_pricing_candidates(
    raw_model: &str,
    provider_id: Option<&str>,
    pricing_overrides: &BTreeMap<String, PricingOverride>,
) -> Vec<String> {
    let candidates = pricing_candidates(raw_model, provider_id);
    let mut ordered: Vec<_> = candidates
        .iter()
        .filter(|name| pricing_overrides.contains_key(*name))
        .cloned()
        .collect();
    if provider_id.is_none_or(|provider| is_zai_provider(Some(provider))) {
        ordered.extend(
            candidates
                .iter()
                .filter(|name| name.starts_with("zai/"))
                .cloned(),
        );
        ordered.extend(
            candidates
                .iter()
                .filter(|name| !name.starts_with("zai/"))
                .cloned(),
        );
    }
    ordered
}

fn is_zai_provider(provider_id: Option<&str>) -> bool {
    provider_id.is_some_and(|provider| {
        let normalized = provider.trim().to_ascii_lowercase();
        let normalized = normalized.as_str();
        matches!(
            normalized,
            "zai"
                | "z.ai"
                | "zai-coding-plan"
                | "builtin:zai-coding-plan"
                | "builtin:bigmodel-coding-plan"
        ) || normalized.starts_with("account:zai-")
            || normalized.starts_with("builtin:zai-")
    })
}

fn is_zai_model(model: &str) -> bool {
    model.starts_with("glm-") || model.starts_with("glm/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CostMode;

    fn row() -> ZcodeUsageRow {
        ZcodeUsageRow {
            id: Some("usage-1".to_string()),
            session_id: Some("session-1".to_string()),
            started_at: Some(1_786_909_042_666),
            model_id: Some("GLM-5.3".to_string()),
            provider_id: Some("builtin:zai-coding-plan".to_string()),
            input_tokens: 100,
            output_tokens: 10,
            cache_creation_input_tokens: 15,
            cache_read_input_tokens: 25,
            computed_total_tokens: 110,
            directory: Some("/workspace/project".to_string()),
            version: Some("0.16.3".to_string()),
        }
    }

    #[test]
    fn normalizes_both_inclusive_cache_buckets() {
        ccusage_test_support::install_pricing_snapshots();
        let (id, entry) = row_to_entry(
            row(),
            &JiffTimeZone::UTC,
            CostMode::Display,
            ccusage_core::cost::PriceTable::Display,
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage");
        assert_eq!(id, "usage-1");
        assert_eq!(entry.message_id.as_deref(), Some(id.as_str()));
        let usage = entry.usage;

        assert_eq!(usage.input_tokens, 60);
        assert_eq!(usage.cache_creation_input_tokens, 15);
        assert_eq!(usage.cache_read_input_tokens, 25);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(total_usage_tokens(usage), 110);
        assert_eq!(entry.model.as_deref(), Some("GLM-5.3"));
        assert_eq!(entry.version.as_deref(), Some("0.16.3"));
        assert_eq!(
            entry.timestamp,
            ccusage_core::parse_ts_timestamp("2026-08-16T19:37:22.666Z").unwrap()
        );
    }

    #[test]
    fn prices_current_zai_account_and_builtin_plan_ids() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for provider in [
            "account:zai-individual-coding-plan",
            "account:zai-start-plan",
            "builtin:zai-start-plan",
        ] {
            for model in ["GLM-5.3", "GLM-5.3-Flash"] {
                let mut row = row();
                row.provider_id = Some(provider.to_string());
                row.model_id = Some(model.to_string());
                let entry = row_to_entry(
                    row,
                    &JiffTimeZone::UTC,
                    CostMode::Calculate,
                    ccusage_core::cost::PriceTable::Calculate(&pricing),
                    &BTreeMap::new(),
                )
                .unwrap()
                .expect("test row has usage")
                .1;
                assert!(entry.cost > 0.0, "{provider} {model}");
                assert!(entry.missing_pricing_model.is_none(), "{provider} {model}");
            }
        }
    }

    #[test]
    fn a_mixed_case_model_id_matches_fuzzily_only_through_its_lowercase_form() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
            "zai/glm-5.2": {"litellm_provider":"zai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "zai/GLM-5.2": {"litellm_provider":"host","input_cost_per_token":9e-6,"output_cost_per_token":9e-6}
        }"#,
        );
        let cost = |model: &str| {
            let mut row = row();
            row.provider_id = Some("zai".to_string());
            row.model_id = Some(model.to_string());
            row_to_entry(
                row,
                &JiffTimeZone::UTC,
                CostMode::Calculate,
                ccusage_core::cost::PriceTable::Calculate(&pricing),
                &BTreeMap::new(),
            )
            .unwrap()
            .expect("test row has usage")
            .1
            .cost
        };
        assert!(cost("glm-5.2-20261101") > 0.0);
        assert_eq!(cost("GLM-5.2-20261101"), cost("glm-5.2-20261101"));
    }

    #[test]
    fn prices_zai_models_but_not_unknown_custom_providers() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let priced = row_to_entry(
            row(),
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage")
        .1;
        assert!(priced.cost > 0.0);
        assert!(priced.missing_pricing_model.is_none());

        let mut custom = row();
        custom.provider_id = Some("847d13c9-0568-4f2f-818e-8bd498e5d920".to_string());
        custom.model_id = Some("deepseek-v4-flash".to_string());
        let custom = row_to_entry(
            custom,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage")
        .1;
        assert_eq!(custom.cost, 0.0);
        assert_eq!(
            custom.missing_pricing_model.as_deref(),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn prices_legacy_bigmodel_provider_with_provider_qualified_glm_pricing() {
        ccusage_test_support::install_pricing_snapshots();
        let mut row = row();
        row.provider_id = Some("builtin:bigmodel-coding-plan".to_string());

        let entry = row_to_entry(
            row,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&ccusage_core::PricingMap::load_embedded()),
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage")
        .1;

        assert_eq!(entry.cost, 0.00015549999999999999);
        assert!(entry.missing_pricing_model.is_none());
    }

    #[test]
    fn prices_zai_cache_creation_at_standard_input_rate_for_glm_52_and_53() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let priced = |model: &str| {
            let mut row = row();
            row.model_id = Some(model.to_string());
            row_to_entry(
                row,
                &JiffTimeZone::UTC,
                CostMode::Calculate,
                ccusage_core::cost::PriceTable::Calculate(&pricing),
                &BTreeMap::new(),
            )
            .unwrap()
            .expect("test row has usage")
            .1
        };

        let glm_52 = priced("GLM-5.2");
        assert_eq!(glm_52.cost, 0.00015549999999999999);
        assert_eq!(glm_52.usage.input_tokens, 60);
        assert_eq!(glm_52.usage.cache_creation_input_tokens, 15);

        let glm_53 = priced("GLM-5.3");
        assert_eq!(glm_53.cost, 0.00015549999999999999);
        assert_eq!(glm_53.usage.input_tokens, 60);
        assert_eq!(glm_53.usage.cache_creation_input_tokens, 15);
    }

    #[test]
    fn honors_raw_model_overrides_for_custom_providers() {
        ccusage_test_support::install_pricing_snapshots();
        let mut row = row();
        row.provider_id = Some("847d13c9-0568-4f2f-818e-8bd498e5d920".to_string());
        row.model_id = Some("deepseek-v4-flash".to_string());
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "deepseek-v4-flash".to_string(),
            PricingOverride {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                cache_read_input_token_cost: Some(3e-6),
                ..Default::default()
            },
        );
        let pricing = ccusage_core::PricingMap::load_with_overrides(true, false, overrides.iter());

        let entry = row_to_entry(
            row,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
            &overrides,
        )
        .unwrap()
        .expect("test row has usage")
        .1;

        assert!(entry.cost > 0.0);
        assert!(entry.missing_pricing_model.is_none());
    }

    #[test]
    fn routes_unaccounted_total_tokens_to_extra_tokens() {
        ccusage_test_support::install_pricing_snapshots();
        let mut row = row();
        row.computed_total_tokens = 123;
        let entry = row_to_entry(
            row,
            &JiffTimeZone::UTC,
            CostMode::Display,
            ccusage_core::cost::PriceTable::Display,
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage")
        .1;
        assert_eq!(entry.extra_total_tokens, 13);
        assert_eq!(entry.usage.output_tokens, 10);
    }

    #[test]
    fn rejects_missing_identity_and_invalid_timestamp_separately() {
        let reject = |row: ZcodeUsageRow| {
            row_to_entry(
                row,
                &JiffTimeZone::UTC,
                CostMode::Display,
                ccusage_core::cost::PriceTable::Display,
                &BTreeMap::new(),
            )
            .err()
        };
        let blank_id = ZcodeUsageRow {
            id: Some(" ".to_string()),
            ..row()
        };
        let null_model = ZcodeUsageRow {
            model_id: None,
            ..row()
        };
        let zero_timestamp = ZcodeUsageRow {
            started_at: Some(0),
            ..row()
        };

        assert_eq!(reject(blank_id), Some(RowRejection::InvalidIdentity));
        assert_eq!(reject(null_model), Some(RowRejection::InvalidIdentity));
        assert_eq!(reject(zero_timestamp), Some(RowRejection::InvalidTimestamp));
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            ordered_pricing_candidates(" GLM-4.7 ", None, &BTreeMap::new()),
            ["zai/GLM-4.7", "zai/glm-4.7", "GLM-4.7", "glm-4.7"]
        );
    }

    #[test]
    fn prices_a_foreign_provider_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let row = ZcodeUsageRow {
            id: Some("usage-1".to_string()),
            session_id: Some("session-1".to_string()),
            started_at: Some(
                ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z")
                    .unwrap()
                    .as_millis(),
            ),
            model_id: Some("deepseek-v4-flash".to_string()),
            provider_id: Some("zai".to_string()),
            input_tokens: 1000,
            output_tokens: 1000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            computed_total_tokens: 2000,
            directory: Some("/workspace/project".to_string()),
            version: Some("0.16.3".to_string()),
        };
        let entry = row_to_entry(
            row,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
            &BTreeMap::new(),
        )
        .unwrap()
        .expect("test row has usage")
        .1;
        assert!((entry.cost - 0.0015).abs() < 1e-12, "{}", entry.cost);
        assert_eq!(entry.missing_pricing_model, None);
    }
}
