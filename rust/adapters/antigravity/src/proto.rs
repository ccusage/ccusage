//! Minimal protobuf wire-format reader for the blobs Antigravity stores in its
//! conversation SQLite databases.
//!
//! Antigravity persists `CortexStepMetadata` and `ChatModelMetadata` as raw
//! protobuf, and ships no `.proto` files. The field numbers below were recovered
//! from the `FileDescriptorProto` blobs embedded in the `agy` binary, so they are
//! the upstream numbers rather than guesses. Antigravity has to read these same
//! blobs back after an upgrade, which means it cannot renumber fields without
//! breaking its own persistence — that is what makes decoding by number safe.
//!
//! Only the handful of fields ccusage needs are decoded; everything else is
//! skipped. A hand-rolled reader keeps `prost` (and its build-time codegen and
//! binary-size cost) out of the dependency graph.

use ccusage_core::TimestampMs;

/// One protobuf field value, limited to the wire types Antigravity actually uses.
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed64,
    Fixed32,
}

/// Cursor over a protobuf message body.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Decode a base-128 varint, advancing the cursor.
    ///
    /// Returns `None` on truncated input or on a varint longer than the 10 bytes
    /// a 64-bit value can occupy, which keeps malformed blobs from looping.
    fn varint(&mut self) -> Option<u64> {
        let mut value: u64 = 0;
        for index in 0..10 {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            // Each byte contributes 7 bits; bit 8 marks "more bytes follow".
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    /// Read the next `(field_number, value)` pair, or `None` at end of message.
    ///
    /// A malformed tag also yields `None`, which makes callers treat the rest of
    /// the message as absent instead of erroring out. Partial data is preferable
    /// to discarding a whole conversation.
    fn next_field(&mut self) -> Option<(u32, Value<'a>)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let tag = self.varint()?;
        // Low 3 bits are the wire type, the rest is the field number.
        let field = u32::try_from(tag >> 3).ok()?;
        if field == 0 {
            return None;
        }
        let value = match tag & 0x7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.pos = self.pos.checked_add(8)?;
                Value::Fixed64
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                let start = self.pos;
                let end = start.checked_add(len)?;
                let slice = self.buf.get(start..end)?;
                self.pos = end;
                Value::Bytes(slice)
            }
            5 => {
                self.pos = self.pos.checked_add(4)?;
                Value::Fixed32
            }
            // Wire types 3 and 4 are deprecated groups, and 6/7 are invalid.
            // Neither appears in Antigravity's schema, so stop rather than guess
            // at a length and misread the remainder.
            _ => return None,
        };
        Some((field, value))
    }
}

/// Read a `google.protobuf.Timestamp` (field 1 seconds, field 2 nanos).
fn timestamp(bytes: &[u8]) -> Option<TimestampMs> {
    let mut seconds: Option<i64> = None;
    let mut nanos: u32 = 0;
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        match (field, value) {
            (1, Value::Varint(raw)) => seconds = Some(raw as i64),
            (2, Value::Varint(raw)) => nanos = u32::try_from(raw).unwrap_or(0),
            _ => {}
        }
    }
    let seconds = seconds?;
    // Antigravity only ever writes wall-clock timestamps, so reject the zero and
    // negative range instead of producing 1970 dates from an unset field.
    if seconds <= 0 {
        return None;
    }
    let millis = seconds
        .checked_mul(1_000)?
        .checked_add(i64::from(nanos / 1_000_000))?;
    Some(TimestampMs::from_millis(millis))
}

/// Interpret a length-delimited field as UTF-8, discarding empty or binary values.
fn text(bytes: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(bytes).ok()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Token counts and identifiers for a single model invocation.
///
/// Mirrors Antigravity's `ModelUsageStats`. `output_tokens` already includes
/// `thinking_output_tokens`, so the thinking count must never be added on top.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ModelUsage {
    /// `model` — an opaque numeric model id. The shipped descriptor only names
    /// these as `MODEL_PLACEHOLDER_M<n>`, so it cannot be turned into a price.
    pub(crate) model_id: u64,
    /// `input_tokens` — excludes `cache_read_tokens`. A record with
    /// `input_tokens` below `cache_read_tokens` proves the two are disjoint.
    pub(crate) input_tokens: u64,
    /// `output_tokens` — thinking plus visible response tokens.
    pub(crate) output_tokens: u64,
    /// `cache_write_tokens`.
    pub(crate) cache_write_tokens: u64,
    /// `cache_read_tokens`.
    pub(crate) cache_read_tokens: u64,
    /// `thinking_output_tokens`, kept only to reconstruct `output_tokens` when
    /// the total is absent.
    pub(crate) thinking_output_tokens: u64,
    /// `response_output_tokens`, used for the same fallback.
    pub(crate) response_output_tokens: u64,
    /// `response_id` — server-assigned, and the primary dedup key.
    pub(crate) response_id: Option<String>,
    /// `message_id` — dedup fallback when `response_id` is absent.
    pub(crate) message_id: Option<String>,
    /// `provider_assigned_message_id` — second dedup fallback.
    pub(crate) provider_message_id: Option<String>,
}

