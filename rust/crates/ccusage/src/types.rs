use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::TimestampMs;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageEntry {
    pub(crate) session_id: Option<String>,
    pub(crate) timestamp: String,
    pub(crate) version: Option<String>,
    pub(crate) message: UsageMessage,
    #[serde(rename = "costUSD")]
    pub(crate) cost_usd: Option<f64>,
    pub(crate) request_id: Option<String>,
    pub(crate) is_api_error_message: Option<bool>,
    pub(crate) is_sidechain: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UsageMessage {
    pub(crate) usage: TokenUsageRaw,
    pub(crate) model: Option<String>,
    pub(crate) id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct TokenUsageRaw {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    #[serde(default)]
    pub(crate) cache_creation_input_tokens: u64,
    #[serde(default)]
    pub(crate) cache_read_input_tokens: u64,
    pub(crate) speed: Option<Speed>,
    #[serde(default)]
    pub(crate) cache_creation: Option<CacheCreationRaw>,
}

impl TokenUsageRaw {
    pub(crate) fn cache_creation_token_count(&self) -> u64 {
        if let Some(b) = &self.cache_creation {
            b.ephemeral_5m_input_tokens + b.ephemeral_1h_input_tokens
        } else {
            self.cache_creation_input_tokens
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct CacheCreationRaw {
    #[serde(default)]
    pub(crate) ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub(crate) ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Speed {
    Standard,
    Fast,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenCounts {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) extra_total_tokens: u64,
}

impl TokenCounts {
    pub(crate) fn add_usage(&mut self, usage: TokenUsageRaw) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.cache_creation_tokens += usage.cache_creation_token_count();
        self.cache_read_tokens += usage.cache_read_input_tokens;
    }

    pub(crate) fn total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_tokens
            + self.cache_read_tokens
            + self.extra_total_tokens
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelBreakdown {
    pub(crate) model_name: String,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub(crate) extra_total_tokens: u64,
    pub(crate) cost: f64,
    #[serde(skip_serializing)]
    pub(crate) missing_pricing: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedEntry {
    pub(crate) data: UsageEntry,
    pub(crate) timestamp: TimestampMs,
    pub(crate) date: String,
    pub(crate) project: Arc<str>,
    pub(crate) session_id: Arc<str>,
    pub(crate) project_path: Arc<str>,
    pub(crate) cost: f64,
    pub(crate) extra_total_tokens: u64,
    pub(crate) credits: Option<f64>,
    pub(crate) message_count: Option<u64>,
    pub(crate) model: Option<String>,
    pub(crate) usage_limit_reset_time: Option<TimestampMs>,
    pub(crate) missing_pricing_model: Option<String>,
}

#[derive(Debug)]
pub(crate) struct LoadedFile {
    pub(crate) timestamp: Option<TimestampMs>,
    pub(crate) entries: Vec<LoadedEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CodexRawUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexTokenUsageEvent {
    pub(crate) session_id: String,
    pub(crate) project_path: Option<String>,
    pub(crate) timestamp: String,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) is_fallback_model: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodexModelUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) is_fallback: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodexGroup {
    pub(crate) project_path: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) models: BTreeMap<String, CodexModelUsage>,
    pub(crate) last_activity: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) month: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) week: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_activity: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub(crate) extra_total_tokens: u64,
    pub(crate) total_cost: f64,
    pub(crate) credits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message_count: Option<u64>,
    pub(crate) models_used: Vec<String>,
    pub(crate) model_breakdowns: Vec<ModelBreakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) versions: Option<Vec<String>>,
}

impl UsageSummary {
    pub(crate) fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_tokens
            + self.cache_read_tokens
            + self.extra_total_tokens
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SessionBlock {
    pub(crate) id: String,
    pub(crate) start_time: TimestampMs,
    pub(crate) end_time: TimestampMs,
    pub(crate) actual_end_time: Option<TimestampMs>,
    pub(crate) is_active: bool,
    pub(crate) is_gap: bool,
    pub(crate) entries: Vec<LoadedEntry>,
    pub(crate) token_counts: TokenCounts,
    pub(crate) cost_usd: f64,
    pub(crate) models: Vec<String>,
    pub(crate) usage_limit_reset_time: Option<TimestampMs>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BurnRate {
    pub(crate) tokens_per_minute: f64,
    pub(crate) tokens_per_minute_for_indicator: f64,
    pub(crate) cost_per_hour: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Projection {
    pub(crate) total_tokens: u64,
    pub(crate) total_cost: f64,
    pub(crate) remaining_minutes: u64,
}
