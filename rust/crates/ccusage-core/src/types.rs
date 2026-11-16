use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::TimestampMs;

mod iterations;

use iterations::{Lenient, MessageIterationsCacheCreation};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub session_id: Option<String>,
    pub timestamp: String,
    pub version: Option<String>,
    pub message: UsageMessage,
    #[serde(rename = "costUSD")]
    pub cost_usd: Option<f64>,
    pub request_id: Option<String>,
    pub is_api_error_message: Option<bool>,
    pub is_sidechain: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageMessage {
    pub usage: TokenUsageRaw,
    pub model: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(from = "TokenUsageRawJson")]
pub struct TokenUsageRaw {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub speed: Option<Speed>,
    pub cache_creation: Option<CacheCreationRaw>,
}

#[derive(Deserialize)]
struct TokenUsageRawJson {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    speed: Option<Speed>,
    #[serde(default)]
    cache_creation: Option<CacheCreationRaw>,
    #[serde(default)]
    iterations: Lenient<MessageIterationsCacheCreation>,
}

impl From<TokenUsageRawJson> for TokenUsageRaw {
    fn from(json: TokenUsageRawJson) -> Self {
        Self {
            input_tokens: json.input_tokens,
            output_tokens: json.output_tokens,
            cache_creation_input_tokens: json.cache_creation_input_tokens,
            cache_read_input_tokens: json.cache_read_input_tokens,
            speed: json.speed,
            // Claude Code advisor turns report one `usage` object whose
            // top-level `cache_creation` split covers only the first `message`
            // iteration, while `cache_creation_input_tokens` sums them all.
            cache_creation: json.iterations.0.0.or(json.cache_creation),
        }
    }
}

impl TokenUsageRaw {
    pub fn cache_creation_token_count(&self) -> u64 {
        if let Some(b) = &self.cache_creation {
            b.ephemeral_5m_input_tokens
                .saturating_add(b.ephemeral_1h_input_tokens)
        } else {
            self.cache_creation_input_tokens
        }
    }

    /// `None` when the source did not report the cache-write TTL split.
    fn cache_creation_1h_token_count(&self) -> Option<u64> {
        self.cache_creation.map(|b| b.ephemeral_1h_input_tokens)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct CacheCreationRaw {
    #[serde(default)]
    pub(crate) ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub(crate) ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Standard,
    Fast,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub(crate) extra_total_tokens: u64,
}

impl TokenCounts {
    pub fn add_usage(&mut self, usage: TokenUsageRaw) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
    }

    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Part of `cache_creation_tokens` written with the 1-hour cache TTL; the
    /// rest was billed at the 5-minute rate. Absent when no entry reported the
    /// split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_1h_tokens: Option<u64>,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    /// Present in JSON only when true, so priced breakdowns keep their shape.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub missing_pricing: bool,
    /// Part of `cost` taken from stored `costUSD` values in `auto` mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_cost: Option<f64>,
}

impl ModelBreakdown {
    pub fn add_usage(&mut self, usage: &TokenUsageRaw) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        self.add_cache_creation_1h_tokens(usage.cache_creation_1h_token_count());
    }

    /// Folds another breakdown of the same model into this one.
    pub fn merge(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.add_cache_creation_1h_tokens(other.cache_creation_1h_tokens);
        self.extra_total_tokens = self
            .extra_total_tokens
            .saturating_add(other.extra_total_tokens);
        self.cost += other.cost;
        self.missing_pricing |= other.missing_pricing;
        self.add_stored_cost(other.stored_cost);
    }

    fn add_cache_creation_1h_tokens(&mut self, tokens: Option<u64>) {
        if let Some(tokens) = tokens {
            let total = self.cache_creation_1h_tokens.get_or_insert(0);
            *total = total.saturating_add(tokens);
        }
    }

