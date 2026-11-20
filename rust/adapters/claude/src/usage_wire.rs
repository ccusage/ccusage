use std::borrow::Cow;

use ccusage_adapter_common::json::JsonText;
use ccusage_core::{CacheCreationRaw, Speed, TokenUsageRaw};
use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeOwned, DeserializeSeed, MapAccess, Visitor,
        value::{BorrowedStrDeserializer, MapAccessDeserializer, StringDeserializer},
    },
};

mod iterations;

use iterations::{Iterations, Lenient};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct WireCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireSpeed {
    Standard,
    Fast,
}

#[derive(Deserialize)]
struct TokenUsageRawJson {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    speed: Option<WireSpeed>,
    #[serde(default)]
    cache_creation: Option<WireCacheCreation>,
    #[serde(default)]
    iterations: Lenient<Iterations>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(from = "TokenUsageRawJson")]
pub(crate) struct IteratedTokenUsage {
    pub(crate) usage: TokenUsageRaw,
    pub(crate) has_advisor_iteration: bool,
}

impl From<TokenUsageRawJson> for IteratedTokenUsage {
    fn from(json: TokenUsageRawJson) -> Self {
        Self {
            has_advisor_iteration: json.iterations.0.has_advisor_message,
            usage: json.into(),
        }
    }
}

impl From<TokenUsageRawJson> for TokenUsageRaw {
    fn from(json: TokenUsageRawJson) -> Self {
        Self {
            input_tokens: json.input_tokens,
            output_tokens: json.output_tokens,
            cache_creation_input_tokens: json.cache_creation_input_tokens,
            cache_read_input_tokens: json.cache_read_input_tokens,
            speed: json.speed.map(|speed| match speed {
                WireSpeed::Standard => Speed::Standard,
                WireSpeed::Fast => Speed::Fast,
            }),
            // NOTE(2026-11-16, #2085): In Claude Code advisor turns, the top-level `cache_creation` split covers the first `message` iteration while the total covers them all. Sum the per-message splits when complete.
            cache_creation: json
                .iterations
                .0
                .message_cache_creation
                .or(json.cache_creation)
                .map(|split| {
                    CacheCreationRaw::from_splits(
                        split.ephemeral_5m_input_tokens,
                        split.ephemeral_1h_input_tokens,
                    )
                }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rejection {
    // NOTE(2026-11-16): Claude Code writes synthetic error, refusal and interrupt records with null speed and zero tokens. Skip these quietly to avoid invalid-data warnings during normal operation.
    NullContract,
    Malformed,
}

pub(crate) fn deserialize_usage_line<T: DeserializeOwned>(
    line: &[u8],
) -> std::result::Result<T, Rejection> {
    ccusage_adapter_common::json::parse(line).map_err(|_| Rejection::Malformed)
}

pub(crate) fn deserialize_borrowed_usage_line<'a, T: Deserialize<'a>>(
    line: &'a JsonText<'a>,
) -> std::result::Result<T, Rejection> {
    line.parse().map_err(|_| Rejection::Malformed)
}

#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct Text<'a>(#[serde(borrow)] pub(crate) Cow<'a, str>);

pub(crate) type Nullable<T> = Option<Option<T>>;

pub(super) fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Nullable<T>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(crate) fn non_null<T>(field: Nullable<T>) -> std::result::Result<Option<T>, Rejection> {
    match field {
        Some(None) => Err(Rejection::NullContract),
        Some(value) => Ok(value),
        None => Ok(None),
    }
}

#[derive(Debug)]
pub(crate) struct GuardedUsage {
    pub(super) usage: IteratedTokenUsage,
    pub(super) has_null: bool,
}

impl<'de> Deserialize<'de> for GuardedUsage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct UsageVisitor;
        impl<'de> Visitor<'de> for UsageVisitor {
            type Value = GuardedUsage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a usage object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut fields = UsageFields {
                    map,
                    pending: None,
                    has_null: false,
                };
                let usage =
                    IteratedTokenUsage::deserialize(MapAccessDeserializer::new(&mut fields))?;
                Ok(GuardedUsage {
                    usage,
                    has_null: fields.has_null,
                })
            }
        }
        deserializer.deserialize_map(UsageVisitor)
    }
}

fn is_guarded_usage_member(key: &str) -> bool {
    matches!(
        key,
        "speed" | "cache_creation_input_tokens" | "cache_read_input_tokens"
    )
}

struct UsageFields<A> {
    map: A,
    pending: Option<serde_json::Value>,
    has_null: bool,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for UsageFields<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> std::result::Result<Option<K::Value>, Self::Error> {
        loop {
            let Some(key) = self.map.next_key::<UsageKey<'de>>()? else {
                return Ok(None);
            };
            if !is_guarded_usage_member(key.as_str()) {
                return key.into_seed(seed).map(Some);
            }
            let value = self.map.next_value::<serde_json::Value>()?;
            if value.is_null() {
                self.has_null = true;
            } else {
                self.pending = Some(value);
                return key.into_seed(seed).map(Some);
            }
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(value) => seed.deserialize(value).map_err(serde::de::Error::custom),
            None => self.map.next_value_seed(seed),
        }
    }
}

