use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::TimestampMs;

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
    pub attribution_plugin: Option<String>,
    pub attribution_skill: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageMessage {
    pub usage: TokenUsageRaw,
    pub model: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct TokenUsageRaw {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    pub speed: Option<Speed>,
    #[serde(default)]
    pub cache_creation: Option<CacheCreationRaw>,
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
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginBreakdown {
    pub plugin_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBreakdown {
    pub skill_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTypeBreakdown {
    pub source_type: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
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
    pub plugin_breakdowns: Vec<PluginBreakdown>,
    pub skill_breakdowns: Vec<SkillBreakdown>,
    pub source_type_breakdowns: Vec<SourceTypeBreakdown>,
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
    fn deserializes_usage_entry_with_attribution_fields() {
        let json = r#"{
            "sessionId": "test-session",
            "timestamp": "2024-01-01T00:00:00Z",
            "message": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50
                },
                "model": "test-model",
                "id": "msg-123"
            },
            "costUSD": 0.01,
            "attributionPlugin": "test-plugin",
            "attributionSkill": "test-skill"
        }"#;

        let entry: UsageEntry = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(entry.attribution_plugin, Some("test-plugin".to_string()));
        assert_eq!(entry.attribution_skill, Some("test-skill".to_string()));
    }

    #[test]
    fn deserializes_usage_entry_without_attribution_fields() {
        let json = r#"{
            "sessionId": "test-session",
            "timestamp": "2024-01-01T00:00:00Z",
            "message": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50
                },
                "model": "test-model",
                "id": "msg-456"
            },
            "costUSD": 0.02
        }"#;

        let entry: UsageEntry = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(entry.attribution_plugin, None);
        assert_eq!(entry.attribution_skill, None);
    }
}
