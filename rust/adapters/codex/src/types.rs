use std::{
    borrow::Cow,
    collections::BTreeMap,
    marker::PhantomData,
    ops::{Add, AddAssign, Index, IndexMut},
};

use serde::Deserialize;

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

impl<'de> Deserialize<'de> for CodexUsageCandidate {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UsageCandidateVisitor;

        impl<'de> serde::de::Visitor<'de> for UsageCandidateVisitor {
            type Value = CodexUsageCandidate;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an optional Codex usage object")
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Missing)
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Missing)
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }

            fn visit_map<A>(self, map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let value = serde_json::Value::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(serde_json::from_value(value)
                    .map(CodexUsageCandidate::Decoded)
                    .unwrap_or(CodexUsageCandidate::Malformed))
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                while sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(CodexUsageCandidate::Malformed)
            }

            fn visit_bool<E>(self, _: bool) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Malformed)
            }

            fn visit_i64<E>(self, _: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Malformed)
            }

            fn visit_u64<E>(self, _: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Malformed)
            }

            fn visit_f64<E>(self, _: f64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Malformed)
            }

            fn visit_str<E>(self, _: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CodexUsageCandidate::Malformed)
            }
        }

        deserializer.deserialize_any(UsageCandidateVisitor)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexUsageBucket {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub long_context_input_tokens: u64,
    pub long_context_cached_input_tokens: u64,
    pub long_context_cache_creation_tokens: u64,
    pub long_context_output_tokens: u64,
}

impl CodexUsageBucket {
    pub(crate) fn from_usage(usage: CodexRawUsage, is_long_context: bool) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            output_tokens: usage.output_tokens,
            long_context_input_tokens: if is_long_context {
                usage.input_tokens
            } else {
                0
            },
            long_context_cached_input_tokens: if is_long_context {
                usage.cached_input_tokens
            } else {
                0
            },
            long_context_cache_creation_tokens: if is_long_context {
                usage.cache_creation_tokens
            } else {
                0
            },
            long_context_output_tokens: if is_long_context {
                usage.output_tokens
            } else {
                0
            },
        }
    }

    pub(crate) fn saturating_sub(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(other.cached_input_tokens),
            cache_creation_tokens: self
                .cache_creation_tokens
                .saturating_sub(other.cache_creation_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            long_context_input_tokens: self
                .long_context_input_tokens
                .saturating_sub(other.long_context_input_tokens),
            long_context_cached_input_tokens: self
                .long_context_cached_input_tokens
                .saturating_sub(other.long_context_cached_input_tokens),
            long_context_cache_creation_tokens: self
                .long_context_cache_creation_tokens
                .saturating_sub(other.long_context_cache_creation_tokens),
            long_context_output_tokens: self
                .long_context_output_tokens
                .saturating_sub(other.long_context_output_tokens),
        }
    }
}

impl AddAssign for CodexUsageBucket {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.long_context_input_tokens = self
            .long_context_input_tokens
            .saturating_add(other.long_context_input_tokens);
        self.long_context_cached_input_tokens = self
            .long_context_cached_input_tokens
            .saturating_add(other.long_context_cached_input_tokens);
        self.long_context_cache_creation_tokens = self
            .long_context_cache_creation_tokens
            .saturating_add(other.long_context_cache_creation_tokens);
        self.long_context_output_tokens = self
            .long_context_output_tokens
            .saturating_add(other.long_context_output_tokens);
    }
}

impl Add for CodexUsageBucket {
    type Output = Self;