enum UsageKey<'de> {
    Borrowed(&'de str),
    Owned(String),
}

impl<'de> UsageKey<'de> {
    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(key) => key,
            Self::Owned(key) => key,
        }
    }

    fn into_seed<K: DeserializeSeed<'de>, E: serde::de::Error>(
        self,
        seed: K,
    ) -> std::result::Result<K::Value, E> {
        match self {
            Self::Borrowed(key) => seed.deserialize(BorrowedStrDeserializer::new(key)),
            Self::Owned(key) => seed.deserialize(StringDeserializer::new(key)),
        }
    }
}

impl<'de> Deserialize<'de> for UsageKey<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = UsageKey<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a usage member name")
            }

            fn visit_borrowed_str<E>(self, key: &'de str) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Borrowed(key))
            }

            fn visit_str<E>(self, key: &str) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Owned(key.to_owned()))
            }

            fn visit_string<E>(self, key: String) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Owned(key))
            }
        }
        deserializer.deserialize_identifier(KeyVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_message_iteration_breakdowns_instead_of_the_truncated_top_level() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
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

        assert_eq!(usage.usage.cache_creation_token_count(), 12323);
    }

    #[test]
    fn keeps_the_top_level_breakdown_without_message_iteration_breakdowns() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
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

        assert_eq!(usage.usage.cache_creation_token_count(), 11142);
    }

    #[test]
    fn keeps_the_top_level_breakdown_when_a_message_iteration_lacks_its_own() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
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

        assert_eq!(usage.usage.cache_creation_token_count(), 11142);
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
            let usage = serde_json::from_str::<IteratedTokenUsage>(&line)
                .unwrap_or_else(|error| panic!("{iterations}: {error}"));

            assert_eq!(usage.usage.cache_creation_token_count(), 7, "{iterations}");
            assert_eq!(
                usage
                    .usage
                    .cache_creation
                    .map(|split| split.one_hour_tokens()),
                Some(4),
                "{iterations}"
            );
        }
    }

    #[test]
    fn notes_an_advisor_message_iteration_however_its_type_is_spelled() {
        let escaped_type = ["advisor", "\\", "u005fmessage"].concat();
        for (iterations, has_advisor) in [
            r#"[{"type": "advisor_message", "model": "m"}]"#.to_owned(),
            format!(r#"[{{"type": "message"}}, {{"type": "{escaped_type}"}}]"#),
        ]
        .into_iter()
        .map(|iterations| (iterations, true))
        .chain(
            [
                r#"[{"type": "message"}, {"kind": "advisor_message"}]"#,
                r#"[{"type": ["advisor_message"]}, "advisor_message"]"#,
                r#""advisor_message""#,
            ]
            .map(|iterations| (iterations.to_owned(), false)),
        ) {
            let line =
                format!(r#"{{"input_tokens":1,"output_tokens":2,"iterations":{iterations}}}"#);
            let usage = serde_json::from_str::<IteratedTokenUsage>(&line)
                .unwrap_or_else(|error| panic!("{iterations}: {error}"));

            assert_eq!(usage.has_advisor_iteration, has_advisor, "{iterations}");
            assert_eq!(usage.usage.input_tokens, 1, "{iterations}");
        }
    }

    #[test]
    fn takes_the_split_from_message_iterations_without_a_top_level_one() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
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

        assert_eq!(usage.usage.cache_creation_token_count(), 30);
        assert_eq!(
            usage
                .usage
                .cache_creation
                .map(|split| split.one_hour_tokens()),
            Some(20)
        );
    }
}
