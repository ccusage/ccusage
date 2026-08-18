use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, cli::CostMode, format_date_tz,
    missing_pricing_model_for_candidates, total_usage_tokens,
};

/// One `model_usage` row joined with its session, as read by the loader. Token
/// counts are already non-negative; the loader clamps negative columns.
///
/// `reasoning_tokens` is deliberately not carried: every observed row records
/// zero and `computed_total_tokens` already encodes any tokens outside the
/// counted buckets, which the fallback below routes safely.
pub(super) struct ZcodeUsageRow {
    pub(super) id: String,
    pub(super) session_id: String,
    pub(super) started_at: i64,
    pub(super) model_id: String,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) computed_total_tokens: u64,
    pub(super) directory: Option<String>,
}

/// Turns a row into a reportable entry.
///
/// ZCode counts cache tokens *inside* `input_tokens` (its own
/// `computed_total_tokens` is `input + output` exactly), so the cache slices
/// are carved out of input to match the additive buckets ccusage reports.
pub(super) fn row_to_entry(
    row: ZcodeUsageRow,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    if row.started_at <= 0 {
        return None;
    }
    let cache_read_input_tokens = row.cache_read_input_tokens.min(row.input_tokens);
    let input_tokens = row.input_tokens - cache_read_input_tokens;
    let cache_creation_input_tokens = row.cache_creation_input_tokens.min(input_tokens);
    let usage = TokenUsageRaw {
        input_tokens: input_tokens - cache_creation_input_tokens,
        output_tokens: row.output_tokens,
        cache_creation_input_tokens,
        cache_read_input_tokens,
        speed: None,
        cache_creation: None,
    };
    // ZCode records no reasoning separately today (computed_total == input +
    // output, and the carved-out buckets sum back to exactly that). If a future
    // version moves reasoning outside output, computed_total grows past the
    // counted buckets and the fallback routes it to the extra bucket.
    let (usage, extra_total_tokens) =
        apply_total_token_fallback(usage, 0, row.computed_total_tokens);
    if total_usage_tokens(usage) == 0 && extra_total_tokens == 0 {
        return None;
    }
    let model = row.model_id.trim().to_string();
    if model.is_empty() {
        return None;
    }
    let timestamp = TimestampMs::from_millis(row.started_at);
    let data = UsageEntry {
        session_id: Some(row.session_id.clone()),
        timestamp: crate::format_rfc3339_millis(timestamp),
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: Some(row.id),
        },
        // ZCode records no per-request cost.
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(extra_total_tokens),
        ..usage
    };
    let pricing_candidates = model_candidates(&model);
    let pricing_model = pricing_model(&model, &pricing_candidates, pricing);
    let cost = calculate_cost_for_usage(pricing_model, cost_usage, None, mode, Some(pricing));
    let missing_pricing_model = if mode == CostMode::Display {
        None
    } else {
        missing_pricing_model_for_candidates(
            &model,
            pricing_candidates,
            total_usage_tokens(cost_usage),
            Some(pricing),
        )
    };
    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("zcode"),
        session_id: Arc::from(row.session_id),
        project_path: Arc::from(row.directory.as_deref().unwrap_or("ZCode")),
        cost,
        extra_total_tokens,
        credits: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        message_count: None,
        data,
    })
}

fn model_candidates(model: &str) -> [String; 4] {
    let lowercase = model.to_ascii_lowercase();
    [
        model.to_string(),
        format!("zai/{model}"),
        lowercase.clone(),
        format!("zai/{lowercase}"),
    ]
}

fn pricing_model<'a>(
    model: &'a str,
    candidates: &'a [String; 4],
    pricing: &PricingMap,
) -> Option<&'a str> {
    if pricing.find_exact(model).is_some() {
        return Some(model);
    }

    // ZCode model ids omit the provider. Prefer Z.ai's provider-qualified
    // entry before a generic fuzzy match, while preserving an exact raw-id
    // override above for custom providers.
    [1, 3, 2, 0].into_iter().find_map(|index| {
        pricing
            .find(&candidates[index])
            .map(|_| candidates[index].as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model_id: &str) -> ZcodeUsageRow {
        ZcodeUsageRow {
            id: "usage-1".to_string(),
            session_id: "session-1".to_string(),
            started_at: 1_735_689_600_123,
            model_id: model_id.to_string(),
            input_tokens: 1_000,
            output_tokens: 300,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 200,
            computed_total_tokens: 1_300,
            directory: None,
        }
    }

    #[test]
    fn uses_zai_pricing_for_glm_5_2() {
        let pricing = PricingMap::load_embedded();

        let entry = row_to_entry(row("GLM-5.2"), None, CostMode::Calculate, &pricing).unwrap();

        assert_eq!(entry.model.as_deref(), Some("GLM-5.2"));
        assert!((entry.cost - 0.002_492).abs() < 1e-12);
        assert!(entry.missing_pricing_model.is_none());
    }

    #[test]
    fn unknown_models_report_missing_pricing() {
        let pricing = PricingMap::load_embedded();

        let entry = row_to_entry(
            row("custom-zcode-provider-unknown-v1"),
            None,
            CostMode::Auto,
            &pricing,
        )
        .unwrap();

        assert_eq!(entry.cost, 0.0);
        assert_eq!(
            entry.missing_pricing_model.as_deref(),
            Some("custom-zcode-provider-unknown-v1")
        );
    }

    #[test]
    fn display_mode_reports_zero_without_missing_pricing() {
        let pricing = PricingMap::load_embedded();

        let entry = row_to_entry(row("GLM-5.2"), None, CostMode::Display, &pricing).unwrap();

        assert_eq!(entry.cost, 0.0);
        assert!(entry.missing_pricing_model.is_none());
    }
}
