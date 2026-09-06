use ccusage_core::{TokenUsageRaw, apply_total_token_fallback, json_value_u64};
use serde_json::Value;

#[derive(Debug)]
pub(super) struct GjcUsage {
    pub timestamp: String,
    pub message_id: String,
    pub model: String,
    pub usage: TokenUsageRaw,
    pub extra_total_tokens: u64,
    pub cost_usd: Option<f64>,
}

pub(super) fn parse_usage_record(value: &Value) -> Option<GjcUsage> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = value.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage_value = message.get("usage")?;
    let usage = TokenUsageRaw {
        input_tokens: json_value_u64(usage_value.get("input")),
        output_tokens: json_value_u64(usage_value.get("output")),
        cache_creation_input_tokens: json_value_u64(usage_value.get("cacheWrite")),
        cache_read_input_tokens: json_value_u64(usage_value.get("cacheRead")),
        speed: None,
        cache_creation: None,
    };
    let (usage, extra_total_tokens) =
        apply_total_token_fallback(usage, 0, json_value_u64(usage_value.get("totalTokens")));
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
        && extra_total_tokens == 0
    {
        return None;
    }
    let timestamp = value.get("timestamp")?.as_str()?.to_string();
    let message_id = value.get("id")?.as_str()?.to_string();
    let model = message.get("model")?.as_str()?.to_string();
    let cost_usd = usage_value
        .get("cost")
        .and_then(|cost| cost.get("total"))
        .and_then(Value::as_f64);

    Some(GjcUsage {
        timestamp,
        message_id,
        model,
        usage,
        extra_total_tokens,
        cost_usd,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_assistant_usage_record() {
        let record = json!({
            "type": "message",
            "id": "assistant-1",
            "timestamp": "2026-08-28T05:57:30.059Z",
            "message": {
                "role": "assistant",
                "api": "openai-codex-responses",
                "provider": "openai-codex",
                "model": "gpt-5.6-sol",
                "usage": {
                    "input": 954,
                    "output": 25,
                    "cacheRead": 11008,
                    "cacheWrite": 7,
                    "totalTokens": 11994,
                    "cost": {
                        "input": 0.00477,
                        "output": 0.00075,
                        "cacheRead": 0.005504,
                        "cacheWrite": 0.00004375,
                        "total": 0.01106775
                    }
                }
            }
        });

        let parsed = parse_usage_record(&record).expect("usage record");

        assert_eq!(parsed.timestamp, "2026-08-28T05:57:30.059Z");
        assert_eq!(parsed.message_id, "assistant-1");
        assert_eq!(parsed.model, "gpt-5.6-sol");
        assert_eq!(parsed.usage.input_tokens, 954);
        assert_eq!(parsed.usage.output_tokens, 25);
        assert_eq!(parsed.usage.cache_read_input_tokens, 11008);
        assert_eq!(parsed.usage.cache_creation_input_tokens, 7);
        assert_eq!(parsed.extra_total_tokens, 0);
        assert_eq!(parsed.cost_usd, Some(0.01106775));
    }

    #[test]
    fn ignores_non_assistant_records() {
        let record = json!({
            "type": "message",
            "id": "tool-1",
            "timestamp": "2026-08-28T05:57:31.000Z",
            "message": {
                "role": "toolResult",
                "model": "gpt-5.6-sol",
                "usage": {"input": 100}
            }
        });

        assert!(parse_usage_record(&record).is_none());
    }
}