impl ModelUsage {
    /// Total output tokens, reconstructed from the thinking and response split
    /// when the precomputed total is missing.
    pub(crate) fn total_output_tokens(&self) -> u64 {
        if self.output_tokens > 0 {
            return self.output_tokens;
        }
        self.thinking_output_tokens
            .saturating_add(self.response_output_tokens)
    }

    /// Whether the record carries any billable token count.
    pub(crate) fn has_tokens(&self) -> bool {
        self.input_tokens > 0
            || self.cache_read_tokens > 0
            || self.cache_write_tokens > 0
            || self.total_output_tokens() > 0
    }

    /// Stable identity for this invocation, preferring server-assigned ids.
    ///
    /// Deduplicating on this is what keeps a single model call from being counted
    /// twice when it shows up in more than one place — `retry_infos` repeats the
    /// successful attempt alongside the failed ones, and a sub-conversation may
    /// repeat a call already recorded by its parent.
    pub(crate) fn identity(&self) -> Option<&str> {
        self.response_id
            .as_deref()
            .or(self.provider_message_id.as_deref())
            .or(self.message_id.as_deref())
    }
}

/// Decode a `ModelUsageStats` message.
fn model_usage(bytes: &[u8]) -> ModelUsage {
    let mut usage = ModelUsage::default();
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        match (field, value) {
            (1, Value::Varint(raw)) => usage.model_id = raw,
            (2, Value::Varint(raw)) => usage.input_tokens = raw,
            (3, Value::Varint(raw)) => usage.output_tokens = raw,
            (4, Value::Varint(raw)) => usage.cache_write_tokens = raw,
            (5, Value::Varint(raw)) => usage.cache_read_tokens = raw,
            (9, Value::Varint(raw)) => usage.thinking_output_tokens = raw,
            (10, Value::Varint(raw)) => usage.response_output_tokens = raw,
            (7, Value::Bytes(raw)) => usage.message_id = text(raw),
            (11, Value::Bytes(raw)) => usage.response_id = text(raw),
            (12, Value::Bytes(raw)) => usage.provider_message_id = text(raw),
            _ => {}
        }
    }
    usage
}

/// Pull the `usage` (field 2) out of a `RetryInfo` message.
fn retry_usage(bytes: &[u8]) -> Option<ModelUsage> {
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        if let (2, Value::Bytes(raw)) = (field, value) {
            return Some(model_usage(raw));
        }
    }
    None
}

/// Every model invocation recorded by one conversation step.
#[derive(Debug, Default, Clone)]
pub(crate) struct StepUsage {
    /// `created_at`, falling back to `started_at`.
    pub(crate) timestamp: Option<TimestampMs>,
    /// The successful invocation plus every retry attempt.
    pub(crate) usages: Vec<ModelUsage>,
}

/// Decode the `CortexStepMetadata` blob stored in `steps.metadata`.
///
/// Both `model_usage` (field 9) and each `retry_infos` entry (field 28) are
/// collected. Attempts that failed still consumed input tokens, and the caller
/// deduplicates by [`ModelUsage::identity`], so gathering both cannot
/// double-count the attempt that succeeded.
pub(crate) fn parse_step_metadata(bytes: &[u8]) -> StepUsage {
    let mut step = StepUsage::default();
    let mut started_at = None;
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        match (field, value) {
            (1, Value::Bytes(raw)) => step.timestamp = timestamp(raw),
            (32, Value::Bytes(raw)) => started_at = timestamp(raw),
            (9, Value::Bytes(raw)) => step.usages.push(model_usage(raw)),
            (28, Value::Bytes(raw)) => step.usages.extend(retry_usage(raw)),
            _ => {}
        }
    }
    step.timestamp = step.timestamp.or(started_at);
    step
}

