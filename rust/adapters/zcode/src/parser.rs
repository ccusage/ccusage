use std::{collections::BTreeMap, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage,
    cli::{CostMode, PricingOverride},
    format_date_tz, format_rfc3339_millis, total_usage_tokens,
};

pub(super) struct ZcodeUsageRow {
    pub(super) id: String,
    pub(super) session_id: String,
    pub(super) started_at: i64,
    pub(super) model_id: String,
    pub(super) provider_id: Option<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) computed_total_tokens: u64,
    pub(super) directory: Option<String>,
    pub(super) version: Option<String>,
}

pub(super) fn row_to_entry(
    row: ZcodeUsageRow,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
    pricing_overrides: &BTreeMap<String, PricingOverride>,
) -> Option<LoadedEntry> {
    if row.id.trim().is_empty()
        || row.session_id.trim().is_empty()
        || row.model_id.trim().is_empty()
        || row.started_at <= 0
    {
        return None;
    }

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
    let (usage, extra_total_tokens) =
        apply_total_token_fallback(usage, 0, row.computed_total_tokens);
    if total_usage_tokens(usage) == 0 && extra_total_tokens == 0 {
        return None;
    }

    let raw_model = row.model_id.trim().to_string();
    let cost_model = pricing_model(
        &raw_model,
        row.provider_id.as_deref(),
        pricing,
        pricing_overrides,
    );
    let timestamp = TimestampMs::from_millis(row.started_at);
    let project_path = row
        .directory
        .filter(|directory| !directory.trim().is_empty())
        .unwrap_or_else(|| "ZCode".to_string());
    // Z.ai exposes new content as a cache-creation bucket for display, but
    // bills that content at the model's standard input rate.
    let is_zai = is_zai_provider(row.provider_id.as_deref())
        || (row.provider_id.is_none() && is_zai_model(&raw_model.to_ascii_lowercase()));
    let cost_usage = if is_zai {
        TokenUsageRaw {
            input_tokens: usage
                .input_tokens
                .saturating_add(usage.cache_creation_input_tokens),
            cache_creation_input_tokens: 0,
            output_tokens: usage.output_tokens.saturating_add(extra_total_tokens),
            ..usage
        }
    } else {
        TokenUsageRaw {
            output_tokens: usage.output_tokens.saturating_add(extra_total_tokens),
            ..usage
        }
    };
    let cost = if mode == CostMode::Display {
        0.0
    } else {
        calculate_cost_for_usage(
            cost_model.as_deref(),
            cost_usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        )
    };
    let missing_pricing_model = if mode == CostMode::Display || cost_model.is_some() {
        None
    } else {
        (total_usage_tokens(usage).saturating_add(extra_total_tokens) > 0)
            .then(|| ccusage_core::model_aliases::resolve_model_name(&raw_model).into_owned())
    };

    Some(LoadedEntry {
        data: UsageEntry {
            session_id: Some(row.session_id.clone()),
            timestamp: format_rfc3339_millis(timestamp),
            version: row.version,
            message: UsageMessage {
                usage,
                model: Some(raw_model.clone()),
                id: Some(row.id),
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        },
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("zcode"),
        session_id: Arc::from(row.session_id),
        project_path: Arc::from(project_path),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(raw_model),
        usage_limit_reset_time: None,
        missing_pricing_model,
    })
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

fn pricing_model(
    raw_model: &str,
    provider_id: Option<&str>,
    pricing: &PricingMap,
    pricing_overrides: &BTreeMap<String, PricingOverride>,
) -> Option<String> {
    let candidates = pricing_candidates(raw_model, provider_id);
    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| pricing_overrides.contains_key(*candidate))
    {
        return Some(candidate.clone());
    }
    if provider_id.is_some_and(|provider| !is_zai_provider(Some(provider))) {
        return None;
    }

    if let Some(candidate) = candidates
        .iter()
        .filter(|candidate| candidate.starts_with("zai/"))
        .find(|candidate| pricing.find(candidate).is_some())
    {
        return Some(candidate.clone());
    }
    candidates
        .iter()
        .filter(|candidate| !candidate.starts_with("zai/"))
        .find(|candidate| pricing.find(candidate).is_some())
        .cloned()
}

fn is_zai_provider(provider_id: Option<&str>) -> bool {
    provider_id.is_some_and(|provider| {
        matches!(
            provider.trim().to_ascii_lowercase().as_str(),
            "zai"
                | "z.ai"
                | "zai-coding-plan"
                | "builtin:zai-coding-plan"
                | "builtin:bigmodel-coding-plan"
        )
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
            id: "usage-1".to_string(),
            session_id: "session-1".to_string(),
            started_at: 1_786_909_042_666,
            model_id: "GLM-5.3".to_string(),
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
        let entry = row_to_entry(
            row(),
            Some(&JiffTimeZone::UTC),
            CostMode::Display,
            &PricingMap::load_embedded(),
            &BTreeMap::new(),
        )
        .unwrap();
        let usage = entry.data.message.usage;

        assert_eq!(usage.input_tokens, 60);
        assert_eq!(usage.cache_creation_input_tokens, 15);
        assert_eq!(usage.cache_read_input_tokens, 25);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(total_usage_tokens(usage), 110);
        assert_eq!(entry.data.message.model.as_deref(), Some("GLM-5.3"));
        assert_eq!(entry.data.version.as_deref(), Some("0.16.3"));
        assert_eq!(entry.data.timestamp, "2026-08-16T19:37:22.666Z");
    }

    #[test]
    fn prices_zai_models_but_not_unknown_custom_providers() {
        let pricing = PricingMap::load_embedded();
        let priced = row_to_entry(
            row(),
            Some(&JiffTimeZone::UTC),
            CostMode::Calculate,
            &pricing,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(priced.cost > 0.0);
        assert!(priced.missing_pricing_model.is_none());

        let mut custom = row();
        custom.provider_id = Some("847d13c9-0568-4f2f-818e-8bd498e5d920".to_string());
        custom.model_id = "deepseek-v4-flash".to_string();
        let custom = row_to_entry(
            custom,
            Some(&JiffTimeZone::UTC),
            CostMode::Calculate,
            &pricing,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(custom.cost, 0.0);
        assert_eq!(
            custom.missing_pricing_model.as_deref(),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn prices_legacy_bigmodel_provider_with_provider_qualified_glm_pricing() {
        let mut row = row();
        row.provider_id = Some("builtin:bigmodel-coding-plan".to_string());

        let entry = row_to_entry(
            row,
            Some(&JiffTimeZone::UTC),
            CostMode::Calculate,
            &PricingMap::load_embedded(),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(entry.cost, 0.00015549999999999999);
        assert!(entry.missing_pricing_model.is_none());
    }

    #[test]
    fn prices_zai_cache_creation_at_standard_input_rate_for_glm_52_and_53() {
        let pricing = PricingMap::load_embedded();
        let priced = |model: &str| {
            let mut row = row();
            row.model_id = model.to_string();
            row_to_entry(
                row,
                Some(&JiffTimeZone::UTC),
                CostMode::Calculate,
                &pricing,
                &BTreeMap::new(),
            )
            .unwrap()
        };

        let glm_52 = priced("GLM-5.2");
        assert_eq!(glm_52.cost, 0.00015549999999999999);
        assert_eq!(glm_52.data.message.usage.input_tokens, 60);
        assert_eq!(glm_52.data.message.usage.cache_creation_input_tokens, 15);

        let glm_53 = priced("GLM-5.3");
        assert_eq!(glm_53.cost, 0.00015549999999999999);
        assert_eq!(glm_53.data.message.usage.input_tokens, 60);
        assert_eq!(glm_53.data.message.usage.cache_creation_input_tokens, 15);
    }

    #[test]
    fn honors_raw_model_overrides_for_custom_providers() {
        let mut row = row();
        row.provider_id = Some("847d13c9-0568-4f2f-818e-8bd498e5d920".to_string());
        row.model_id = "deepseek-v4-flash".to_string();
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
        let pricing = PricingMap::load_with_overrides(true, false, overrides.iter());

        let entry = row_to_entry(
            row,
            Some(&JiffTimeZone::UTC),
            CostMode::Calculate,
            &pricing,
            &overrides,
        )
        .unwrap();

        assert!(entry.cost > 0.0);
        assert!(entry.missing_pricing_model.is_none());
    }

    #[test]
    fn routes_unaccounted_total_tokens_to_extra_tokens() {
        let mut row = row();
        row.computed_total_tokens = 123;
        let entry = row_to_entry(
            row,
            Some(&JiffTimeZone::UTC),
            CostMode::Display,
            &PricingMap::load_embedded(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(entry.extra_total_tokens, 13);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
    }

    #[test]
    fn returns_no_entry_for_invalid_identity_or_timestamp() {
        let mut invalid_row = row();
        invalid_row.id.clear();
        assert!(
            row_to_entry(
                invalid_row,
                Some(&JiffTimeZone::UTC),
                CostMode::Display,
                &PricingMap::default(),
                &BTreeMap::new(),
            )
            .is_none()
        );

        let mut invalid_row = row();
        invalid_row.started_at = 0;
        assert!(
            row_to_entry(
                invalid_row,
                Some(&JiffTimeZone::UTC),
                CostMode::Display,
                &PricingMap::default(),
                &BTreeMap::new(),
            )
            .is_none()
        );
    }
}
