//! Shared JSONL parsing helpers for agent adapters.
//!
//! Adapters historically parsed each log line into a dynamic
//! [`serde_json::Value`] and then hand-navigated it with `Value::get`. This
//! module centralizes the faster gold-standard approach used by the Claude
//! loader so every adapter shares the same optimizations:
//!
//! 1. Read the whole file once and split it into byte slices with
//!    [`byte_lines`](ccusage_core::fast::byte_lines), avoiding a `String` allocation
//!    per line.
//! 2. Skip lines that cannot possibly match using a precompiled `memmem`
//!    substring prefilter, before any JSON parsing happens.
//! 3. Deserialize the surviving lines directly into a typed struct with
//!    `serde_json::from_slice`, so unused fields are skipped instead of being
//!    materialized into an intermediate `Value` tree.

use std::{fmt, marker::PhantomData, path::Path};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use ccusage_core::{
    Diagnostics, Skip, SkipKind,
    fast::{LinePrefilter, byte_lines},
};

/// Iterate over deserialized JSONL records contained in `content`.
///
/// When `prefilter` is provided, lines that it rejects are skipped before any
/// JSON parsing, mirroring the per-line `memmem` prefilter used by the Claude
/// loader. Build the [`LinePrefilter`] from markers that appear in every line
/// the adapter would accept (for example the required `"usage"` key) so the
/// prefilter never drops a usable record. Pass `None` to parse every line.
///
/// Lines that fail to deserialize into `T` are silently skipped, matching the
/// historical `serde_json::from_str::<Value>(line).ok()` behavior.
///
/// # Examples
///
/// ```ignore
/// #[derive(serde::Deserialize)]
/// struct Record {
///     model: Option<String>,
/// }
///
/// let content = b"{\"model\":\"qwen3-coder\"}\n{}\n";
/// let prefilter = LinePrefilter::all(&[b"model"]);
/// let models: Vec<_> = jsonl::records::<Record>(content, Some(&prefilter))
///     .filter_map(|record| record.model)
///     .collect();
/// assert_eq!(models, ["qwen3-coder"]);
/// ```
pub fn records<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
) -> impl Iterator<Item = T> + 'data
where
    T: DeserializeOwned + 'data,
{
    try_records(content, prefilter).filter_map(|(_, record)| record.ok())
}

/// Like [`records`], but keeps the lines that pass the prefilter and then fail
/// to deserialize, each with its 1-based line number.
///
/// The prefilter selects possible records; it does not prove a record carries
/// usage. A caller decides how to classify a decoding failure.
pub fn try_records<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
) -> impl Iterator<Item = (u32, serde_json::Result<T>)> + 'data
where
    T: DeserializeOwned + 'data,
{
    selected_lines(content, prefilter)
        .map(|(number, line)| (number, serde_json::from_slice::<T>(line)))
}

fn selected_lines<'data>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
) -> impl Iterator<Item = (u32, &'data [u8])> + 'data {
    byte_lines(content)
        .enumerate()
        .filter_map(move |(index, line)| {
            if line
                .iter()
                .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
            {
                return None;
            }
            if let Some(prefilter) = prefilter
                && !prefilter.matches(line)
            {
                return None;
            }
            let number = ccusage_core::Skip::line_number(index);
            Some((number, line))
        })
}

/// Iterates over decoded JSONL records, diagnosing malformed JSON separately
/// from selected records that a typed decoder rejects.
///
/// Lines rejected by `prefilter` are never parsed or counted as skipped. JSON
/// error details retain only the category and source location, not input text.
pub fn records_with_diagnostics<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
    unit: &'data Path,
    diagnostics: &'data Diagnostics,
) -> impl Iterator<Item = T> + 'data
where
    T: DeserializeOwned + 'data,
{
    diagnosed_records::<T>(content, prefilter, unit, diagnostics, None).map(|(_, record)| record)
}

/// Adds a normal warning for a decoding failure whose JSON envelope the source
/// confirms carries usage. The confirmation runs only on decoding failures.
pub fn records_with_usage_diagnostics<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
    unit: &'data Path,
    diagnostics: &'data Diagnostics,
    is_usage: fn(&serde_json::Value) -> bool,
) -> impl Iterator<Item = T> + 'data
where
    T: DeserializeOwned + 'data,
{
    numbered_records_with_usage_diagnostics::<T>(content, prefilter, unit, diagnostics, is_usage)
        .map(|(_, record)| record)
}

