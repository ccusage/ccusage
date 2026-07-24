use std::{borrow::Cow, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use super::proto::{FieldValue, read_fields};
use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_usage,
};

/// Token counts, model id, timestamp, and response id decoded from one
/// `gen_metadata` row of an Antigravity conversation database.
#[derive(Debug, Default)]
pub(super) struct GenerationMetadata {
    pub(super) raw_model: Option<String>,
    pub(super) timestamp_ms: Option<i64>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) thinking_tokens: u64,
    pub(super) response_id: Option<String>,
}

/// Session-level fallbacks decoded from `trajectory_metadata_blob`.
#[derive(Debug, Default)]
pub(super) struct TrajectoryMetadata {
    pub(super) created_ms: Option<i64>,
    pub(super) workspace_uri: Option<String>,
}

/// Shared per-database context used to shape every generation entry.
pub(super) struct SessionContext {
    pub(super) session_id: String,
    pub(super) project: String,
    pub(super) project_path: String,
    pub(super) fallback_timestamp: TimestampMs,
}

impl GenerationMetadata {
    fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.thinking_tokens)
    }
}

/// Decodes one `GeneratorMetadata` blob. Layout (hand-verified, no .proto):
///
/// - field 1 (LEN message) → chatModel
///   - field 19 (LEN string) → raw model id
///   - field 9 (LEN message) → generation info; its field 4 → timestamp
///     message `{1: seconds varint, 2: nanos varint}`
///   - field 4 (LEN message) → usage
///     - field 1 (varint) → fixed system-prompt input tokens
///     - field 2 (varint) → fresh (non-cached) input tokens
///     - field 5 (varint) → cache-read tokens
///     - field 9 (varint) → output (text) tokens
///     - field 10 (varint) → thinking/reasoning tokens
///     - field 11 (LEN string) → responseId (dedup key)
pub(super) fn decode_generation(bytes: &[u8]) -> GenerationMetadata {
    let mut metadata = GenerationMetadata::default();
    for (number, value) in read_fields(bytes) {
        if number != 1 {
            continue;
        }
        let FieldValue::LengthDelimited(chat_model) = value else {
            continue;
        };
        for (number, value) in read_fields(chat_model) {
            match (number, value) {
                (19, FieldValue::LengthDelimited(model)) => {
                    metadata.raw_model = utf8_string(model);
                }
                (9, FieldValue::LengthDelimited(info)) => {
                    metadata.timestamp_ms = decode_generation_timestamp(info);
                }
                (4, FieldValue::LengthDelimited(usage)) => decode_usage(usage, &mut metadata),
                _ => {}
            }
        }
    }
    metadata
}

fn decode_generation_timestamp(info: &[u8]) -> Option<i64> {
    for (number, value) in read_fields(info) {
        if number == 4
            && let FieldValue::LengthDelimited(timestamp) = value
        {
            return decode_timestamp(timestamp);
        }
    }
    None
}

/// Decodes `{1: seconds varint, 2: nanos varint}` into Unix epoch
/// milliseconds. Nanos outside `0..=999_999_999` reject the timestamp.
fn decode_timestamp(bytes: &[u8]) -> Option<i64> {
    let mut seconds = None;
    let mut nanos = None;
    for (number, value) in read_fields(bytes) {
        let FieldValue::Varint(value) = value else {
            continue;
        };
        match number {
            1 => seconds = Some(clamp_varint(value)),
            2 => nanos = Some(value),
            _ => {}
        }
    }
    let seconds = i64::try_from(seconds?).ok()?;
    let nanos = nanos.unwrap_or(0);
    if nanos > 999_999_999 {
        return None;
    }
    let millis = i64::try_from(nanos / 1_000_000).ok()?;
    seconds.checked_mul(1_000)?.checked_add(millis)
}

