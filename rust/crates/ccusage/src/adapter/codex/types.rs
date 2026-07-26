use std::{borrow::Cow, marker::PhantomData};

use serde::Deserialize;

use crate::CodexRawUsage;

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
        Ok(Self {
            input_tokens: input,
            cached_input_tokens: fields
                .cached_input_tokens
                .or(fields.cache_read_input_tokens)
                .or(fields.cached_tokens)
                .unwrap_or(0),
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            total_tokens: fields
                .total_tokens
                .filter(|total| *total > 0 || input + output + reasoning == 0)
                .unwrap_or(input + output + reasoning),
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
