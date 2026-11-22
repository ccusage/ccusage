use std::{fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

pub fn lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    lenient_optional_u64(deserializer).map(Option::unwrap_or_default)
}

pub fn lenient_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_u64))
}

pub fn lenient_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_i64))
}

pub fn lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_f64))
}

pub fn lenient_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(value @ serde_json::Value::Object(_)) => serde_json::from_value(value).ok(),
        _ => None,
    })
}

pub fn lenient_array<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .into_iter()
                .filter_map(|item| serde_json::from_value(item).ok())
                .collect(),
        ),
        _ => None,
    })
}

pub fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(lenient_array(deserializer)?.unwrap_or_default())
}

pub fn non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(ccusage_core::non_empty_json_string(value.as_ref()))
}

macro_rules! skip_scalars {
    ($empty:expr) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($empty)
        }
    };
}

pub trait Present {
    fn is_present(&self) -> bool;
}

pub trait FieldName {
    const NAME: &'static str;
}

/// Reads the first usable value of a field. Use lenient types for `T`, such as
/// [`LenientString`] or [`LenientSeq`], because Serde cannot recover from an
/// error midway through a streamed object.
pub struct FirstField<N, T>(pub Option<T>, PhantomData<N>);

impl<N, T> Default for FirstField<N, T> {
    fn default() -> Self {
        Self(None, PhantomData)
    }
}

impl<N, T: Present> Present for FirstField<N, T> {
    fn is_present(&self) -> bool {
        self.0.as_ref().is_some_and(Present::is_present)
    }
}

struct IsField<N>(bool, PhantomData<N>);

impl<'de, N: FieldName> Deserialize<'de> for IsField<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor<N>(PhantomData<N>);

        impl<N: FieldName> Visitor<'_> for KeyVisitor<N> {
            type Value = IsField<N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object key")
            }

            fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
                Ok(IsField(key == N::NAME, PhantomData))
            }
        }

        deserializer.deserialize_str(KeyVisitor(PhantomData))
    }
}

impl<'de, N: FieldName, T: Deserialize<'de> + Present> Deserialize<'de> for FirstField<N, T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor<N, T>(PhantomData<(N, T)>);

        impl<'de, N: FieldName, T: Deserialize<'de> + Present> Visitor<'de> for FieldVisitor<N, T> {
            type Value = FirstField<N, T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                std::iter::from_fn(|| {
                    map.next_key::<IsField<N>>()
                        .and_then(|key| {
                            key.map(|IsField(matches, _)| {
                                if matches {
                                    map.next_value::<T>().map(Some)
                                } else {
                                    map.next_value::<IgnoredAny>().map(|_| None)
                                }
                            })
                            .transpose()
                        })
                        .transpose()
                })
                .try_fold(None, |found: Option<T>, value| {
                    value.map(|value| found.or(value.filter(Present::is_present)))
                })
                .map(|found| FirstField(found, PhantomData))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_seq(seq).map(|_| FirstField::default())
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(FirstField::default())
            }

            skip_scalars!(FirstField::default());
        }

        deserializer.deserialize_any(FieldVisitor(PhantomData))
    }
}

#[derive(Default)]
pub struct LenientString(pub Option<String>);

impl Present for LenientString {
    fn is_present(&self) -> bool {
        self.0.is_some()
    }
}

impl<'de> Deserialize<'de> for LenientString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringVisitor;

        impl<'de> Visitor<'de> for StringVisitor {
            type Value = LenientString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(LenientString(Some(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(LenientString(Some(value)))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_map(map).map(|_| LenientString(None))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_seq(seq).map(|_| LenientString(None))
            }

            skip_scalars!(LenientString(None));
        }

        deserializer.deserialize_any(StringVisitor)
    }
}

pub struct LenientSeq<T>(pub Vec<T>);

impl<T> Present for LenientSeq<T> {
    fn is_present(&self) -> bool {
        !self.0.is_empty()
    }
}

impl<T> Default for LenientSeq<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for LenientSeq<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SeqVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for SeqVisitor<T> {
            type Value = LenientSeq<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                std::iter::from_fn(|| seq.next_element().transpose())
                    .collect::<Result<_, _>>()
                    .map(LenientSeq)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_map(map).map(|_| LenientSeq(Vec::new()))
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(LenientSeq(Vec::new()))
            }

            skip_scalars!(LenientSeq(Vec::new()));
        }

        deserializer.deserialize_any(SeqVisitor(PhantomData))
    }
}