/// Retains source line numbers so callers can diagnose rejected fields while
/// preserving independently decoded state in the same record.
pub fn numbered_records_with_usage_diagnostics<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
    unit: &'data Path,
    diagnostics: &'data Diagnostics,
    is_usage: fn(&serde_json::Value) -> bool,
) -> impl Iterator<Item = (u32, T)> + 'data
where
    T: DeserializeOwned + 'data,
{
    diagnosed_records(content, prefilter, unit, diagnostics, Some(is_usage))
}

fn diagnosed_records<'data, T>(
    content: &'data [u8],
    prefilter: Option<&'data LinePrefilter>,
    unit: &'data Path,
    diagnostics: &'data Diagnostics,
    is_usage: Option<fn(&serde_json::Value) -> bool>,
) -> impl Iterator<Item = (u32, T)> + 'data
where
    T: DeserializeOwned + 'data,
{
    selected_lines(content, prefilter).filter_map(move |(number, line)| {
        decode_line::<T>(content, line, unit, number, diagnostics, is_usage)
            .map(|record| (number, record))
    })
}

/// Decodes one JSON record held whole, such as a database JSON column. Usage
/// confirmation runs only after a rejection.
pub fn decode_record<T: DeserializeOwned>(
    content: &[u8],
    unit: &Path,
    number: u32,
    diagnostics: &Diagnostics,
    is_usage: Option<fn(&serde_json::Value) -> bool>,
) -> Option<T> {
    match serde_json::from_slice(content) {
        Ok(record) => Some(record),
        Err(error) => {
            record_decode_error(content, error, unit, number, false, diagnostics, is_usage);
            None
        }
    }
}

/// Decodes `line`, one line of the JSONL file `content`, like
/// [`decode_record`]. The last line of a file that does not end in a newline
/// may still be being written, so a record it cuts short is not a skip.
pub fn decode_line<T: DeserializeOwned>(
    content: &[u8],
    line: &[u8],
    unit: &Path,
    number: u32,
    diagnostics: &Diagnostics,
    is_usage: Option<fn(&serde_json::Value) -> bool>,
) -> Option<T> {
    match serde_json::from_slice(line) {
        Ok(record) => Some(record),
        Err(error) => {
            let unterminated =
                !content.ends_with(b"\n") && line.as_ptr_range().end == content.as_ptr_range().end;
            record_decode_error(
                line,
                error,
                unit,
                number,
                unterminated,
                diagnostics,
                is_usage,
            );
            None
        }
    }
}

/// Records why `record`, record `number` of `unit`, failed to decode.
///
/// `unterminated` says the record is the last line of a JSONL file that does
/// not end in a newline yet. An agent appends its log while ccusage reads it,
/// so a record such a line cuts short is noted under `--debug` instead of
/// skipped: the next run reads it whole.
///
/// A data error is invalid data when `is_usage` confirms that the record, read
/// as plain JSON, carries usage, and undecoded usage otherwise. `is_usage`
/// runs only for a data error, so a source may also take what it needs from
/// the rejected record there.
#[cold]
pub fn record_decode_error(
    record: &[u8],
    error: serde_json::Error,
    unit: &Path,
    number: u32,
    unterminated: bool,
    diagnostics: &Diagnostics,
    is_usage: Option<impl FnOnce(&serde_json::Value) -> bool>,
) {
    use serde_json::error::Category;
    let (kind, category) = match error.classify() {
        Category::Eof if unterminated => {
            diagnostics.debug_note(
                unit,
                format!(
                    "{}: {}:{number}: last line is incomplete, read as still being written",
                    diagnostics.source(),
                    unit.display()
                ),
            );
            return;
        }
        Category::Io => (SkipKind::Unreadable, "I/O"),
        Category::Syntax => (SkipKind::InvalidData, "syntax"),
        Category::Eof => (SkipKind::InvalidData, "EOF"),
        Category::Data => {
            let confirmed = is_usage.is_some_and(|confirm| {
                serde_json::from_slice::<serde_json::Value>(record)
                    .is_ok_and(|value| confirm(&value))
            });
            (
                if confirmed {
                    SkipKind::InvalidData
                } else {
                    SkipKind::UndecodedUsage
                },
                "data",
            )
        }
    };
    diagnostics.record(Skip::record(
        unit,
        number,
        kind,
        format!("JSON {category} error at column {}", error.column()),
    ));
}