    pub fn add_stored_cost(&mut self, stored_cost: Option<f64>) {
        if let Some(stored_cost) = stored_cost {
            *self.stored_cost.get_or_insert(0.0) += stored_cost;
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }
}

#[derive(Debug, Clone)]
pub struct LoadedEntry {
    pub data: UsageEntry,
    pub timestamp: TimestampMs,
    pub date: String,
    pub project: Arc<str>,
    pub session_id: Arc<str>,
    pub project_path: Arc<str>,
    pub cost: f64,
    pub extra_total_tokens: u64,
    pub credits: Option<f64>,
    pub message_count: Option<u64>,
    pub model: Option<String>,
    pub usage_limit_reset_time: Option<TimestampMs>,
    pub missing_pricing_model: Option<String>,
    pub stored_cost: Option<f64>,
}

#[derive(Debug)]
pub struct LoadedFile {
    pub timestamp: Option<TimestampMs>,
    pub entries: Vec<LoadedEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_activity: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub total_cost: f64,
    pub credits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u64>,
    pub models_used: Vec<String>,
    pub model_breakdowns: Vec<ModelBreakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<String>>,
}

impl UsageSummary {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturates_token_count_accumulation_and_total() {
        let mut counts = TokenCounts {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_creation_tokens: u64::MAX,
            cache_read_tokens: u64::MAX,
            extra_total_tokens: u64::MAX,
        };
        counts.add_usage(TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: 1,
            cache_read_input_tokens: 1,
            ..TokenUsageRaw::default()
        });

        assert_eq!(counts.input_tokens, u64::MAX);
        assert_eq!(counts.output_tokens, u64::MAX);
        assert_eq!(counts.cache_creation_tokens, u64::MAX);
        assert_eq!(counts.cache_read_tokens, u64::MAX);
        assert_eq!(counts.total(), u64::MAX);
    }

    #[test]
    fn sums_message_iteration_breakdowns_instead_of_the_truncated_top_level() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_read_input_tokens": 140030,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 0,
                    "ephemeral_1h_input_tokens": 11142
                },
                "iterations": [
                    {"type": "message", "input_tokens": 2, "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142}},
                    {"type": "advisor_message", "model": "claude-fable-5-2", "input_tokens": 77025, "cache_creation": {"ephemeral_5m_input_tokens": 999, "ephemeral_1h_input_tokens": 999}},
                    {"type": "message", "input_tokens": 2, "cache_creation": {"ephemeral_5m_input_tokens": 1181, "ephemeral_1h_input_tokens": 0}}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.cache_creation_token_count(), 12323);
    }

    #[test]
    fn keeps_the_top_level_breakdown_without_message_iteration_breakdowns() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 0,
                    "ephemeral_1h_input_tokens": 11142
                },
                "iterations": [
                    {"type": "advisor_message", "model": "m", "input_tokens": 1}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.cache_creation_token_count(), 11142);
    }

    #[test]
    fn keeps_the_top_level_breakdown_when_a_message_iteration_lacks_its_own() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142},
                "iterations": [
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142}},
                    {"type": "message", "cache_creation_input_tokens": 1181}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.cache_creation_token_count(), 11142);
    }

    #[test]
    fn malformed_iterations_keep_the_usage_and_its_top_level_breakdown() {
        let iterations = [
            r#""n/a""#,
            "null",
            "{}",
            r#"[1, "x", null, [], {"type": 7}]"#,
            r#"[{"type": "message", "cache_creation": "n/a"}]"#,
            r#"[{"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": -1}}]"#,
            r#"[{"type": "message", "cache_creation": {"ephemeral_1h_input_tokens": "10"}}]"#,
            r#"[{"type": "message", "cache_creation": null}]"#,
        ];
        for iterations in iterations {
            let line = format!(
                r#"{{"input_tokens":1,"output_tokens":2,"cache_creation":{{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":4}},"iterations":{iterations}}}"#
            );
            let usage = serde_json::from_str::<TokenUsageRaw>(&line)
                .unwrap_or_else(|error| panic!("{iterations}: {error}"));

            assert_eq!(usage.cache_creation_token_count(), 7, "{iterations}");
            assert_eq!(
                usage.cache_creation_1h_token_count(),
                Some(4),
                "{iterations}"
            );
        }
    }

    #[test]
    fn takes_the_split_from_message_iterations_without_a_top_level_one() {
        let usage = serde_json::from_str::<TokenUsageRaw>(
            r#"{
                "input_tokens": 1,
                "output_tokens": 2,
                "cache_creation_input_tokens": 30,
                "iterations": [
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 10}},
                    {"type": "message", "cache_creation": {"ephemeral_1h_input_tokens": 20, "ephemeral_2h_input_tokens": 5}}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.cache_creation_token_count(), 30);
        assert_eq!(usage.cache_creation_1h_token_count(), Some(20));
    }
}
