use std::{fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use super::WireCacheCreation;

#[derive(Default)]
pub(super) struct Iterations {
    pub(super) message_cache_creation: Option<WireCacheCreation>,
    pub(super) has_advisor_message: bool,
}

#[derive(Default)]
pub(super) struct Lenient<T>(pub(super) T);

trait LenientValue: Default {
    fn from_str(_value: &str) -> Self {
        Self::default()
    }

    fn from_u64(_value: u64) -> Self {
        Self::default()
    }

    fn from_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Self::default())
    }

    fn from_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Self::default())
    }
}

impl<'de, T: LenientValue> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(LenientVisitor(PhantomData))
            .map(Lenient)
    }
}

struct LenientVisitor<T>(PhantomData<T>);

impl<'de, T: LenientValue> Visitor<'de> for LenientVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_u64<E>(self, value: u64) -> Result<T, E> {
        Ok(T::from_u64(value))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_str<E>(self, value: &str) -> Result<T, E> {
        Ok(T::from_str(value))
    }

    fn visit_bytes<E>(self, _value: &[u8]) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_unit<E>(self) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_none<E>(self) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        T::from_seq(seq)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::from_map(map)
    }
}

impl LenientValue for Iterations {
    fn from_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        let mut total = WireCacheCreation::default();
        let mut has_message = false;
        let mut every_message_has_split = true;
        let mut has_advisor_message = false;
        while let Some(Lenient(iteration)) = seq.next_element::<Lenient<Iteration>>()? {
            has_advisor_message |= matches!(iteration.kind, IterationKind::AdvisorMessage);
            if !matches!(iteration.kind, IterationKind::Message) {
                continue;
            }
            has_message = true;
            match iteration.cache_creation {
                Some(split) => {
                    total.ephemeral_5m_input_tokens = total
                        .ephemeral_5m_input_tokens
                        .saturating_add(split.ephemeral_5m_input_tokens);
                    total.ephemeral_1h_input_tokens = total
                        .ephemeral_1h_input_tokens
                        .saturating_add(split.ephemeral_1h_input_tokens);
                }
                None => every_message_has_split = false,
            }
        }
        Ok(Self {
            message_cache_creation: (has_message && every_message_has_split).then_some(total),
            has_advisor_message,
        })
    }
}

#[derive(Default)]
struct Iteration {
    kind: IterationKind,
    cache_creation: Option<WireCacheCreation>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum IterationKey {
    Type,
    CacheCreation,
    #[serde(other)]
    Other,
}

impl LenientValue for Iteration {
    fn from_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut iteration = Self::default();
        while let Some(key) = map.next_key::<IterationKey>()? {
            match key {
                IterationKey::Type => {
                    iteration.kind = map.next_value::<Lenient<IterationKind>>()?.0;
                }
                IterationKey::CacheCreation => {
                    iteration.cache_creation = map.next_value::<Lenient<CacheCreationSplit>>()?.0.0;
                }
                IterationKey::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(iteration)
    }
}

#[derive(Default)]
enum IterationKind {
    Message,
    AdvisorMessage,
    #[default]
    Other,
}

impl LenientValue for IterationKind {
    fn from_str(value: &str) -> Self {
        match value {
            "message" => Self::Message,
            "advisor_message" => Self::AdvisorMessage,
            _ => Self::Other,
        }
    }
}

#[derive(Default)]
struct CacheCreationSplit(Option<WireCacheCreation>);

#[derive(Deserialize)]
#[serde(field_identifier)]
enum CacheCreationKey {
    #[serde(rename = "ephemeral_5m_input_tokens")]
    FiveMinutes,
    #[serde(rename = "ephemeral_1h_input_tokens")]
    OneHour,
    #[serde(other)]
    Other,
}

impl LenientValue for CacheCreationSplit {
    fn from_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut five_minutes = Some(0);
        let mut one_hour = Some(0);
        while let Some(key) = map.next_key::<CacheCreationKey>()? {
            match key {
                CacheCreationKey::FiveMinutes => {
                    five_minutes = map.next_value::<Lenient<TokenCount>>()?.0.0;
                }
                CacheCreationKey::OneHour => {
                    one_hour = map.next_value::<Lenient<TokenCount>>()?.0.0;
                }
                CacheCreationKey::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Self(five_minutes.zip(one_hour).map(
            |(ephemeral_5m_input_tokens, ephemeral_1h_input_tokens)| WireCacheCreation {
                ephemeral_5m_input_tokens,
                ephemeral_1h_input_tokens,
            },
        )))
    }
}

#[derive(Default)]
struct TokenCount(Option<u64>);

impl LenientValue for TokenCount {
    fn from_u64(value: u64) -> Self {
        Self(Some(value))
    }
}
