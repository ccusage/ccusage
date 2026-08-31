use std::{borrow::Cow, collections::BTreeMap, marker::PhantomData};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexRawUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexServiceTier {
    Standard,
    Fast,
}

pub const fn merge_codex_service_tiers(
    current: Option<CodexServiceTier>,
    incoming: Option<CodexServiceTier>,
) -> Option<CodexServiceTier> {
    // Preserve explicit metadata over an unclassified copy. If two copied
    // records conflict, Standard keeps the cost estimate conservative and
    // makes the result independent of file order or worker scheduling.
    match (current, incoming) {
        (Some(CodexServiceTier::Standard), _) | (_, Some(CodexServiceTier::Standard)) => {
            Some(CodexServiceTier::Standard)
        }
        (Some(CodexServiceTier::Fast), _) | (_, Some(CodexServiceTier::Fast)) => {
            Some(CodexServiceTier::Fast)
        }
        (None, None) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexTokenUsageEvent {
    pub session_id: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub is_fallback_model: bool,
    pub service_tier: Option<CodexServiceTier>,
    pub source: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct CodexModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    // Portion of the sums above that came from long-context requests (input
    // above the OpenAI 272K threshold). OpenAI decides the pricing tier per
    // request and bills the whole request at long-context rates, so the split
    // has to be tracked while events are aggregated; it cannot be derived
    // from the summed totals afterwards.
    pub long_context_input_tokens: u64,
    pub long_context_cached_input_tokens: u64,
    pub long_context_cache_creation_tokens: u64,
    pub long_context_output_tokens: u64,
    pub recorded_standard_usage: CodexUsageBucket,
    pub recorded_fast_usage: CodexUsageBucket,
    pub is_fallback: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CodexSourceUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub models: BTreeMap<String, CodexModelUsage>,
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
    pub sources: BTreeMap<String, CodexSourceUsage>,
    pub last_activity: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CodexSessionLogEntry<'a> {
    #[serde(rename = "type", borrow, default)]
    pub(super) entry_type: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    pub(super) timestamp: Option<CodexTimestamp<'a>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) payload: Option<CodexPayload<'a>>,
}

#[derive(Deserialize)]
pub(super) struct CodexLogEntry<'a> {
    #[serde(borrow, default)]
    pub(super) timestamp: Option<CodexTimestamp<'a>>,
    #[serde(rename = "created_at", borrow, default)]
    pub(super) created_at: Option<CodexTimestamp<'a>>,
    #[serde(rename = "createdAt", borrow, default)]
    pub(super) created_at_camel: Option<CodexTimestamp<'a>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) data: Option<CodexResultFields<'a>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) result: Option<CodexResultFields<'a>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) response: Option<CodexResultFields<'a>>,
    #[serde(default, deserialize_with = "deserialize_optional_object_lossy")]
    pub(super) usage: Option<CodexRawUsage>,
    #[serde(borrow, default)]
    pub(super) model: Option<Cow<'a, str>>,
    #[serde(rename = "model_name", borrow, default)]
    pub(super) model_name: Option<Cow<'a, str>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) metadata: Option<CodexModelMetadata<'a>>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum CodexTimestamp<'a> {
    String(Cow<'a, str>),
    Number(u64),
}

#[derive(Default, Deserialize)]
pub(super) struct CodexPayload<'a> {
    #[serde(rename = "type", borrow, default)]
    pub(super) payload_type: Option<Cow<'a, str>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) info: Option<CodexInfo<'a>>,
    #[serde(borrow, default)]
    pub(super) model: Option<Cow<'a, str>>,
    #[serde(rename = "model_name", borrow, default)]
    pub(super) model_name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    pub(super) originator: Option<Cow<'a, str>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) metadata: Option<CodexModelMetadata<'a>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) thread_settings: Option<CodexThreadSettings<'a>>,
}

#[derive(Deserialize)]
pub(super) struct CodexThreadSettings<'a> {
    #[serde(borrow, default)]
    pub(super) service_tier: Option<Cow<'a, str>>,
}

#[derive(Default, Deserialize)]
pub(super) struct CodexInfo<'a> {
    #[serde(default, deserialize_with = "deserialize_optional_object_lossy")]
    pub(super) last_token_usage: Option<CodexRawUsage>,
    #[serde(default, deserialize_with = "deserialize_optional_object_lossy")]
    pub(super) total_token_usage: Option<CodexRawUsage>,
    #[serde(borrow, default)]
    pub(super) model: Option<Cow<'a, str>>,
    #[serde(rename = "model_name", borrow, default)]
    pub(super) model_name: Option<Cow<'a, str>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) metadata: Option<CodexModelMetadata<'a>>,
}

