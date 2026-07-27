use serde_json::Value;

use crate::TokenUsageRaw;

pub fn json_value_u64(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or_default()
}

pub fn non_empty_json_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn total_usage_tokens(usage: TokenUsageRaw) -> u64 {
    usage.input_tokens
        + usage.output_tokens
        + usage.cache_creation_token_count()
        + usage.cache_read_input_tokens
}

pub fn apply_total_token_fallback(
    mut usage: TokenUsageRaw,
    mut extra_total_tokens: u64,
    total_tokens: u64,
) -> (TokenUsageRaw, u64) {
    let known_tokens = total_usage_tokens(usage).saturating_add(extra_total_tokens);
    let missing_tokens = total_tokens.saturating_sub(known_tokens);
    if missing_tokens == 0 {
        return (usage, extra_total_tokens);
    }
    if usage.output_tokens == 0 {
        usage.output_tokens = missing_tokens;
    } else {
        extra_total_tokens = extra_total_tokens.saturating_add(missing_tokens);
    }
    (usage, extra_total_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_total_token_fallback_to_missing_output_tokens() {
        let (usage, extra_total_tokens) = apply_total_token_fallback(
            TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 25,
                speed: None,
                cache_creation: None,
            },
            0,
            175,
        );

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, 25);
        assert_eq!(extra_total_tokens, 0);
    }

    #[test]
    fn keeps_total_fallback_as_extra_when_output_is_known() {
        let (usage, extra_total_tokens) = apply_total_token_fallback(
            TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 25,
                speed: None,
                cache_creation: None,
            },
            0,
            200,
        );

        assert_eq!(usage.output_tokens, 50);
        assert_eq!(extra_total_tokens, 25);
    }
}