fn decode_usage(usage: &[u8], metadata: &mut GenerationMetadata) {
    let mut system_input = 0_u64;
    let mut fresh_input = 0_u64;
    for (number, value) in read_fields(usage) {
        match (number, value) {
            (1, FieldValue::Varint(value)) => system_input = clamp_varint(value),
            (2, FieldValue::Varint(value)) => fresh_input = clamp_varint(value),
            (5, FieldValue::Varint(value)) => metadata.cache_read_tokens = clamp_varint(value),
            (9, FieldValue::Varint(value)) => metadata.output_tokens = clamp_varint(value),
            (10, FieldValue::Varint(value)) => metadata.thinking_tokens = clamp_varint(value),
            (11, FieldValue::LengthDelimited(id)) => metadata.response_id = utf8_string(id),
            _ => {}
        }
    }
    metadata.input_tokens = system_input.saturating_add(fresh_input);
}

/// Decodes a `trajectory_metadata_blob` row: field 2 holds the session
/// created-at timestamp, field 1 → field 1 holds the workspace `file://` URI.
pub(super) fn decode_trajectory_metadata(bytes: &[u8]) -> TrajectoryMetadata {
    let mut metadata = TrajectoryMetadata::default();
    for (number, value) in read_fields(bytes) {
        match (number, value) {
            (2, FieldValue::LengthDelimited(timestamp)) => {
                metadata.created_ms = decode_timestamp(timestamp);
            }
            (1, FieldValue::LengthDelimited(inner)) => {
                for (number, value) in read_fields(inner) {
                    if number == 1
                        && let FieldValue::LengthDelimited(uri) = value
                    {
                        metadata.workspace_uri = utf8_string(uri);
                    }
                }
            }
            _ => {}
        }
    }
    metadata
}

fn utf8_string(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Token varints are clamped to `i64::MAX` so corrupt blobs cannot wrap the
/// counters into negative or overflowing values downstream.
fn clamp_varint(value: u64) -> u64 {
    value.min(i64::MAX as u64)
}

/// Maps Antigravity's raw model ids to LiteLLM-priced names
/// (case-insensitive). Unknown ids pass through unchanged; the pricing lookup
/// handles misses gracefully.
pub(super) fn resolve_model_name(raw: &str) -> Cow<'_, str> {
    let resolved = match raw.to_ascii_lowercase().as_str() {
        "model_placeholder_m26" => "claude-opus-4-6",
        "model_placeholder_m35" => "claude-sonnet-4-6",
        "model_placeholder_m36" | "model_placeholder_m37" | "model_placeholder_m16" => {
            "gemini-3.1-pro"
        }
        "model_placeholder_m18" | "model_placeholder_m84" | "model_placeholder_m47" => {
            "gemini-3-flash-preview"
        }
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
        "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
        "gemini-3-flash-agent" | "gemini-3-flash-a" | "gemini-3-flash-b" => "gemini-3.5-flash-high",
        "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
        "gemini-3.5-flash-low" => "gemini-3.5-flash-medium",
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro",
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro",
        "claude-opus-4-6-thinking" => "claude-opus-4-6",
        "claude-sonnet-4-6-thinking" => "claude-sonnet-4-6",
        _ => return Cow::Borrowed(raw),
    };
    Cow::Borrowed(resolved)
}

