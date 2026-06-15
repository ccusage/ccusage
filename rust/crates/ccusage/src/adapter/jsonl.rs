//! Shared JSONL parsing helpers for agent adapters.
//!
//! Adapters historically parsed each log line into a dynamic
//! [`serde_json::Value`] and then hand-navigated it with `Value::get`. This
//! module centralizes the faster gold-standard approach used by the Claude
//! loader so every adapter shares the same optimizations:
//!
//! 1. Read the whole file once and split it into byte slices with
//!    [`byte_lines`](crate::fast::byte_lines), avoiding a `String` allocation
//!    per line.
//! 2. Skip lines that cannot possibly match using a precompiled `memmem`
//!    substring prefilter, before any JSON parsing happens.
//! 3. Deserialize the surviving lines directly into a typed struct with
//!    `serde_json::from_slice`, so unused fields are skipped instead of being
//!    materialized into an intermediate `Value` tree.

use memchr::memmem;
use serde::{Deserialize, Deserializer, de::DeserializeOwned};

use crate::fast::byte_lines;

/// Iterate over deserialized JSONL records contained in `content`.
///
/// When `marker` is provided, lines that do not contain that byte substring are
/// skipped before any JSON parsing, mirroring the per-line `memmem` prefilter
/// used by the Claude loader. Pick a marker that appears in every line the
/// adapter would accept (for example the required `"usage"` key) so the
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
/// let models: Vec<_> = jsonl::records::<Record>(content, Some(b"model"))
///     .filter_map(|record| record.model)
///     .collect();
/// assert_eq!(models, ["qwen3-coder"]);
/// ```
pub(crate) fn records<'data, T>(
    content: &'data [u8],
    marker: Option<&[u8]>,
) -> impl Iterator<Item = T> + 'data
where
    T: DeserializeOwned + 'data,
{
    // Build the finder once and move it into the closure so the needle is
    // compiled a single time and reused for every line in the file.
    let finder = marker.map(|needle| memmem::Finder::new(needle).into_owned());
    byte_lines(content).filter_map(move |line| {
        if let Some(finder) = &finder
            && finder.find(line).is_none()
        {
            return None;
        }
        serde_json::from_slice::<T>(line).ok()
    })
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
pub(crate) fn lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
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
pub(crate) fn lenient_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
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
pub(crate) fn lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_f64))
}

/// Deserialize a JSON value into a trimmed, non-empty [`String`].
///
/// Mirrors [`crate::non_empty_json_string`]: non-string values and
/// empty-after-trim strings become `None`, and surviving strings are trimmed.
/// This keeps typed structs lenient about unexpected field types instead of
/// erroring on the whole line.
///
/// Use with `#[serde(default, deserialize_with = "jsonl::non_empty_string")]`.
pub(crate) fn non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(crate::non_empty_json_string(value.as_ref()))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{lenient_f64, lenient_i64, lenient_u64, non_empty_string, records};

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
        let parsed = records::<Record>(content, Some(b"model")).collect::<Vec<_>>();

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
