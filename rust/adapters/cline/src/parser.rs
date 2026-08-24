use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_candidates, parse_ts_timestamp,
};

pub(super) struct ClineEntry {
    pub(super) timestamp: TimestampMs,
    timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    usage: TokenUsageRaw,
    cost_usd: Option<f64>,
}

/// Parses a `*.messages.json` transcript file into one `ClineEntry` per
/// assistant message that carries token metrics. Each entry keeps the model
/// that produced it (from `modelInfo.id`), so a session that switches models
/// shows up as separate per-model rows in the report.
pub(super) fn parse_messages_file(contents: &str, shared: &crate::cli::SharedArgs) -> Vec<ClineEntry> {
    let Ok(value) = serde_json::from_str::<Value>(contents) else {
        crate::debug_log(shared, "Failed to parse Cline messages.json".to_string());
        return Vec::new();
    };

    let session_id = value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let Some(messages) = value.get("messages").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for message in messages {
        let Some(role) = message.get("role").and_then(|v| v.as_str()) else {
            continue;
        };
        if role != "assistant" {
            continue;
        }

        let model = message
            .get("modelInfo")
            .and_then(|mi| mi.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if model.is_empty() {
            continue;
        }

        let metrics = message.get("metrics");
        let usage = metrics.map(parse_metrics).unwrap_or_default();
        let cost_usd = metrics
            .and_then(|m| m.get("cost"))
            .and_then(|v| v.as_f64())
            .filter(|cost| *cost > 0.0);

        if usage.input_tokens == 0
            && usage.output_tokens == 0
            && usage.cache_read_input_tokens == 0
            && usage.cache_creation_input_tokens == 0
            && cost_usd.unwrap_or(0.0) == 0.0
        {
            continue;
        }

        let timestamp = message
            .get("ts")
            .and_then(|v| v.as_u64())
            .filter(|ts| *ts > 0)
            .map(|ts| TimestampMs::from_millis(ts as i64))
            .or_else(|| {
                message
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .and_then(parse_ts_timestamp)
            });
        let Some(timestamp) = timestamp else { continue };

        entries.push(ClineEntry {
            timestamp,
            timestamp_text: format_rfc3339_millis(timestamp),
            session_id: session_id.clone(),
            model,
            usage,
            cost_usd,
        });
    }
    entries
}

fn parse_metrics(metrics: &Value) -> TokenUsageRaw {
    TokenUsageRaw {
        input_tokens: metrics
            .get("inputTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: metrics
            .get("outputTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_input_tokens: metrics
            .get("cacheReadTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_creation_input_tokens: metrics
            .get("cacheWriteTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        speed: None,
        cache_creation: None,
    }
}

pub(super) fn to_loaded_entry(
    entry: ClineEntry,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost = calculate_cline_cost(&entry, pricing);
    let missing_pricing_model = missing_cline_pricing(&entry, pricing);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(format!("cline:{}", entry.session_id)),
        },
        cost_usd: entry.cost_usd,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("cline"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Cline"),
        cost,
        credits: None,
        extra_total_tokens: 0,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

fn calculate_cline_cost(entry: &ClineEntry, pricing: &PricingMap) -> f64 {
    if let Some(cost) = entry.cost_usd.filter(|cost| *cost > 0.0) {
        return cost;
    }
    for candidate in model_candidates(entry) {
        let cost = calculate_cost_for_usage(
            Some(&candidate),
            entry.usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        );
        if cost.is_finite() && cost > 0.0 {
            return cost;
        }
    }
    0.0
}

fn missing_cline_pricing(entry: &ClineEntry, pricing: &PricingMap) -> Option<String> {
    if entry.cost_usd.is_some_and(|cost| cost > 0.0) {
        return None;
    }
    missing_pricing_model_for_candidates(
        &entry.model,
        model_candidates(entry),
        crate::total_usage_tokens(entry.usage),
        Some(pricing),
    )
}

fn model_candidates(entry: &ClineEntry) -> Vec<String> {
    vec![entry.model.clone()]
}

