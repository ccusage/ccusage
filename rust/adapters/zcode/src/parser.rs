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
    pub(super) version: Option<String>,
}

/// Turns a row into a reportable entry.
///
/// ZCode counts cache-read tokens *inside* `input_tokens` (its own
/// `computed_total_tokens` is `input + output` exactly), so the cache-read
/// slice is carved out of input to match the additive buckets ccusage reports.
pub(super) fn row_to_entry(
    row: ZcodeUsageRow,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    if row.started_at <= 0 {
        return None;
    }
    let usage = TokenUsageRaw {
        input_tokens: row.input_tokens.saturating_sub(row.cache_read_input_tokens),
        output_tokens: row.output_tokens,
        cache_creation_input_tokens: row.cache_creation_input_tokens,
        cache_read_input_tokens: row.cache_read_input_tokens,
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
    // Pricing keys are matched case-sensitively and the tables store
    // lowercase ids (`glm-5.3`), while ZCode writes `GLM-5.3`.
    let model = row.model_id.to_ascii_lowercase();
    let timestamp = TimestampMs::from_millis(row.started_at);
    let data = UsageEntry {
        session_id: Some(row.session_id.clone()),
        timestamp: crate::format_rfc3339_millis(timestamp),
        version: row.version.clone(),
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: Some(row.id),
        },
        // ZCode records no per-request cost; every mode derives from pricing.
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: usage.output_tokens.saturating_add(extra_total_tokens),
        ..usage
    };
    let cost = if mode == CostMode::Display {
        0.0
    } else {
        calculate_cost_for_usage(
            Some(&model),
            cost_usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        )
    };
    let missing_pricing_model = if mode == CostMode::Display {
        None
    } else {
        missing_pricing_model_for_candidates(
            &model,
            [model.clone()],
            total_usage_tokens(usage),
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