    fn add(mut self, other: Self) -> Self {
        self += other;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierBuckets([CodexUsageBucket; 4]);

impl CodexServiceTier {
    const fn index(self) -> usize {
        match self {
            Self::Standard => 0,
            Self::Fast => 1,
            Self::Flex => 2,
            Self::Ultrafast => 3,
        }
    }
}

impl Index<CodexServiceTier> for TierBuckets {
    type Output = CodexUsageBucket;

    fn index(&self, tier: CodexServiceTier) -> &Self::Output {
        &self.0[tier.index()]
    }
}

impl IndexMut<CodexServiceTier> for TierBuckets {
    fn index_mut(&mut self, tier: CodexServiceTier) -> &mut Self::Output {
        &mut self.0[tier.index()]
    }
}

impl AddAssign for TierBuckets {
    fn add_assign(&mut self, other: Self) {
        for (target, source) in self.0.iter_mut().zip(other.0) {
            *target += source;
        }
    }
}

impl TierBuckets {
    pub(crate) fn total(self) -> CodexUsageBucket {
        self.0
            .into_iter()
            .fold(CodexUsageBucket::default(), Add::add)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CodexTimestampedUsage {
    pub(crate) usage: CodexUsageBucket,
    pub(crate) recorded_usage: TierBuckets,
}

#[derive(Debug, Clone, Default)]
pub struct CodexModelUsage {
    pub(crate) usage: CodexUsageBucket,
    pub(crate) recorded_usage: TierBuckets,
    pub(crate) reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub(crate) timestamped_usage: BTreeMap<i64, CodexTimestampedUsage>,
    pub(crate) is_fallback: bool,
}

impl CodexModelUsage {
    pub fn from_buckets(usage: CodexUsageBucket, recorded_usage: TierBuckets) -> Self {
        Self {
            usage,
            recorded_usage,
            ..Self::default()
        }
    }

    pub(crate) fn add_usage(&mut self, usage: CodexUsageBucket, tier: Option<CodexServiceTier>) {
        self.usage += usage;
        if let Some(tier) = tier {
            self.recorded_usage[tier] += usage;
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

macro_rules! codex_record {
    ($name:ident { $($field:ident: $ty:ty = $key:literal => $variant:ident, $read:ident),* $(,)? }) => {
        #[derive(Default)]
        pub(super) struct $name<'a> {
            $(pub(super) $field: $ty,)*
        }

        impl<'de: 'a, 'a> Deserialize<'de> for $name<'a> {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct RecordVisitor;
                impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                    type Value = $name<'de>;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("a Codex record")
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        #[derive(Deserialize)]
                        #[serde(field_identifier)]
                        enum Field {
                            $(#[serde(rename = $key)] $variant,)*
                            #[serde(other)]
                            Unknown,
                        }
                        let mut record = $name::default();
                        while let Some(field) = map.next_key()? {
                            match field {
                                $(Field::$variant => {
                                    record.$field = map.next_value::<$read<_>>()?.0;
                                },)*
                                Field::Unknown => { map.next_value::<serde::de::IgnoredAny>()?; }
                            }
                        }
                        Ok(record)
                    }
                }
                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
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

struct DecodedField<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for DecodedField<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

struct LossyObject<T>(Option<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for LossyObject<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_optional_object_lossy(deserializer).map(Self)
    }
}

struct LossyRawUsage<T>(T);
impl<'de> Deserialize<'de> for LossyRawUsage<CodexUsageCandidate> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = deserialize_optional_object_lossy::<D, CodexRawUsageFields>(deserializer)?;
        Ok(Self(match fields {
            Some(fields) if fields.invalid_fields => CodexUsageCandidate::Malformed,
            Some(fields) => CodexUsageCandidate::Decoded(CodexRawUsage::from_fields(fields)),
            None => CodexUsageCandidate::Missing,
        }))
    }
}

struct LossyString<T>(T);
impl<'de: 'a, 'a> Deserialize<'de> for LossyString<Option<Cow<'a, str>>> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringVisitor;
        impl<'de> serde::de::Visitor<'de> for StringVisitor {
            type Value = Option<Cow<'de, str>>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an optional string")
            }
            fn visit_borrowed_str<E: serde::de::Error>(
                self,
                value: &'de str,
            ) -> Result<Self::Value, E> {
                Ok(Some(Cow::Borrowed(value)))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Some(Cow::Owned(value.to_string())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(Some(Cow::Owned(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))?;
                Ok(None)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::SeqAccessDeserializer::new(
                    seq,
                ))?;
                Ok(None)
            }
        }
        deserializer.deserialize_any(StringVisitor).map(Self)
    }
}

struct LossyTimestamp<T>(T);
impl<'de: 'a, 'a> Deserialize<'de> for LossyTimestamp<Option<CodexTimestamp<'a>>> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimestampVisitor;
        impl<'de> serde::de::Visitor<'de> for TimestampVisitor {
            type Value = Option<CodexTimestamp<'de>>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an optional timestamp")
            }
            fn visit_borrowed_str<E: serde::de::Error>(
                self,
                value: &'de str,
            ) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::String(Cow::Borrowed(value))))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::String(Cow::Owned(value.to_string()))))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::Number(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::Invalid))
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::Invalid))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Some(CodexTimestamp::Invalid))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))?;
                Ok(Some(CodexTimestamp::Invalid))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::SeqAccessDeserializer::new(
                    seq,
                ))?;
                Ok(Some(CodexTimestamp::Invalid))
            }
        }
        deserializer.deserialize_any(TimestampVisitor).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CodexRawUsageFields {
    input_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    input: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    output_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    output: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    total_tokens: Option<u64>,
    invalid_fields: bool,
}