/// A model invocation recorded by `gen_metadata`, including its model name.
#[derive(Debug, Default, Clone)]
pub(crate) struct GenerationUsage {
    /// `response_model`, e.g. `gemini-3.6-flash`. This is the only place a
    /// human-readable model name appears, which is why `gen_metadata` is read at
    /// all rather than relying on `steps` alone.
    pub(crate) model: Option<String>,
    /// `chat_start_metadata.created_at`.
    pub(crate) timestamp: Option<TimestampMs>,
    /// The successful invocation plus every retry attempt.
    pub(crate) usages: Vec<ModelUsage>,
}

impl GenerationUsage {
    /// Whether this record can name the model behind a usage row.
    fn names_a_model(&self) -> bool {
        self.model.is_some() && self.usages.iter().any(|usage| usage.identity().is_some())
    }
}

/// Decode a `ChatModelMetadata` message.
fn chat_model_metadata(bytes: &[u8]) -> GenerationUsage {
    let mut generation = GenerationUsage::default();
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        match (field, value) {
            (4, Value::Bytes(raw)) => generation.usages.push(model_usage(raw)),
            (17, Value::Bytes(raw)) => generation.usages.extend(retry_usage(raw)),
            (19, Value::Bytes(raw)) => generation.model = text(raw),
            // `chat_start_metadata.created_at` is the request time; the step rows
            // carry their own timestamp, so this only matters for usage that
            // appears in `gen_metadata` alone.
            (9, Value::Bytes(raw)) => {
                let mut start = Reader::new(raw);
                while let Some((inner, inner_value)) = start.next_field() {
                    if let (4, Value::Bytes(stamp)) = (inner, inner_value) {
                        generation.timestamp = timestamp(stamp);
                    }
                }
            }
            _ => {}
        }
    }
    generation
}

/// Decode the blob stored in `gen_metadata.data`.
///
/// The `ChatModelMetadata` sits inside a wrapper message. Rather than hard-coding
/// the wrapper's field number, every length-delimited field is tried and only
/// those yielding both a model name and an identifiable usage row are kept. The
/// sibling field holding the outbound request has neither, so it is rejected
/// without needing to know the wrapper's shape.
pub(crate) fn parse_generation_metadata(bytes: &[u8]) -> Vec<GenerationUsage> {
    let mut found = Vec::new();
    let mut reader = Reader::new(bytes);
    while let Some((_, value)) = reader.next_field() {
        if let Value::Bytes(raw) = value {
            let generation = chat_model_metadata(raw);
            if generation.names_a_model() {
                found.push(generation);
            }
        }
    }
    found
}

/// Protobuf encoders for building fixture blobs.
///
/// Antigravity's real conversation databases embed absolute paths, user prompts and
/// Antigravity's own system prompt, none of which belong in this repository, so
/// tests synthesize the blobs they need instead of shipping a captured database.
#[cfg(test)]
pub(crate) mod encode {
    /// Encode a base-128 varint.
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// Encode a varint field.
    pub(crate) fn field_varint(number: u32, value: u64) -> Vec<u8> {
        let mut out = varint(u64::from(number) << 3);
        out.extend(varint(value));
        out
    }