/// Deserialize a JSON value into `u64` with the same lenient rules as
/// [`serde_json::Value::as_u64`].
///
/// Non-negative integers that fit in `u64` are returned as-is; floats, strings,
/// nulls, negative numbers, and missing values all become `0`. This reproduces
/// the historical `json_value_u64(value.get(...))` behavior so typed structs
/// match the previous dynamic-`Value` parsing instead of failing the whole line
/// when a token count is encoded unexpectedly.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_u64")]` so a
/// missing field also defaults to `0`.
pub fn lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default())
}

/// Deserialize a JSON value into `Option<i64>` with the same lenient rules as
/// [`serde_json::Value::as_i64`].
///
/// Any integer that fits in `i64` is returned; floats, strings, nulls, and
/// missing values become `None`. This reproduces the historical
/// `Value::as_i64` navigation so an unexpectedly typed field does not fail the
/// whole record.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_i64")]`.
pub fn lenient_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_i64))
}

/// Deserialize a JSON value into `Option<f64>` with the same lenient rules as
/// [`serde_json::Value::as_f64`].
///
/// Any JSON number yields a value; strings, nulls, and missing values become
/// `None`. This reproduces the historical `Value::as_f64` navigation so an
/// unexpectedly typed number (for example a cost field) does not fail the whole
/// record.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_f64")]`.
pub fn lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_f64))
}

/// Deserialize a nested JSON object into `Option<T>` leniently.
///
/// JSON objects are deserialized into `T`; any non-object value (number,
/// string, array, bool), nulls, and missing values become `None`. An object
/// that fails to deserialize into `T` also yields `None` rather than failing
/// the whole record. This reproduces the historical
/// `Value::get(...).map_or(.., ..)` navigation, where a malformed nested object
/// was simply treated as absent instead of discarding an otherwise usable
/// record.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_object")]`.
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

/// Deserialize a JSON array into `Option<Vec<T>>` leniently, skipping elements
/// that fail to deserialize into `T`.
///
/// A JSON array yields `Some(vec)` containing only the elements that
/// successfully deserialize (malformed entries are dropped, not fatal); any
/// non-array value, null, and missing values become `None`. This reproduces the
/// historical `Value::as_array` navigation, where a non-array field was treated
/// as absent and individual bad elements were skipped instead of discarding the
/// whole record. The `Some`/`None` distinction lets callers tell an array that
/// was present (even if empty) apart from a missing or non-array field.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_array")]`.
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

/// Deserialize a JSON array into `Vec<T>` leniently, skipping elements that fail
/// to deserialize into `T`.
///
/// Like [`lenient_array`] but collapses the missing/non-array case to an empty
/// `Vec` for callers that do not need to distinguish a present-but-empty array
/// from an absent field.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::lenient_vec")]`.
pub fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(lenient_array(deserializer)?.unwrap_or_default())
}

/// Deserialize a JSON value into a trimmed, non-empty [`String`].
///
/// Mirrors [`ccusage_core::non_empty_json_string`]: non-string values and
/// empty-after-trim strings become `None`, and surviving strings are trimmed.
/// This keeps typed structs lenient about unexpected field types instead of
/// erroring on the whole line.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::non_empty_string")]`.
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

/// A lenient value that can tell whether it read anything usable.
pub trait Present {
    fn is_present(&self) -> bool;
}

/// Names the object field a [`FirstField`] reads.
pub trait FieldName {
    const NAME: &'static str;
}

/// The first usable value of field `N::NAME` in a JSON object, or `None` for
/// any other value. Unlike a derived struct, it tolerates a repeated field, and
/// it skips every other field and any non-object without building it. `T` must
/// be lenient too, such as [`LenientString`], [`LenientSeq`], or another
/// `FirstField`, because a failure inside the object cannot be recovered.
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

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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

/// A JSON string, or `None` for any other value, which is skipped without
/// being built. Never fails, so it is safe inside a [`FirstField`].
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

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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

