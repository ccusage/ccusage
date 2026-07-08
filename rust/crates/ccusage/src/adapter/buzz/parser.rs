use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, TokenUsageRaw, UsageEntry, UsageMessage, calculate_cost_for_usage,
    cli::CostMode, format_date_tz, missing_pricing_model_for_candidates,
};

/// Token counts from a single agent turn or cumulative session total.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnTokens {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// The decrypted `AgentTurnMetricPayload` stored as `raw_json` in archive.db.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTurnMetricPayload {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    turn_seq: Option<u64>,
    timestamp: String,
    #[serde(default)]
    turn: Option<TurnTokens>,
    #[serde(default)]
    cumulative: Option<TurnTokens>,
}

/// Parse a single `raw_json` value from an `archived_events` row into a
/// `LoadedEntry`. Returns `None` for rows that must be skipped:
/// - `turn` is null and `turn_seq > 1` (cumulative-only after first turn —
///   summing cumulative would double-count the whole session).
/// - The row has zero tokens after applying the accounting rule.
pub(super) fn payload_to_entry(
    event_id: &str,
    raw_json: &str,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    let payload = serde_json::from_str::<AgentTurnMetricPayload>(raw_json).ok()?;

    let timestamp = crate::parse_ts_timestamp(payload.timestamp.trim())?;
    let turn_seq = payload.turn_seq.unwrap_or(0);

    // Accounting rule:
    //   - turn present → use turn tokens (per-turn delta)
    //   - turn null + turnSeq == 1 → use cumulative (it IS the first turn)
    //   - turn null + turnSeq  > 1 → SKIP (dropping avoids double-count)
    let (input_tokens, output_tokens) = match &payload.turn {
        Some(t) => (t.input_tokens.unwrap_or(0), t.output_tokens.unwrap_or(0)),
        None => {
            if turn_seq <= 1 {
                // First turn: cumulative == this turn's usage.
                let c = payload.cumulative.as_ref()?;
                (c.input_tokens.unwrap_or(0), c.output_tokens.unwrap_or(0))
            } else {
                // turnSeq > 1 with no per-turn delta: skip.
                return None;
            }
        }
    };

    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }

    let model = payload
        .model
        .as_deref()
        .filter(|m| !m.is_empty())
        .map(str::to_string);

    let session_id = payload
        .session_id
        .as_deref()
        .unwrap_or(event_id)
        .to_string();

    let usage = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };

    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let data = UsageEntry {
        session_id: Some(session_id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: model.clone(),
            id: Some(event_id.to_string()),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };

    let cost = calculate_buzz_cost(model.as_deref(), usage, pricing);
    let missing_pricing_model = missing_buzz_pricing(model.as_deref(), usage, pricing);

    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: Arc::from("buzz"),
        session_id: Arc::from(session_id.as_str()),
        project_path: Arc::from("Buzz"),
        cost,
        credits: None,
        model,
        usage_limit_reset_time: None,
        missing_pricing_model,
        extra_total_tokens: 0,
        message_count: None,
        data,
    })
}

fn calculate_buzz_cost(model: Option<&str>, usage: TokenUsageRaw, pricing: &PricingMap) -> f64 {
    let Some(model) = model else {
        return 0.0;
    };
    let raw =
        calculate_cost_for_usage(Some(model), usage, None, CostMode::Calculate, Some(pricing));
    if raw > 0.0 {
        return raw;
    }
    // Mirror goose's provider-prefixed fallback: try "{provider}/{model}" when the
    // model name includes a provider prefix like "goose-" or "databricks-".
    if let Some(provider) = guess_provider(model) {
        let candidate = format!("{provider}/{model}");
        return calculate_cost_for_usage(
            Some(&candidate),
            usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        );
    }
    0.0
}