    /// Encode a length-delimited field.
    pub(crate) fn field_bytes(number: u32, value: &[u8]) -> Vec<u8> {
        let mut out = varint((u64::from(number) << 3) | 2);
        out.extend(varint(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    /// Build a `google.protobuf.Timestamp`.
    pub(crate) fn timestamp_bytes(seconds: i64, nanos: u32) -> Vec<u8> {
        let mut out = field_varint(1, seconds as u64);
        out.extend(field_varint(2, u64::from(nanos)));
        out
    }

    /// Build a `ModelUsageStats` with the token fields ccusage reads.
    pub(crate) fn usage_bytes(
        model_id: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        response_id: &str,
    ) -> Vec<u8> {
        let mut out = field_varint(1, model_id);
        out.extend(field_varint(2, input));
        out.extend(field_varint(3, output));
        out.extend(field_varint(5, cache_read));
        out.extend(field_bytes(11, response_id.as_bytes()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{encode::*, *};

    #[test]
    fn decodes_the_token_fields_of_a_model_usage_record() {
        let usage = model_usage(&usage_bytes(1071, 4050, 375, 16275, "response-a"));

        assert_eq!(usage.model_id, 1071);
        assert_eq!(usage.input_tokens, 4050);
        assert_eq!(usage.output_tokens, 375);
        assert_eq!(usage.cache_read_tokens, 16275);
        assert_eq!(usage.response_id.as_deref(), Some("response-a"));
    }

    #[test]
    fn reconstructs_output_tokens_from_the_thinking_and_response_split() {
        let mut bytes = field_varint(9, 351);
        bytes.extend(field_varint(10, 24));
        let usage = model_usage(&bytes);

        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.total_output_tokens(), 375);
    }

    #[test]
    fn prefers_the_recorded_output_total_over_the_split() {
        let mut bytes = field_varint(3, 375);
        bytes.extend(field_varint(9, 351));
        bytes.extend(field_varint(10, 24));

        assert_eq!(model_usage(&bytes).total_output_tokens(), 375);
    }

    #[test]
    fn falls_back_through_the_identity_fields() {
        let mut bytes = field_bytes(7, b"message-a");
        bytes.extend(field_bytes(12, b"provider-a"));
        let usage = model_usage(&bytes);

        assert_eq!(usage.identity(), Some("provider-a"));
        assert_eq!(
            model_usage(&field_bytes(7, b"message-a")).identity(),
            Some("message-a")
        );
        assert_eq!(model_usage(&[]).identity(), None);
    }

    #[test]
    fn reads_step_usage_with_retry_attempts() {
        let mut bytes = field_bytes(1, &timestamp_bytes(1_785_328_986, 355_887_000));
        bytes.extend(field_bytes(
            9,
            &usage_bytes(1071, 4050, 375, 16275, "response-ok"),
        ));
        bytes.extend(field_bytes(
            28,
            &field_bytes(2, &usage_bytes(1071, 4000, 0, 0, "response-failed")),
        ));
        let step = parse_step_metadata(&bytes);

        assert_eq!(step.usages.len(), 2);
        assert_eq!(step.usages[0].response_id.as_deref(), Some("response-ok"));
        assert_eq!(
            step.usages[1].response_id.as_deref(),
            Some("response-failed")
        );
        assert_eq!(
            step.timestamp,
            Some(TimestampMs::from_millis(1_785_328_986_355))
        );
    }

    #[test]
    fn falls_back_to_started_at_when_created_at_is_absent() {
        let bytes = field_bytes(32, &timestamp_bytes(1_785_328_980, 0));

        assert_eq!(
            parse_step_metadata(&bytes).timestamp,
            Some(TimestampMs::from_millis(1_785_328_980_000))
        );
    }

    #[test]
    fn rejects_an_unset_timestamp_instead_of_dating_it_to_1970() {
        assert_eq!(parse_step_metadata(&field_bytes(1, &[])).timestamp, None);
        assert_eq!(timestamp(&field_varint(1, 0)), None);
    }

    #[test]
    fn finds_the_named_generation_inside_its_wrapper() {
        let mut chat = field_bytes(4, &usage_bytes(1071, 20110, 33, 0, "response-a"));
        chat.extend(field_bytes(
            9,
            &field_bytes(4, &timestamp_bytes(1_785_328_980, 0)),
        ));
        chat.extend(field_bytes(19, b"gemini-3.6-flash"));
        chat.extend(field_bytes(21, b"Gemini 3.6 Flash (High)"));

        // Wrapper: an unrelated request-shaped sibling plus the real payload.
        let mut wrapper = field_bytes(3, &field_bytes(28, b"gemini-3.6-flash-high"));
        wrapper.extend(field_bytes(1, &chat));
        let found = parse_generation_metadata(&wrapper);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model.as_deref(), Some("gemini-3.6-flash"));
        assert_eq!(
            found[0].usages[0].response_id.as_deref(),
            Some("response-a")
        );
        assert_eq!(
            found[0].timestamp,
            Some(TimestampMs::from_millis(1_785_328_980_000))
        );
    }

    #[test]
    fn ignores_a_generation_that_cannot_name_a_model() {
        // A model name with no identifiable usage row cannot be attributed.
        let bytes = field_bytes(1, &field_bytes(19, b"gemini-3.6-flash"));

        assert!(parse_generation_metadata(&bytes).is_empty());
    }

    #[test]
    fn stops_at_malformed_input_without_panicking() {
        // Truncated length prefix, unterminated varint, and a group wire type.
        assert!(parse_step_metadata(&[0x0a, 0x7f]).usages.is_empty());
        assert!(parse_step_metadata(&[0x08, 0xff, 0xff]).usages.is_empty());
        assert!(parse_step_metadata(&[0x0b, 0x00]).usages.is_empty());
        assert!(parse_generation_metadata(&[0xff, 0xff, 0xff]).is_empty());
    }

    #[test]
    fn treats_a_record_without_tokens_as_empty() {
        assert!(!model_usage(&field_bytes(11, b"response-a")).has_tokens());
        assert!(model_usage(&field_varint(5, 16275)).has_tokens());
    }
}
