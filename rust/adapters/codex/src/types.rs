use std::{borrow::Cow, collections::BTreeMap};

use serde::Deserialize;

mod bucket;
mod wire;

pub(crate) use bucket::ConfiguredTiers;
pub use bucket::{CodexUsageBucket, TierBuckets};
use wire::{DecodedField, LossyObject, LossyRawUsage, LossyString, LossyTimestamp, codex_record};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct CodexRawUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
}

impl CodexRawUsage {
    pub(crate) const fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.cached_input_tokens == 0
            && self.cache_creation_tokens == 0
            && self.output_tokens == 0
            && self.reasoning_output_tokens == 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CodexUsageCandidate {
    #[default]
    Missing,
    Malformed,
    Decoded(CodexRawUsage),
}

impl CodexUsageCandidate {
    pub(super) const fn decoded(self) -> Option<CodexRawUsage> {
        match self {
            Self::Decoded(usage) => Some(usage),
            Self::Missing | Self::Malformed => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexServiceTier {
    Standard,
    Fast,
    Flex,
    Ultrafast,
}

impl CodexServiceTier {
    pub(crate) fn from_codex_name(name: &str) -> Option<Self> {
        match name {
            // NOTE(2026-07-26, Codex CLI 0.144.0, #1438): Codex Desktop writes `"standard"` and the CLI writes `"default"` for the same tier. Accept both spellings.
            "default" | "standard" => Some(Self::Standard),
            "fast" | "priority" => Some(Self::Fast),
            "flex" => Some(Self::Flex),
            "ultrafast" => Some(Self::Ultrafast),
            _ => None,
        }
    }
}

pub(crate) const fn merge_codex_service_tiers(
    current: Option<CodexServiceTier>,
    incoming: Option<CodexServiceTier>,
) -> Option<CodexServiceTier> {
    match (current, incoming) {
        (Some(CodexServiceTier::Standard), _) | (_, Some(CodexServiceTier::Standard)) => {
            Some(CodexServiceTier::Standard)
        }
        (Some(CodexServiceTier::Ultrafast), _) | (_, Some(CodexServiceTier::Ultrafast)) => {
            Some(CodexServiceTier::Ultrafast)
        }
        (Some(CodexServiceTier::Fast), _) | (_, Some(CodexServiceTier::Fast)) => {
            Some(CodexServiceTier::Fast)
        }
        (Some(CodexServiceTier::Flex), _) | (_, Some(CodexServiceTier::Flex)) => {
            Some(CodexServiceTier::Flex)
        }
        (None, None) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodexTokenUsageEvent {
    pub(crate) session_id: String,
    pub(crate) response_id: Option<String>,
    pub(crate) record_response_id: Option<std::sync::Arc<str>>,
    pub(crate) timestamp: crate::TimestampMs,
    pub(crate) last_activity: String,
    pub(crate) model: String,
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) is_fallback_model: bool,
    pub(crate) service_tier: Option<CodexServiceTier>,
}

impl CodexTokenUsageEvent {
    pub(crate) fn raw_usage(&self) -> CodexRawUsage {
        CodexRawUsage {
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            total_tokens: self.total_tokens,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodexTimestampedUsage {
    pub(crate) usage: CodexUsageBucket,
    pub(crate) tier_usage: TierBuckets,
    pub(crate) configured_copy_usage: BTreeMap<ConfiguredTiers, CodexUsageBucket>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexModelUsage {
    pub(crate) usage: CodexUsageBucket,
    pub(crate) tier_usage: TierBuckets,
    pub(crate) configured_copy_usage: BTreeMap<ConfiguredTiers, CodexUsageBucket>,
    pub(crate) reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub(crate) timestamped_usage: BTreeMap<i64, CodexTimestampedUsage>,
    pub(crate) is_fallback: bool,
}

impl CodexModelUsage {
    pub fn from_buckets(usage: CodexUsageBucket, tier_usage: TierBuckets) -> Self {
        Self {
            usage,
            tier_usage,
            ..Self::default()
        }
    }

    pub(crate) fn add_usage(&mut self, usage: CodexUsageBucket, tier: Option<CodexServiceTier>) {
        self.usage += usage;
        if let Some(tier) = tier {
            self.tier_usage[tier] += usage;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CodexGroup {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub models: BTreeMap<String, CodexModelUsage>,
    pub last_activity: Option<String>,
}

codex_record! { CodexLogEntry {
    entry_type: Option<Cow<'a, str>> = "type" => EntryType, LossyString,
    timestamp: Option<CodexTimestamp<'a>> = "timestamp" => Timestamp, LossyTimestamp,
    created_at: Option<CodexTimestamp<'a>> = "created_at" => CreatedAt, LossyTimestamp,
    created_at_camel: Option<CodexTimestamp<'a>> = "createdAt" => CreatedAtCamel, LossyTimestamp,
    payload: Option<CodexPayload<'a>> = "payload" => Payload, LossyObject,
    data: Option<CodexResultFields<'a>> = "data" => Data, LossyObject,
    result: Option<CodexResultFields<'a>> = "result" => Result, LossyObject,
    response: Option<CodexResultFields<'a>> = "response" => Response, LossyObject,
    usage: CodexUsageCandidate = "usage" => Usage, DecodedField,
    model: Option<Cow<'a, str>> = "model" => Model, LossyString,
    model_name: Option<Cow<'a, str>> = "model_name" => ModelName, LossyString,
    metadata: Option<CodexModelMetadata<'a>> = "metadata" => Metadata, LossyObject,
} }

codex_record! { CodexPayload {
    payload_type: Option<Cow<'a, str>> = "type" => PayloadType, LossyString,
    info: Option<CodexInfo<'a>> = "info" => Info, LossyObject,
    model: Option<Cow<'a, str>> = "model" => Model, LossyString,
    model_name: Option<Cow<'a, str>> = "model_name" => ModelName, LossyString,
    response_id: Option<Cow<'a, str>> = "response_id" => ResponseId, LossyString,
    thread_id: Option<Cow<'a, str>> = "thread_id" => ThreadId, LossyString,
    compaction_response_id: Option<Cow<'a, str>> = "compaction_response_id" => CompactionResponseId, LossyString,
    usage: CodexUsageCandidate = "usage" => Usage, DecodedField,
    thread_token_usage: CodexUsageCandidate = "thread_token_usage" => ThreadTokenUsage, LossyRawUsage,
    metadata: Option<CodexModelMetadata<'a>> = "metadata" => Metadata, LossyObject,
    thread_settings: Option<CodexThreadSettings<'a>> = "thread_settings" => ThreadSettings, LossyObject,
} }

codex_record! { CodexThreadSettings {
    service_tier: Option<Cow<'a, str>> = "service_tier" => ServiceTier, LossyString,
} }

codex_record! { CodexInfo {
    last_token_usage: CodexUsageCandidate = "last_token_usage" => LastTokenUsage, LossyRawUsage,
    total_token_usage: CodexUsageCandidate = "total_token_usage" => TotalTokenUsage, LossyRawUsage,
    model: Option<Cow<'a, str>> = "model" => Model, LossyString,
    model_name: Option<Cow<'a, str>> = "model_name" => ModelName, LossyString,
    metadata: Option<CodexModelMetadata<'a>> = "metadata" => Metadata, LossyObject,
} }

codex_record! { CodexResultFields {
    timestamp: Option<CodexTimestamp<'a>> = "timestamp" => Timestamp, LossyTimestamp,
    created_at: Option<CodexTimestamp<'a>> = "created_at" => CreatedAt, LossyTimestamp,
    created_at_camel: Option<CodexTimestamp<'a>> = "createdAt" => CreatedAtCamel, LossyTimestamp,
    usage: CodexUsageCandidate = "usage" => Usage, DecodedField,
    model: Option<Cow<'a, str>> = "model" => Model, LossyString,
    model_name: Option<Cow<'a, str>> = "model_name" => ModelName, LossyString,
    metadata: Option<CodexModelMetadata<'a>> = "metadata" => Metadata, LossyObject,
} }

codex_record! { CodexModelMetadata {
    model: Option<Cow<'a, str>> = "model" => Model, LossyString,
} }

codex_record! { CodexSessionMeta {
    entry_type: Option<Cow<'a, str>> = "type" => EntryType, LossyString,
    timestamp: Option<CodexTimestamp<'a>> = "timestamp" => Timestamp, LossyTimestamp,
    payload: Option<CodexSessionMetaPayload<'a>> = "payload" => Payload, LossyObject,
} }

codex_record! { CodexSessionMetaPayload {
    id: Option<Cow<'a, str>> = "id" => Id, LossyString,
    forked_from_id: Option<Cow<'a, str>> = "forked_from_id" => ForkedFromId, LossyString,
    history_base: Option<CodexHistoryBase> = "history_base" => HistoryBase, LossyObject,
    source: Option<CodexSessionSource<'a>> = "source" => Source, LossyObject,
} }

#[derive(serde::Deserialize)]
pub(super) struct CodexHistoryBase {}

codex_record! { CodexSessionSource {
    subagent: Option<CodexSubagent<'a>> = "subagent" => Subagent, LossyObject,
} }

codex_record! { CodexSubagent {
    thread_spawn: Option<CodexThreadSpawn<'a>> = "thread_spawn" => ThreadSpawn, LossyObject,
} }

codex_record! { CodexThreadSpawn {
    parent_thread_id: Option<Cow<'a, str>> = "parent_thread_id" => ParentThreadId, LossyString,
} }

#[derive(Clone)]
pub(super) enum CodexTimestamp<'a> {
    String(Cow<'a, str>),
    Number(u64),
    Invalid,
}

#[cfg(test)]
mod tests;
