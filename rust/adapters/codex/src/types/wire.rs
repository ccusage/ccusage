use std::{borrow::Cow, marker::PhantomData};

use serde::Deserialize;

use super::{CodexRawUsage, CodexTimestamp, CodexUsageCandidate};

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
                let fields = UsageFieldsVisitor::<CheckedAny>(PhantomData).visit_map(map)?;
                Ok(if fields.invalid_members == 0 {
                    CodexUsageCandidate::Decoded(CodexRawUsage::from_fields(fields))
                } else {
                    CodexUsageCandidate::Malformed
                })
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
pub(super) use codex_record;

pub(super) struct DecodedField<T>(pub(super) T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for DecodedField<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

pub(super) struct LossyObject<T>(pub(super) Option<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for LossyObject<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_optional_object_lossy(deserializer).map(Self)
    }
}

pub(super) struct LossyRawUsage<T>(pub(super) T);
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

pub(super) struct LossyString<T>(pub(super) T);
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

pub(super) struct LossyTimestamp<T>(pub(super) T);
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
    invalid_members: u16,
}

impl<'de> Deserialize<'de> for CodexRawUsageFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(UsageFieldsVisitor::<serde::de::IgnoredAny>(PhantomData))
    }
}

// NOTE(2026-11-20): A top-level `usage` object decodes as a `serde_json::Value` would: every string and number in it is checked, and the last of duplicate keys wins. `CheckedAny` and `invalid_members` keep that without building the value; `IgnoredAny` would accept lines that decoding as a value rejects.
struct UsageFieldsVisitor<I>(PhantomData<I>);

impl<'de, I: Deserialize<'de>> serde::de::Visitor<'de> for UsageFieldsVisitor<I> {
    type Value = CodexRawUsageFields;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Codex token counts")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        #[derive(Clone, Copy, Deserialize)]
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
                    map.next_value::<I>()?;
                    continue;
                }
            };
            let OptionalU64(value, invalid, _) = map.next_value::<OptionalU64<I>>()?;
            fields.invalid_fields |= invalid;
            let member = 1 << field as u16;
            if invalid {
                fields.invalid_members |= member;
            } else {
                fields.invalid_members &= !member;
            }
            *target = value;
        }
        Ok(fields)
    }
}

struct CheckedAny;

impl<'de> Deserialize<'de> for CheckedAny {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CheckedVisitor;
        impl<'de> serde::de::Visitor<'de> for CheckedVisitor {
            type Value = CheckedAny;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(CheckedAny)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<CheckedAny>()?.is_some() {}
                Ok(CheckedAny)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map.next_entry::<CheckedAny, CheckedAny>()?.is_some() {}
                Ok(CheckedAny)
            }
        }
        deserializer.deserialize_any(CheckedVisitor)
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

struct OptionalU64<I>(Option<u64>, bool, PhantomData<I>);
impl<'de, I: Deserialize<'de>> Deserialize<'de> for OptionalU64<I> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OptionalU64Visitor<I>(PhantomData<I>);

        impl<'de, I: Deserialize<'de>> serde::de::Visitor<'de> for OptionalU64Visitor<I> {
            type Value = OptionalU64<I>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an optional unsigned integer")
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false, PhantomData))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false, PhantomData))
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                OptionalU64::<I>::deserialize(deserializer)
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(Some(value), false, PhantomData))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(
                    value.trim().parse::<u64>().ok(),
                    false,
                    PhantomData,
                ))
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
                Ok(OptionalU64(None, false, PhantomData))
            }

            fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false, PhantomData))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                I::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(OptionalU64(None, true, PhantomData))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                I::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))?;
                Ok(OptionalU64(None, true, PhantomData))
            }

            fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OptionalU64(None, false, PhantomData))
            }
        }

        deserializer.deserialize_any(OptionalU64Visitor(PhantomData))
    }
}