#[derive(Default, Deserialize)]
pub(super) struct CodexResultFields<'a> {
    #[serde(borrow, default)]
    pub(super) timestamp: Option<CodexTimestamp<'a>>,
    #[serde(rename = "created_at", borrow, default)]
    pub(super) created_at: Option<CodexTimestamp<'a>>,
    #[serde(rename = "createdAt", borrow, default)]
    pub(super) created_at_camel: Option<CodexTimestamp<'a>>,
    #[serde(default, deserialize_with = "deserialize_optional_object_lossy")]
    pub(super) usage: Option<CodexRawUsage>,
    #[serde(borrow, default)]
    pub(super) model: Option<Cow<'a, str>>,
    #[serde(rename = "model_name", borrow, default)]
    pub(super) model_name: Option<Cow<'a, str>>,
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_optional_object_lossy"
    )]
    pub(super) metadata: Option<CodexModelMetadata<'a>>,
}

#[derive(Deserialize)]
pub(super) struct CodexModelMetadata<'a> {
    #[serde(borrow, default)]
    pub(super) model: Option<Cow<'a, str>>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
struct CodexRawUsageFields {
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    prompt_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    input: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    cached_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    cache_write_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    cached_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    completion_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    output: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    reasoning_output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    reasoning_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_lossy")]
    total_tokens: Option<u64>,
}

impl<'de> Deserialize<'de> for CodexRawUsage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = CodexRawUsageFields::deserialize(deserializer)?;
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
        Ok(Self {
            input_tokens: input,
            cached_input_tokens: cached_input,
            cache_creation_tokens: cache_creation,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            // Codex reports reasoning as a subset of output, so a derived total
            // must not add it on top. A recorded zero total means the field is
            // unusable rather than that the turn spent nothing, so derive it too.
            total_tokens: fields
                .total_tokens
                .filter(|total| *total > 0)
                .unwrap_or_else(|| input.saturating_add(output)),
        })
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

fn deserialize_optional_u64_lossy<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct OptionalU64Visitor;

    impl<'de> serde::de::Visitor<'de> for OptionalU64Visitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an optional unsigned integer")
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
            deserialize_optional_u64_lossy(deserializer)
        }

        fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(value))
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(value.trim().parse::<u64>().ok())
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
            Ok(None)
        }

        fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }
    }

    deserializer.deserialize_any(OptionalU64Visitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_total_tokens_without_double_counting_reasoning() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 100,
                "output_tokens": 50,
                "reasoning_output_tokens": 20
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn derives_explicit_zero_total_tokens_without_double_counting_reasoning() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 9,
                "output_tokens": 4,
                "reasoning_output_tokens": 1,
                "total_tokens": 0
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.total_tokens, 13);
    }

    #[test]
    fn keeps_a_recorded_total() {
        // Deliberately not input plus output, so deriving instead of preserving
        // the recorded value cannot pass this test.
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 100,
                "output_tokens": 50,
                "reasoning_output_tokens": 20,
                "total_tokens": 151
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.total_tokens, 151);
    }

    #[test]
    fn saturates_a_derived_total_instead_of_overflowing() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 18446744073709551615,
                "output_tokens": 5
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.total_tokens, u64::MAX);
    }

    #[test]
    fn derives_total_tokens_from_openai_field_spellings() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "prompt_tokens": 50,
                "cached_tokens": 5,
                "completion_tokens": 12,
                "reasoning_tokens": 4
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.total_tokens, 62);
    }

    #[test]
    fn normalizes_cache_reads_and_writes_to_the_reported_input_total() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 100,
                "cached_input_tokens": 80,
                "cache_write_input_tokens": 40,
                "output_tokens": 5,
                "total_tokens": 105
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.cached_input_tokens, 80);
        assert_eq!(usage.cache_creation_tokens, 20);
    }

    #[test]
    fn accepts_the_cache_creation_input_tokens_compatibility_spelling() {
        let usage: CodexRawUsage = serde_json::from_str(
            r#"{
                "input_tokens": 100,
                "cached_input_tokens": 20,
                "cache_creation_input_tokens": 30,
                "output_tokens": 5
            }"#,
        )
        .expect("usage should deserialize");

        assert_eq!(usage.cache_creation_tokens, 30);
    }

    #[test]
    fn leaves_an_empty_usage_total_at_zero() {
        let usage: CodexRawUsage =
            serde_json::from_str(r#"{"total_tokens": 0}"#).expect("usage should deserialize");

        assert_eq!(usage.total_tokens, 0);
    }
}