fn missing_buzz_pricing(
    model: Option<&str>,
    usage: TokenUsageRaw,
    pricing: &PricingMap,
) -> Option<String> {
    let model = model?;
    let mut candidates = vec![model.to_string()];
    if let Some(provider) = guess_provider(model) {
        candidates.push(format!("{provider}/{model}"));
    }
    missing_pricing_model_for_candidates(
        model,
        candidates,
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

/// Infer a provider string from known buzz-agent model name prefixes.
/// The fuzzy matcher in ccusage's pricing engine strips these prefixes
/// automatically, but we supply the explicit candidate as a belt-and-suspenders
/// fallback — same pattern as the goose adapter's `normalize_provider`.
fn guess_provider(model: &str) -> Option<&'static str> {
    if model.starts_with("goose-") {
        return Some("goose");
    }
    if model.starts_with("databricks-") {
        return Some("databricks");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pricing() -> PricingMap {
        PricingMap::load_embedded()
    }

    #[test]
    fn parses_per_turn_tokens_when_turn_present() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "sessionId":"ses_abc","turnSeq":2,"timestamp":"2026-07-07T10:00:00.000Z",
            "turn":{"inputTokens":5000,"outputTokens":400,"totalTokens":null,"costUsd":null},
            "cumulative":{"inputTokens":10000,"outputTokens":800,"totalTokens":null,"costUsd":null},
            "deltaReliable":true,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let entry = payload_to_entry("evt-1", raw, None, &pricing).unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 5000);
        assert_eq!(entry.data.message.usage.output_tokens, 400);
        assert_eq!(entry.session_id.as_ref(), "ses_abc");
        assert_eq!(entry.model.as_deref(), Some("goose-claude-4-6-sonnet"));
    }

    #[test]
    fn uses_cumulative_for_turn_seq_1_null_turn() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "sessionId":"ses_abc","turnSeq":1,"timestamp":"2026-07-07T09:00:00.000Z",
            "turn":null,
            "cumulative":{"inputTokens":3000,"outputTokens":200,"totalTokens":null,"costUsd":null},
            "deltaReliable":false,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let entry = payload_to_entry("evt-0", raw, None, &pricing).unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 3000);
        assert_eq!(entry.data.message.usage.output_tokens, 200);
    }

    #[test]
    fn skips_turn_seq_gt_1_null_turn() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "sessionId":"ses_abc","turnSeq":3,"timestamp":"2026-07-07T11:00:00.000Z",
            "turn":null,
            "cumulative":{"inputTokens":99999,"outputTokens":9999,"totalTokens":null,"costUsd":null},
            "deltaReliable":false,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let result = payload_to_entry("evt-2", raw, None, &pricing);

        // Must be None — counting cumulative on turnSeq>1 would double-count.
        assert!(result.is_none(), "expected skip, got {result:?}");
    }

    #[test]
    fn counts_tokens_but_returns_zero_cost_for_null_model() {
        let raw = r#"{
            "harness":"buzz-agent","model":null,
            "sessionId":"ses_old","turnSeq":1,"timestamp":"2026-06-01T08:00:00.000Z",
            "turn":null,
            "cumulative":{"inputTokens":1000,"outputTokens":100,"totalTokens":null,"costUsd":null},
            "deltaReliable":false,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let entry = payload_to_entry("evt-old", raw, None, &pricing).unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 1000);
        assert_eq!(entry.data.message.usage.output_tokens, 100);
        assert_eq!(entry.model, None);
        // No model → cost is 0, but missing_pricing_model is also None (no model to report missing)
        assert_eq!(entry.cost, 0.0);
        assert_eq!(entry.missing_pricing_model, None);
    }

    #[test]
    fn skips_row_with_zero_tokens() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "sessionId":"ses_empty","turnSeq":1,"timestamp":"2026-07-07T12:00:00.000Z",
            "turn":{"inputTokens":0,"outputTokens":0,"totalTokens":null,"costUsd":null},
            "cumulative":{"inputTokens":0,"outputTokens":0,"totalTokens":null,"costUsd":null},
            "deltaReliable":true,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let result = payload_to_entry("evt-empty", raw, None, &pricing);

        assert!(result.is_none());
    }

    #[test]
    fn falls_back_to_event_id_as_session_id_when_absent() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "turnSeq":1,"timestamp":"2026-07-07T09:00:00.000Z",
            "turn":{"inputTokens":100,"outputTokens":10,"totalTokens":null,"costUsd":null},
            "deltaReliable":true,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let entry = payload_to_entry("fallback-id", raw, None, &pricing).unwrap();

        assert_eq!(entry.session_id.as_ref(), "fallback-id");
    }

    #[test]
    fn calculates_nonzero_cost_for_known_model() {
        let raw = r#"{
            "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
            "sessionId":"ses_priced","turnSeq":2,"timestamp":"2026-07-07T10:00:00.000Z",
            "turn":{"inputTokens":100000,"outputTokens":5000,"totalTokens":null,"costUsd":null},
            "cumulative":{"inputTokens":200000,"outputTokens":10000,"totalTokens":null,"costUsd":null},
            "deltaReliable":true,"stopReason":"end_turn"
        }"#;
        let pricing = make_pricing();
        let entry = payload_to_entry("evt-priced", raw, None, &pricing).unwrap();

        assert!(
            entry.cost > 0.0,
            "expected nonzero cost for goose-claude-4-6-sonnet, got {}",
            entry.cost
        );
        assert_eq!(entry.missing_pricing_model, None);
    }
}