impl<'de> Deserialize<'de> for CodexRawUsageFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UsageFieldsVisitor;
        impl<'de> serde::de::Visitor<'de> for UsageFieldsVisitor {
            type Value = CodexRawUsageFields;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("Codex token counts")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                #[derive(Deserialize)]
                #[serde(field_identifier)]
                enum Field {
                    #[serde(rename = "input_tokens")]
                    InputTokens,
                    #[serde(rename = "prompt_tokens")]
                    PromptTokens,
                    #[serde(rename = "input")]
                    Input,
                    #[serde(rename = "cached_input_tokens")]
                    CachedInputTokens,
                    #[serde(rename = "cache_read_input_tokens")]
                    CacheReadInputTokens,
                    #[serde(rename = "cache_creation_input_tokens")]
                    CacheCreationInputTokens,
                    #[serde(rename = "cache_write_input_tokens")]
                    CacheWriteInputTokens,
                    #[serde(rename = "cached_tokens")]
                    CachedTokens,
                    #[serde(rename = "output_tokens")]
                    OutputTokens,
                    #[serde(rename = "completion_tokens")]
                    CompletionTokens,
                    #[serde(rename = "output")]
                    Output,
                    #[serde(rename = "reasoning_output_tokens")]
                    ReasoningOutputTokens,
                    #[serde(rename = "reasoning_tokens")]
                    ReasoningTokens,
                    #[serde(rename = "total_tokens")]
                    TotalTokens,
                    #[serde(other)]
                    Unknown,
                }
                let mut fields = CodexRawUsageFields::default();
                while let Some(field) = map.next_key()? {
                    let target = match field {
                        Field::InputTokens => &mut fields.input_tokens,
                        Field::PromptTokens => &mut fields.prompt_tokens,
                        Field::Input => &mut fields.input,
                        Field::CachedInputTokens => &mut fields.cached_input_tokens,
                        Field::CacheReadInputTokens => &mut fields.cache_read_input_tokens,
                        Field::CacheCreationInputTokens => &mut fields.cache_creation_input_tokens,
                        Field::CacheWriteInputTokens => &mut fields.cache_write_input_tokens,
                        Field::CachedTokens => &mut fields.cached_tokens,
                        Field::OutputTokens => &mut fields.output_tokens,
                        Field::CompletionTokens => &mut fields.completion_tokens,
                        Field::Output => &mut fields.output,
                        Field::ReasoningOutputTokens => &mut fields.reasoning_output_tokens,
                        Field::ReasoningTokens => &mut fields.reasoning_tokens,
                        Field::TotalTokens => &mut fields.total_tokens,
                        Field::Unknown => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            continue;
                        }
                    };
                    let OptionalU64(value, invalid) = map.next_value()?;
                    fields.invalid_fields |= invalid;
                    *target = value;
                }
                Ok(fields)
            }
        }
        deserializer.deserialize_map(UsageFieldsVisitor)
    }
}

impl<'de> Deserialize<'de> for CodexRawUsage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = CodexRawUsageFields::deserialize(deserializer)?;
        if fields.invalid_fields {
            return Err(serde::de::Error::custom("invalid usage fields"));
        }
        Ok(Self::from_fields(fields))
    }
}

impl CodexRawUsage {
    fn from_fields(fields: CodexRawUsageFields) -> Self {
        let input = fields
            .input_tokens
            .or(fields.prompt_tokens)
            .or(fields.input)
            .unwrap_or(0);
        let output = fields
            .output_tokens
            .or(fields.completion_tokens)
            .or(fields.output)
            .unwrap_or(0);
        let reasoning = fields
            .reasoning_output_tokens
            .or(fields.reasoning_tokens)
            .unwrap_or(0);
        let cached_input = fields
            .cached_input_tokens
            .or(fields.cache_read_input_tokens)
            .or(fields.cached_tokens)
            .unwrap_or(0)
            .min(input);
        let cache_creation = fields
            .cache_write_input_tokens
            .or(fields.cache_creation_input_tokens)
            .unwrap_or(0)
            .min(input.saturating_sub(cached_input));
        Self {
            input_tokens: input,
            cached_input_tokens: cached_input,
            cache_creation_tokens: cache_creation,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            // NOTE(2026-07-27, #1499): Codex includes reasoning in `output_tokens`. Saved `codex exec --json` usage can omit the total or report zero with nonzero components, so derive it from input and output.
            total_tokens: fields
                .total_tokens
                .filter(|total| *total > 0)
                .unwrap_or_else(|| input.saturating_add(output)),
        }
    }
}

fn deserialize_optional_object_lossy<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct OptionalObjectVisitor<T>(PhantomData<T>);

    impl<'de, T> serde::de::Visitor<'de> for OptionalObjectVisitor<T>
    where
        T: serde::Deserialize<'de>,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an optional object")
        }

        fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserialize_optional_object_lossy(deserializer)
        }

        fn visit_map<A>(self, map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }

        fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_i64<E>(self, _value: i64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_u64<E>(self, _value: u64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_str<E>(self, _value: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            while sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    }

    deserializer.deserialize_any(OptionalObjectVisitor(PhantomData))
}

struct OptionalU64(Option<u64>, bool);
impl<'de> Deserialize<'de> for OptionalU64 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OptionalU64Visitor;

        impl<'de> serde::de::Visitor<'de> for OptionalU64Visitor {
            type Value = OptionalU64;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an optional unsigned integer")
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false))
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                OptionalU64::deserialize(deserializer)
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(Some(value), false))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(value.trim().parse::<u64>().ok(), false))
            }

            fn visit_borrowed_str<E>(self, value: &'de str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_str(value)
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_str(&value)
            }

            fn visit_i64<E>(self, _value: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false))
            }

            fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))?;
                Ok(OptionalU64(None, true))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                serde::de::IgnoredAny::deserialize(serde::de::value::SeqAccessDeserializer::new(
                    seq,
                ))?;
                Ok(OptionalU64(None, true))
            }

            fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false))
            }
        }

        deserializer.deserialize_any(OptionalU64Visitor)
    }
}

#[cfg(test)]
mod tests;