/// A JSON array read as `T` elements, or empty for any other value, which is
/// skipped without being built. Elements must be lenient for one malformed
/// element not to fail the whole array.
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

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde::Deserialize;

    use ccusage_core::{Diagnostics, SkipKind, cli::SharedArgs};
    use ccusage_test_support::EnvVarGuard;

    use super::{
        FieldName, FirstField, LenientSeq, LenientString, lenient_array, lenient_f64, lenient_i64,
        lenient_object, lenient_u64, lenient_vec, non_empty_string, records,
        records_with_diagnostics, records_with_usage_diagnostics, try_records,
    };
    use ccusage_core::fast::LinePrefilter;

    #[derive(Debug, PartialEq, Deserialize)]
    struct Record {
        #[serde(default, deserialize_with = "non_empty_string")]
        model: Option<String>,
        #[serde(default, deserialize_with = "lenient_u64")]
        tokens: u64,
    }

    #[test]
    fn records_skips_lines_without_marker() {
        let content =
            b"{\"model\":\"a\",\"tokens\":1}\n{\"other\":true}\n{\"model\":\"b\",\"tokens\":2}\n";
        let prefilter = LinePrefilter::all(&[b"model"]);
        let parsed = records::<Record>(content, Some(&prefilter)).collect::<Vec<_>>();

        assert_eq!(
            parsed,
            [
                Record {
                    model: Some("a".to_string()),
                    tokens: 1,
                },
                Record {
                    model: Some("b".to_string()),
                    tokens: 2,
                },
            ]
        );
    }

    #[test]
    fn try_records_numbers_the_marked_lines_that_do_not_decode() {
        let content = b"{\"model\":\"a\",\"tokens\":1}\n{\"other\":true}\n\n{\"model\":\"b\",\"tok";
        let prefilter = LinePrefilter::all(&[b"model"]);

        let parsed = try_records::<Record>(content, Some(&prefilter))
            .map(|(line, record)| (line, record.ok().map(|record| record.tokens)))
            .collect::<Vec<_>>();

        assert_eq!(parsed, [(1, Some(1)), (4, None)]);
    }

    #[test]
    fn a_record_cut_short_by_the_end_of_the_file_is_noted_not_skipped() {
        let diagnostics = Diagnostics::new("test");
        let records = records_with_diagnostics::<Record>(
            b"{\"tokens\":1}\n{\"tokens\":2,\"mod",
            None,
            Path::new("/logs/live.jsonl"),
            &diagnostics,
        )
        .map(|record| record.tokens)
        .collect::<Vec<_>>();

        assert_eq!(records, [1]);
        let _log_level = EnvVarGuard::set("LOG_LEVEL", "1");
        assert!(diagnostics.warnings(&SharedArgs::default()).is_empty());
        assert_eq!(
            diagnostics.warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            }),
            ["test: /logs/live.jsonl:2: last line is incomplete, read as still being written"]
        );
    }

    #[test]
    fn a_record_cut_short_before_a_newline_is_invalid_data() {
        for content in [
            b"{\"tokens\":2,\"mod\n{\"tokens\":1}".as_slice(),
            b"{\"tokens\":1}\n{\"tokens\":2,\"mod\n",
        ] {
            let diagnostics = Diagnostics::new("test");
            let _ = records_with_diagnostics::<Record>(
                content,
                None,
                Path::new("/logs/torn.jsonl"),
                &diagnostics,
            )
            .count();

            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }

    #[test]
    fn records_skips_unparsable_lines() {
        let content = b"{\"tokens\":1}\nnot json\n{\"tokens\":2}\n";
        let parsed = records::<Record>(content, None).collect::<Vec<_>>();

        assert_eq!(
            parsed
                .iter()
                .map(|record| record.tokens)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn records_with_diagnostics_skips_bad_selected_lines_without_leaking_input() {
        #[derive(Debug, Deserialize)]
        struct StrictRecord {
            model: String,
            tokens: u64,
        }

        let content = b"{\"model\":\"before\",\"tokens\":1}\n{\"other\":\"SECRET_CANARY\",\"tokens\":}\n{\"model\":\"broken\",\"tokens\":\"SECRET_CANARY\"}\n{\"model\":\"after\",\"tokens\":2}";
        let prefilter = LinePrefilter::all(&[b"model"]);
        let diagnostics = Diagnostics::new("test");

        let records = records_with_diagnostics::<StrictRecord>(
            content,
            Some(&prefilter),
            Path::new("/logs/session.jsonl"),
            &diagnostics,
        )
        .map(|record| (record.model, record.tokens))
        .collect::<Vec<_>>();

        assert_eq!(
            records,
            [("before".to_string(), 1), ("after".to_string(), 2)]
        );
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);

        let _log_level = EnvVarGuard::set("LOG_LEVEL", "1");
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(warnings.contains("/logs/session.jsonl:3: JSON data error at column"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn confirmed_usage_decode_failures_warn_without_labeling_other_records_as_usage() {
        #[derive(Deserialize)]
        struct Record {
            tokens: u64,
        }
        let diagnostics = Diagnostics::new("test");
        let records = super::records_with_usage_diagnostics::<Record>(
            b"{\"type\":\"user\",\"tokens\":\"text\"}\n{\"type\":\"usage\",\"tokens\":\"SECRET_CANARY\"}\n{\"type\":\"usage\",\"tokens\":3}",
            None,
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            |value| value.get("type").and_then(serde_json::Value::as_str) == Some("usage"),
        ).map(|record| record.tokens).collect::<Vec<_>>();

        assert_eq!(records, [3]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    }

    #[test]
    fn ignores_blank_jsonl_lines_and_keeps_physical_line_numbers() {
        #[derive(Debug, Deserialize)]
        struct UsageRecord {
            tokens: u64,
        }

        let content = concat!(
            r#"{"kind":"usage","tokens":1}"#,
            "\n \t\r\n",
            r#"{"kind":"usage","tokens":"SECRET_CANARY"}"#,
            "\n",
            r#"{"kind":"usage","tokens":2}"#,
        )
        .as_bytes();
        let diagnostics = Diagnostics::new("test");
        let records = records_with_usage_diagnostics::<UsageRecord>(
            content,
            None,
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            |value| value.get("kind").and_then(serde_json::Value::as_str) == Some("usage"),
        )
        .map(|record| record.tokens)
        .collect::<Vec<_>>();

        assert_eq!(records, [1, 2]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);

        let _log_level = EnvVarGuard::set("LOG_LEVEL", "1");
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(warnings.contains("skipped 1 record in 1 input (invalid data)"));
        assert!(warnings.contains("/logs/session.jsonl:3: JSON data error"));
        assert!(!warnings.contains("/logs/session.jsonl:2:"));
        assert!(!warnings.contains("SECRET_CANARY"));

        let database_diagnostics = Diagnostics::new("test");
        assert!(
            super::decode_record::<UsageRecord>(
                b" \t\r",
                Path::new("/logs/state.db"),
                1,
                &database_diagnostics,
                Some(|_| true),
            )
            .is_none()
        );
        assert_eq!(
            database_diagnostics.skipped(SkipKind::InvalidData),
            1,
            "blank database cells are not JSONL blank lines"
        );
    }

    #[test]
    fn lenient_wrappers_skip_values_of_another_shape_and_repeated_fields() {
        struct Id;
        impl FieldName for Id {
            const NAME: &'static str = "id";
        }
        struct Child;
        impl FieldName for Child {
            const NAME: &'static str = "child";
        }
        struct Parts;
        impl FieldName for Parts {
            const NAME: &'static str = "parts";
        }
        type ChildId = FirstField<Child, FirstField<Id, LenientString>>;

        let parse = |raw: &str| {
            serde_json::from_str::<FirstField<Parts, LenientSeq<ChildId>>>(raw)
                .unwrap()
                .0
                .map(|parts| {
                    parts
                        .0
                        .into_iter()
                        .map(|part| part.0.and_then(|child| child.0).and_then(|id| id.0))
                        .collect::<Vec<_>>()
                })
        };

        assert_eq!(
            parse(
                r#"{"parts":[{"child":{"id":"x"}},"junk",{"child":[1]},{"child":{"id":{"o":[2]}}},7,null,{"child":{"id":["n"],"id":"first","id":"second"},"child":{"id":"later"}},{"child":{"id":5},"child":{"id":"y"}}]}"#
            ),
            Some(vec![
                Some("x".to_string()),
                None,
                None,
                None,
                None,
                None,
                Some("first".to_string()),
                Some("y".to_string()),
            ])
        );
        assert_eq!(
            parse(r#"{"parts":"x","parts":[{"child":{"id":"z"}}]}"#),
            Some(vec![Some("z".to_string())])
        );
        assert_eq!(parse(r#"{"parts":{"a":[1]}}"#), None);
        assert_eq!(parse(r#"["parts"]"#), None);
    }

    #[test]
    fn lenient_u64_matches_value_as_u64() {
        let coerce = |raw: &str| {
            serde_json::from_str::<Record>(&format!("{{\"tokens\":{raw}}}"))
                .unwrap()
                .tokens
        };

        assert_eq!(coerce("42"), 42);
        assert_eq!(coerce("12.5"), 0);
        assert_eq!(coerce("-1"), 0);
        assert_eq!(coerce("\"7\""), 0);
        assert_eq!(coerce("null"), 0);
    }

    #[test]
    fn lenient_i64_and_f64_match_value_accessors() {
        #[derive(Deserialize)]
        struct Numbers {
            #[serde(default, deserialize_with = "lenient_i64")]
            created: Option<i64>,
            #[serde(default, deserialize_with = "lenient_f64")]
            cost: Option<f64>,
        }

        let parse = |raw: &str| serde_json::from_str::<Numbers>(raw).unwrap();

        let both = parse("{\"created\":-5,\"cost\":1.5}");
        assert_eq!(both.created, Some(-5));
        assert_eq!(both.cost, Some(1.5));

        // i64 rejects floats; f64 accepts any number.
        let mixed = parse("{\"created\":1.5,\"cost\":7}");
        assert_eq!(mixed.created, None);
        assert_eq!(mixed.cost, Some(7.0));

        // Strings, nulls, and missing values all become None.
        let strings = parse("{\"created\":\"3\",\"cost\":\"x\"}");
        assert_eq!(strings.created, None);
        assert_eq!(strings.cost, None);

        let missing = parse("{}");
        assert_eq!(missing.created, None);
        assert_eq!(missing.cost, None);
    }

    #[test]
    fn lenient_object_keeps_record_when_nested_field_is_not_an_object() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Nested {
            #[serde(default, deserialize_with = "lenient_u64")]
            read: u64,
        }

        #[derive(Debug, PartialEq, Deserialize)]
        struct Outer {
            #[serde(default, deserialize_with = "lenient_object")]
            cache: Option<Nested>,
            #[serde(default, deserialize_with = "lenient_u64")]
            input: u64,
        }

        let parse = |raw: &str| serde_json::from_str::<Outer>(raw).unwrap();

        // A real object deserializes as expected.
        assert_eq!(
            parse("{\"cache\":{\"read\":5},\"input\":7}"),
            Outer {
                cache: Some(Nested { read: 5 }),
                input: 7,
            }
        );

        // Non-object cache payloads become None instead of failing the record,
        // preserving the sibling `input` value.
        for raw in [
            "{\"cache\":5,\"input\":7}",
            "{\"cache\":\"oops\",\"input\":7}",
            "{\"cache\":[1,2],\"input\":7}",
            "{\"cache\":null,\"input\":7}",
            "{\"input\":7}",
        ] {
            assert_eq!(
                parse(raw),
                Outer {
                    cache: None,
                    input: 7,
                },
                "raw: {raw}"
            );
        }
    }

    #[test]
    fn lenient_array_and_vec_skip_bad_elements_and_tolerate_non_arrays() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Item {
            #[serde(default, deserialize_with = "lenient_u64")]
            value: u64,
        }

        #[derive(Debug, PartialEq, Deserialize)]
        struct Outer {
            #[serde(default, deserialize_with = "lenient_array")]
            optional: Option<Vec<Item>>,
            #[serde(default, deserialize_with = "lenient_vec")]
            required: Vec<Item>,
        }

        let parse = |raw: &str| serde_json::from_str::<Outer>(raw).unwrap();

        // Non-object array elements are skipped instead of failing the record.
        let mixed =
            parse(r#"{"optional":[{"value":1},"oops",{"value":2}],"required":[3,{"value":4}]}"#);
        assert_eq!(
            mixed.optional,
            Some(vec![Item { value: 1 }, Item { value: 2 }])
        );
        assert_eq!(mixed.required, vec![Item { value: 4 }]);

        // A present-but-empty array stays distinguishable from a missing field
        // for `lenient_array`, while `lenient_vec` collapses both to an empty
        // vec.
        let empty = parse(r#"{"optional":[],"required":[]}"#);
        assert_eq!(empty.optional, Some(vec![]));
        assert_eq!(empty.required, vec![]);

        // Non-array and missing values become None / empty without erroring.
        for raw in [
            r#"{"optional":5,"required":"nope"}"#,
            r#"{"optional":null,"required":null}"#,
            r#"{}"#,
        ] {
            let parsed = parse(raw);
            assert_eq!(parsed.optional, None, "raw: {raw}");
            assert_eq!(parsed.required, Vec::<Item>::new(), "raw: {raw}");
        }
    }

    #[test]
    fn non_empty_string_trims_and_drops_empty() {
        let parse = |raw: &str| {
            serde_json::from_str::<Record>(&format!("{{\"model\":{raw}}}"))
                .unwrap()
                .model
        };

        assert_eq!(parse("\"  qwen  \""), Some("qwen".to_string()));
        assert_eq!(parse("\"   \""), None);
        assert_eq!(parse("123"), None);
        assert_eq!(parse("null"), None);
    }
}