/// Builds the per-database context: conversation id from the file name,
/// project from the percent-decoded workspace URI, and the timestamp fallback
/// chain (session created-at, then file mtime).
pub(super) fn session_context(
    db_path: &Path,
    trajectory: &TrajectoryMetadata,
    file_mtime_ms: Option<i64>,
) -> SessionContext {
    let session_id = db_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let workspace_path = trajectory
        .workspace_uri
        .as_deref()
        .and_then(workspace_path_from_uri);
    let project = workspace_path
        .as_deref()
        .and_then(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let fallback_ms = trajectory.created_ms.or(file_mtime_ms).unwrap_or_default();
    SessionContext {
        session_id,
        project,
        project_path: workspace_path.unwrap_or_else(|| "unknown".to_string()),
        fallback_timestamp: TimestampMs::from_millis(fallback_ms),
    }
}

/// Decodes a `file://` workspace URI into a filesystem path. Only `%XX`
/// sequences are decoded; the URI authority (always empty for local
/// workspaces) is skipped.
pub(super) fn workspace_path_from_uri(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    if path.is_empty() {
        return None;
    }
    Some(percent_decode(path))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Shapes one decoded generation into a [`LoadedEntry`]. Rows whose input,
/// output, cache-read, and thinking tokens are all zero carry no usage and
/// are skipped. Thinking tokens ride in `extra_total_tokens` (like other
/// agents with reasoning tokens) and are folded into billable output tokens
/// for pricing.
pub(super) fn generation_to_entry(
    record: &GenerationMetadata,
    context: &SessionContext,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<LoadedEntry> {
    if record.total_tokens() == 0 {
        return None;
    }
    let timestamp = record
        .timestamp_ms
        .map(TimestampMs::from_millis)
        .unwrap_or(context.fallback_timestamp);
    let timestamp_text = format_rfc3339_millis(timestamp);
    let usage = TokenUsageRaw {
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: record.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let billable_usage = TokenUsageRaw {
        output_tokens: record.output_tokens.saturating_add(record.thinking_tokens),
        ..usage
    };
    let model = record
        .raw_model
        .as_deref()
        .map(|raw| resolve_model_name(raw).into_owned());
    let cost = calculate_cost_for_usage(model.as_deref(), billable_usage, None, mode, pricing);
    let missing_pricing_model =
        missing_pricing_model_for_usage(model.as_deref(), billable_usage, None, mode, pricing);
    let data = UsageEntry {
        session_id: Some(context.session_id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: model.clone(),
            id: record.response_id.clone(),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    Some(LoadedEntry {
        date: format_date_tz(timestamp, tz),
        timestamp,
        project: std::sync::Arc::from(context.project.as_str()),
        session_id: std::sync::Arc::from(context.session_id.as_str()),
        project_path: std::sync::Arc::from(context.project_path.as_str()),
        cost,
        extra_total_tokens: record.thinking_tokens,
        credits: None,
        message_count: None,
        model,
        data,
        usage_limit_reset_time: None,
        missing_pricing_model,
    })
}

#[cfg(test)]
pub(super) mod encode {
    //! Tiny test-only protobuf encoder (varint + length-delimited fields).

    pub fn varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
        key(out, field, 0);
        varint(out, value);
    }

    pub fn len_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
        key(out, field, 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn key(out: &mut Vec<u8>, field: u64, wire: u64) {
        varint(out, field << 3 | wire);
    }

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    pub struct Generation {
        pub model: &'static str,
        pub timestamp: Option<(u64, u64)>,
        pub system_input: u64,
        pub fresh_input: u64,
        pub cache_read: u64,
        pub output: u64,
        pub thinking: u64,
        pub response_id: &'static str,
    }

    pub fn generation_blob(generation: &Generation) -> Vec<u8> {
        let mut chat_model = Vec::new();
        len_field(&mut chat_model, 19, generation.model.as_bytes());
        if let Some((seconds, nanos)) = generation.timestamp {
            let mut timestamp = Vec::new();
            varint_field(&mut timestamp, 1, seconds);
            varint_field(&mut timestamp, 2, nanos);
            let mut info = Vec::new();
            len_field(&mut info, 4, &timestamp);
            len_field(&mut chat_model, 9, &info);
        }
        let mut usage = Vec::new();
        varint_field(&mut usage, 1, generation.system_input);
        varint_field(&mut usage, 2, generation.fresh_input);
        varint_field(&mut usage, 5, generation.cache_read);
        varint_field(&mut usage, 9, generation.output);
        varint_field(&mut usage, 10, generation.thinking);
        len_field(&mut usage, 11, generation.response_id.as_bytes());
        len_field(&mut chat_model, 4, &usage);
        let mut blob = Vec::new();
        len_field(&mut blob, 1, &chat_model);
        blob
    }

    pub fn trajectory_blob(created: Option<(u64, u64)>, workspace_uri: &str) -> Vec<u8> {
        let mut blob = Vec::new();
        if let Some((seconds, nanos)) = created {
            let mut timestamp = Vec::new();
            varint_field(&mut timestamp, 1, seconds);
            varint_field(&mut timestamp, 2, nanos);
            len_field(&mut blob, 2, &timestamp);
        }
        let mut inner = Vec::new();
        len_field(&mut inner, 1, workspace_uri.as_bytes());
        len_field(&mut blob, 1, &inner);
        blob
    }
}

#[cfg(test)]
mod tests {
    use super::{encode::*, *};

    fn sample_generation() -> Generation {
        Generation {
            model: "gemini-3.1-pro-low",
            timestamp: Some((1_767_312_000, 429_476_000)),
            system_input: 1036,
            fresh_input: 6321,
            cache_read: 36491,
            output: 604,
            thinking: 117,
            response_id: "resp-1",
        }
    }

    #[test]
    fn decodes_all_generation_fields() {
        let metadata = decode_generation(&generation_blob(&sample_generation()));

        assert_eq!(metadata.raw_model.as_deref(), Some("gemini-3.1-pro-low"));
        assert_eq!(metadata.timestamp_ms, Some(1_767_312_000_429));
        assert_eq!(metadata.input_tokens, 1036 + 6321);
        assert_eq!(metadata.output_tokens, 604);
        assert_eq!(metadata.cache_read_tokens, 36491);
        assert_eq!(metadata.thinking_tokens, 117);
        assert_eq!(metadata.response_id.as_deref(), Some("resp-1"));
    }

    #[test]
    fn rejects_timestamp_with_out_of_range_nanos() {
        let generation = Generation {
            timestamp: Some((1_767_312_000, 1_000_000_000)),
            ..sample_generation()
        };
        let metadata = decode_generation(&generation_blob(&generation));

        assert_eq!(metadata.timestamp_ms, None);
    }

    #[test]
    fn decodes_trajectory_metadata() {
        let blob = trajectory_blob(
            Some((1_767_312_000, 207_595_000)),
            "file:///Users/test/My%20Projects/app",
        );
        let metadata = decode_trajectory_metadata(&blob);

        assert_eq!(metadata.created_ms, Some(1_767_312_000_207));
        assert_eq!(
            metadata.workspace_uri.as_deref(),
            Some("file:///Users/test/My%20Projects/app")
        );
    }

    #[test]
    fn workspace_uri_decodes_percent_escapes() {
        assert_eq!(
            workspace_path_from_uri("file:///Users/test/My%20Projects/app").as_deref(),
            Some("/Users/test/My Projects/app")
        );
        assert_eq!(
            workspace_path_from_uri("file:///home/user/%E2%82%ACrates").as_deref(),
            Some("/home/user/€rates")
        );
        assert_eq!(
            workspace_path_from_uri("file:///trailing%2").as_deref(),
            Some("/trailing%2")
        );
        assert_eq!(workspace_path_from_uri("vscode://file/app"), None);
    }

    #[test]
    fn session_context_uses_workspace_basename_as_project() {
        let trajectory = TrajectoryMetadata {
            created_ms: Some(1_767_312_000_207),
            workspace_uri: Some("file:///Users/test/My%20Projects/app".to_string()),
        };
        let context = session_context(
            Path::new("/data/conversations/conv-1.db"),
            &trajectory,
            Some(1_700_000_000_000),
        );

        assert_eq!(context.session_id, "conv-1");
        assert_eq!(context.project, "app");
        assert_eq!(context.project_path, "/Users/test/My Projects/app");
        assert_eq!(
            context.fallback_timestamp,
            TimestampMs::from_millis(1_767_312_000_207)
        );
    }

    #[test]
    fn session_context_falls_back_to_file_mtime_and_unknown_project() {
        let context = session_context(
            Path::new("/data/conversations/conv-2.db"),
            &TrajectoryMetadata::default(),
            Some(1_700_000_000_000),
        );

        assert_eq!(context.project, "unknown");
        assert_eq!(context.project_path, "unknown");
        assert_eq!(
            context.fallback_timestamp,
            TimestampMs::from_millis(1_700_000_000_000)
        );
    }

    #[test]
    fn maps_model_aliases_case_insensitively() {
        let cases = [
            ("model_placeholder_m26", "claude-opus-4-6"),
            ("MODEL_PLACEHOLDER_M26", "claude-opus-4-6"),
            ("model_placeholder_m35", "claude-sonnet-4-6"),
            ("model_placeholder_m36", "gemini-3.1-pro"),
            ("model_placeholder_m37", "gemini-3.1-pro"),
            ("model_placeholder_m16", "gemini-3.1-pro"),
            ("MODEL_PLACEHOLDER_M16", "gemini-3.1-pro"),
            ("model_placeholder_m18", "gemini-3-flash-preview"),
            ("model_placeholder_m84", "gemini-3-flash-preview"),
            ("model_placeholder_m47", "gemini-3-flash-preview"),
            ("model_placeholder_m132", "gemini-3.5-flash-high"),
            ("model_placeholder_m133", "gemini-3.5-flash-high"),
            ("model_placeholder_m187", "gemini-3.5-flash-extra-low"),
            ("model_placeholder_m20", "gemini-3.5-flash-medium"),
            ("model_openai_gpt_oss_120b_medium", "gpt-oss-120b-medium"),
            ("gemini-pro-default", "gemini-3.1-pro"),
            ("gemini-pro-agent", "gemini-3.1-pro"),
            ("gemini-3-flash-agent", "gemini-3.5-flash-high"),
            ("gemini-3-flash-a", "gemini-3.5-flash-high"),
            ("gemini-3-flash-b", "gemini-3.5-flash-high"),
            ("gemini-3-flash-c", "gemini-3-flash-preview"),
            ("gemini-3-flash", "gemini-3-flash-preview"),
            ("gemini-3.5-flash-low", "gemini-3.5-flash-medium"),
            ("gemini-3.1-pro-high", "gemini-3.1-pro"),
            ("gemini-3.1-pro-low", "gemini-3.1-pro"),
            ("gemini-3-pro-high", "gemini-3-pro"),
            ("gemini-3-pro-low", "gemini-3-pro"),
            ("claude-opus-4-6-thinking", "claude-opus-4-6"),
            ("claude-sonnet-4-6-thinking", "claude-sonnet-4-6"),
        ];
        for (raw, expected) in cases {
            assert_eq!(resolve_model_name(raw), expected, "raw: {raw}");
        }
    }

    #[test]
    fn passes_unknown_models_through_unchanged() {
        assert_eq!(resolve_model_name("gemini-9-ultra"), "gemini-9-ultra");
        assert_eq!(resolve_model_name("Some-Custom-Model"), "Some-Custom-Model");
    }

    #[test]
    fn skips_generation_with_all_zero_tokens() {
        let record = decode_generation(&generation_blob(&Generation {
            system_input: 0,
            fresh_input: 0,
            cache_read: 0,
            output: 0,
            thinking: 0,
            ..sample_generation()
        }));
        let context = session_context(
            Path::new("/data/conversations/conv.db"),
            &TrajectoryMetadata::default(),
            None,
        );

        assert!(generation_to_entry(&record, &context, None, CostMode::Display, None).is_none());
    }

    #[test]
    fn entry_carries_thinking_tokens_as_extra_total() {
        let record = decode_generation(&generation_blob(&sample_generation()));
        let context = session_context(
            Path::new("/data/conversations/conv.db"),
            &TrajectoryMetadata::default(),
            None,
        );
        let tz = crate::parse_tz(Some("UTC"));

        let entry =
            generation_to_entry(&record, &context, tz.as_ref(), CostMode::Display, None).unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 7357);
        assert_eq!(entry.data.message.usage.output_tokens, 604);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 36491);
        assert_eq!(entry.data.message.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.extra_total_tokens, 117);
        assert_eq!(entry.model.as_deref(), Some("gemini-3.1-pro"));
        assert_eq!(entry.data.message.id.as_deref(), Some("resp-1"));
        assert_eq!(entry.data.timestamp, "2026-01-02T00:00:00.429Z");
        assert_eq!(entry.date, "2026-01-02");
    }

    #[test]
    fn entry_falls_back_to_context_timestamp() {
        let record = decode_generation(&generation_blob(&Generation {
            timestamp: None,
            ..sample_generation()
        }));
        let trajectory = TrajectoryMetadata {
            created_ms: Some(1_767_312_000_207),
            workspace_uri: None,
        };
        let context = session_context(Path::new("/data/conversations/conv.db"), &trajectory, None);
        let tz = crate::parse_tz(Some("UTC"));

        let entry =
            generation_to_entry(&record, &context, tz.as_ref(), CostMode::Display, None).unwrap();

        assert_eq!(entry.timestamp, TimestampMs::from_millis(1_767_312_000_207));
        assert_eq!(entry.date, "2026-01-02");
    }
}
