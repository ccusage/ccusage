use std::{
    borrow::Cow,
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::LazyLock,
};

use memchr::memmem::Finder;
use serde::Deserialize;
use serde_json::Value;

use crate::{CodexRawUsage, CodexTokenUsageEvent, Result, TimestampMs};

use super::types::{
    CodexInfo, CodexLogEntry, CodexModelMetadata, CodexPayload, CodexResultFields,
    CodexSessionLogEntry, CodexTimestamp,
};

static EVENT_MSG_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"event_msg""#));
static TURN_CONTEXT_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"turn_context""#));
static TOKEN_COUNT_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"token_count""#));
static COMPACT_TYPE_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"#));
static TYPE_KEY_FINDER: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br#""type""#));
static USAGE_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""usage":"#));
static INPUT_TOKENS_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""input_tokens":"#));
static PROMPT_TOKENS_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""prompt_tokens":"#));
static THREAD_SPAWN_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(b"thread_spawn"));
static FORKED_FROM_ID_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(b"forked_from_id"));

const CODEX_AUTO_REVIEW_MODEL: &str = "codex-auto-review";
const CODEX_AUTO_REVIEW_FALLBACKS_JSON: &str = include_str!("codex-auto-review-fallbacks.json");

static CODEX_AUTO_REVIEW_FALLBACK_MODELS: LazyLock<Vec<CodexAutoReviewFallback<'static>>> =
    LazyLock::new(|| {
        serde_json::from_str(CODEX_AUTO_REVIEW_FALLBACKS_JSON)
            .expect("embedded codex-auto-review fallback snapshot must parse")
    });

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAutoReviewFallback<'a> {
    released_on: &'a str,
    model: &'a str,
}

#[derive(Clone, Copy)]
enum CodexLineKind {
    Session,
    Headless,
}

struct CodexExecTimestamps {
    event: String,
    model: String,
}

fn is_codex_replay_session(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16 * 1024];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    THREAD_SPAWN_FINDER.find(&buf[..n]).is_some() || FORKED_FROM_ID_FINDER.find(&buf[..n]).is_some()
}

fn detect_replay_second(path: &Path) -> Option<[u8; 19]> {
    let Ok(file) = fs::File::open(path) else {
        return None;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut first_second: Option<[u8; 19]> = None;

    loop {
        line.clear();
        let Ok(n) = reader.read_until(b'\n', &mut line) else {
            return None;
        };
        if n == 0 {
            break;
        }
        let Some(kind) = codex_line_usage_kind(&line) else {
            continue;
        };
        if !matches!(kind, CodexLineKind::Session) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<CodexSessionLogEntry<'_>>(&line) else {
            continue;
        };
        if value.entry_type.as_deref() != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.payload.as_ref() else {
            continue;
        };
        if payload.payload_type.as_deref() != Some("token_count") {
            continue;
        }
        let info = payload.info.as_ref();
        if info.and_then(|i| i.last_token_usage.as_ref()).is_none()
            && info.and_then(|i| i.total_token_usage.as_ref()).is_none()
        {
            continue;
        }
        let Some(ts) = codex_session_timestamp(value.timestamp.as_ref()) else {
            continue;
        };
        let ts_bytes = ts.as_bytes();
        let ts_second: [u8; 19] = ts_bytes.get(0..19).and_then(|s| s.try_into().ok())?;
        match first_second {
            None => {
                first_second = Some(ts_second);
            }
            Some(ref first) => {
                if first == &ts_second {
                    return Some(ts_second);
                }
                return None;
            }
        }
    }
    None
}

pub(super) fn visit_codex_session_file(
    sessions_dir: &Path,
    path: &Path,
    mut visit: impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let is_replay_session = is_codex_replay_session(path);
    let replay_second = is_replay_session
        .then(|| detect_replay_second(path))
        .flatten();
    let Ok(file) = fs::File::open(path) else {
        return Ok(());
    };
    let mut reader = BufReader::with_capacity(128 * 1024, file);
    let mut line = Vec::new();
    let session_id = codex_session_id(sessions_dir, path);
    let mut previous_totals: Option<CodexRawUsage> = None;
    let mut current_model: Option<String> = None;
    let mut current_model_is_fallback = false;
    let fallback_timestamp = file_modified_timestamp(path);
    let mut skip_replay = replay_second.is_some();

    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            return Ok(());
        };
        if bytes_read == 0 {
            break;
        }
        let Some(line_kind) = codex_line_usage_kind(&line) else {
            continue;
        };
        match line_kind {
            CodexLineKind::Session => {
                let Ok(value) = serde_json::from_slice::<CodexSessionLogEntry<'_>>(&line) else {
                    continue;
                };
                if let Some(ref replay_ts) = replay_second
                    && skip_replay
                    && value.entry_type.as_deref() == Some("event_msg")
                    && value
                        .payload
                        .as_ref()
                        .is_some_and(|p| p.payload_type.as_deref() == Some("token_count"))
                {
                    let Some(ts) = codex_session_timestamp(value.timestamp.as_ref()) else {
                        continue;
                    };
                    let matches_replay = ts
                        .as_bytes()
                        .get(0..19)
                        .is_some_and(|sec| sec.len() == 19 && sec == replay_ts.as_slice());
                    if matches_replay {
                        if let Some(total_usage) = value
                            .payload
                            .as_ref()
                            .and_then(|payload| payload.info.as_ref())
                            .and_then(|info| info.total_token_usage.as_ref())
                            .copied()
                        {
                            previous_totals.replace(total_usage);
                        }
                        continue;
                    }
                    skip_replay = false;
                }
                visit_codex_session_entry(
                    &session_id,
                    value,
                    &mut previous_totals,
                    &mut current_model,
                    &mut current_model_is_fallback,
                    &mut visit,
                )?;
            }
            CodexLineKind::Headless => {
                if let Ok(value) = serde_json::from_slice::<CodexLogEntry<'_>>(&line) {
                    add_codex_exec_event(
                        &session_id,
                        &value,
                        &fallback_timestamp,
                        &mut current_model,
                        &mut current_model_is_fallback,
                        &mut visit,
                    )?;
                } else {
                    add_codex_exec_event_from_value(
                        &session_id,
                        &line,
                        &fallback_timestamp,
                        &mut current_model,
                        &mut current_model_is_fallback,
                        &mut visit,
                    )?;
                };
            }
        }
    }

    Ok(())
}

fn visit_codex_session_entry(
    session_id: &str,
    value: CodexSessionLogEntry<'_>,
    previous_totals: &mut Option<CodexRawUsage>,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let entry_type = value.entry_type.as_deref();
    if entry_type == Some("turn_context") {
        if let Some(model) = value.payload.as_ref().and_then(codex_model_from_payload) {
            *current_model = Some(model);
            *current_model_is_fallback = false;
        }
        return Ok(());
    }
    if entry_type != Some("event_msg") {
        return Ok(());
    }
    let Some(timestamp) = codex_session_timestamp(value.timestamp.as_ref()) else {
        return Ok(());
    };
    let Some(payload) = value.payload.as_ref() else {
        return Ok(());
    };
    if payload.payload_type.as_deref() != Some("token_count") {
        return Ok(());
    }
    let info = payload.info.as_ref();
    let total_usage = info.and_then(|info| info.total_token_usage.as_ref().copied());
    let raw_usage = info
        .and_then(|info| info.last_token_usage.as_ref().copied())
        .or_else(|| {
            total_usage
                .as_ref()
                .map(|usage| subtract_codex_raw_usage(usage, previous_totals.as_ref()))
        });
    if let Some(total_usage) = total_usage {
        *previous_totals = Some(total_usage);
    }
    let Some(raw_usage) = raw_usage else {
        return Ok(());
    };
    if raw_usage.input_tokens == 0
        && raw_usage.cached_input_tokens == 0
        && raw_usage.output_tokens == 0
        && raw_usage.reasoning_output_tokens == 0
    {
        return Ok(());
    }

    let parsed_model =
        codex_model_from_payload(payload).or_else(|| info.and_then(codex_model_from_info));
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        &timestamp,
        current_model,
        current_model_is_fallback,
    );

    visit(CodexTokenUsageEvent {
        session_id: session_id.to_string(),
        timestamp,
        model,
        input_tokens: raw_usage.input_tokens,
        cached_input_tokens: raw_usage.cached_input_tokens.min(raw_usage.input_tokens),
        output_tokens: raw_usage.output_tokens,
        reasoning_output_tokens: raw_usage.reasoning_output_tokens,
        total_tokens: raw_usage.total_tokens,
        is_fallback_model,
    })
}

fn add_codex_exec_event(
    session_id: &str,
    value: &CodexLogEntry<'_>,
    fallback_timestamp: &str,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let Some(raw_usage) = normalize_headless_codex_usage(value) else {
        return Ok(());
    };
    let parsed_model = codex_model_from_result(value);
    let timestamps = CodexExecTimestamps {
        event: codex_timestamp_from_result(value).unwrap_or_else(|| fallback_timestamp.to_string()),
        model: codex_model_timestamp_from_result(value)
            .unwrap_or_else(|| fallback_timestamp.to_string()),
    };
    visit_codex_exec_usage_event(
        session_id,
        raw_usage,
        parsed_model,
        timestamps,
        current_model,
        current_model_is_fallback,
        visit,
    )
}

fn add_codex_exec_event_from_value(
    session_id: &str,
    line: &[u8],
    fallback_timestamp: &str,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return Ok(());
    };
    let Some(raw_usage) = normalize_headless_codex_usage_value(&value) else {
        return Ok(());
    };
    let parsed_model = codex_model_from_result_value(&value);
    let timestamps = CodexExecTimestamps {
        event: codex_timestamp_from_result_value(&value)
            .unwrap_or_else(|| fallback_timestamp.to_string()),
        model: codex_model_timestamp_from_result_value(&value)
            .unwrap_or_else(|| fallback_timestamp.to_string()),
    };
    visit_codex_exec_usage_event(
        session_id,
        raw_usage,
        parsed_model,
        timestamps,
        current_model,
        current_model_is_fallback,
        visit,
    )
}

fn visit_codex_exec_usage_event(
    session_id: &str,
    raw_usage: CodexRawUsage,
    parsed_model: Option<String>,
    timestamps: CodexExecTimestamps,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        &timestamps.model,
        current_model,
        current_model_is_fallback,
    );
    visit(CodexTokenUsageEvent {
        session_id: session_id.to_string(),
        timestamp: timestamps.event,
        model,
        input_tokens: raw_usage.input_tokens,
        cached_input_tokens: raw_usage.cached_input_tokens.min(raw_usage.input_tokens),
        output_tokens: raw_usage.output_tokens,
        reasoning_output_tokens: raw_usage.reasoning_output_tokens,
        total_tokens: raw_usage.total_tokens,
        is_fallback_model,
    })
}

fn codex_line_usage_kind(line: &[u8]) -> Option<CodexLineKind> {
    let has_event_msg = EVENT_MSG_TYPE_FINDER.find(line).is_some();
    let has_token_count = has_event_msg && TOKEN_COUNT_TYPE_FINDER.find(line).is_some();
    if TURN_CONTEXT_TYPE_FINDER.find(line).is_some() || has_token_count {
        return Some(CodexLineKind::Session);
    }
    let has_compact_type = COMPACT_TYPE_FIELD_FINDER.find(line).is_some();
    let has_nested_token_count = !has_event_msg
        && has_compact_type
        && line.len() < 64 * 1024
        && TOKEN_COUNT_TYPE_FINDER.find(line).is_some();
    if has_event_msg || has_nested_token_count || !has_compact_type {
        let (has_turn_context, has_event_msg, has_token_count) = codex_line_type_flags(line);
        if has_turn_context || (has_event_msg && has_token_count) {
            return Some(CodexLineKind::Session);
        }
    }
    if USAGE_FIELD_FINDER.find(line).is_some()
        || INPUT_TOKENS_FIELD_FINDER.find(line).is_some()
        || PROMPT_TOKENS_FIELD_FINDER.find(line).is_some()
    {
        return Some(CodexLineKind::Headless);
    }
    None
}

fn codex_line_type_flags(line: &[u8]) -> (bool, bool, bool) {
    let mut start = 0;
    let mut has_turn_context = false;
    let mut has_event_msg = false;
    let mut has_token_count = false;
    while let Some(index) = TYPE_KEY_FINDER.find(&line[start..]) {
        let key_start = start + index;
        let mut cursor = skip_json_whitespace(line, key_start + br#""type""#.len());
        if line.get(cursor) != Some(&b':') {
            start = key_start + br#""type""#.len();
            continue;
        }
        cursor = skip_json_whitespace(line, cursor + 1);
        if line.get(cursor) != Some(&b'"') {
            start = cursor.saturating_add(1);
            continue;
        }
        cursor += 1;
        has_turn_context |= json_string_value_matches(line, cursor, b"turn_context");
        has_event_msg |= json_string_value_matches(line, cursor, b"event_msg");
        has_token_count |= json_string_value_matches(line, cursor, b"token_count");
        if has_turn_context || (has_event_msg && has_token_count) {
            return (has_turn_context, has_event_msg, has_token_count);
        }
        start = cursor.saturating_add(1);
    }
    (has_turn_context, has_event_msg, has_token_count)
}

fn json_string_value_matches(line: &[u8], start: usize, value: &[u8]) -> bool {
    line.get(start..start + value.len())
        .is_some_and(|candidate| candidate == value)
        && line.get(start + value.len()) == Some(&b'"')
}

fn skip_json_whitespace(line: &[u8], mut index: usize) -> usize {
    while matches!(line.get(index), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        index += 1;
    }
    index
}

fn codex_session_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    match value? {
        CodexTimestamp::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        CodexTimestamp::Number(_) => normalize_codex_timestamp(value),
    }
}

fn parsed_model_is_missing(
    model: &Option<String>,
    current_model: &Option<String>,
    current_model_is_fallback: bool,
) -> bool {
    model.is_some() && current_model.is_some() && current_model_is_fallback
}

fn resolve_codex_usage_model(
    parsed_model: Option<String>,
    timestamp: &str,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
) -> (Option<String>, bool) {
    if let Some(model) = parsed_model.as_ref() {
        *current_model = Some(model.clone());
        *current_model_is_fallback = false;
    }
    let mut is_fallback_model = false;
    let model = parsed_model.or_else(|| current_model.clone()).or_else(|| {
        is_fallback_model = true;
        *current_model_is_fallback = true;
        *current_model = Some("gpt-5".to_string());
        current_model.clone()
    });
    if parsed_model_is_missing(&model, current_model, *current_model_is_fallback) {
        is_fallback_model = true;
    }
    let model = model.map(|model| {
        codex_log_model_fallback(&model, timestamp)
            .map(|fallback| {
                is_fallback_model = true;
                fallback.to_string()
            })
            .unwrap_or(model)
    });
    (model, is_fallback_model)
}

fn codex_log_model_fallback(model: &str, timestamp: &str) -> Option<&'static str> {
    if model != CODEX_AUTO_REVIEW_MODEL {
        return None;
    }
    let Some(date) = codex_timestamp_date(timestamp) else {
        return Some("gpt-5");
    };
    Some(
        codex_auto_review_fallback_models()
            .iter()
            .find_map(|fallback| (date >= fallback.released_on).then_some(fallback.model))
            .unwrap_or("gpt-5"),
    )
}

fn codex_auto_review_fallback_models() -> &'static [CodexAutoReviewFallback<'static>] {
    CODEX_AUTO_REVIEW_FALLBACK_MODELS.as_slice()
}

fn codex_timestamp_date(timestamp: &str) -> Option<&str> {
    let date = timestamp.get(..10)?;
    let bytes = date.as_bytes();
    if !(bytes.len() == 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit))
    {
        return None;
    }
    let year = codex_date_part(&bytes[0..4])?;
    let month = codex_date_part(&bytes[5..7])?;
    let day = codex_date_part(&bytes[8..10])?;
    let max_day = codex_days_in_month(year, month)?;
    (day >= 1 && day <= max_day).then_some(date)
}

fn codex_date_part(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = byte.checked_sub(b'0')?;
        (digit <= 9).then_some(value * 10 + u16::from(digit))
    })
}

fn codex_days_in_month(year: u16, month: u16) -> Option<u16> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if codex_is_leap_year(year) => Some(29),
        2 => Some(28),
        _ => None,
    }
}

fn codex_is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400)
}

fn codex_session_id(sessions_dir: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(sessions_dir).unwrap_or(path);
    let mut session_id = relative
        .with_extension("")
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    if session_id.is_empty() {
        session_id = "unknown".to_string();
    }
    session_id
}

fn codex_model_from_payload(value: &CodexPayload<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_info(value: &CodexInfo<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_result(value: &CodexLogEntry<'_>) -> Option<String> {
    codex_model_from_entry(value)
        .or_else(|| value.data.as_ref().and_then(codex_model_from_result_fields))
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(codex_model_from_result_fields)
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(codex_model_from_result_fields)
        })
}

fn codex_model_from_result_fields(value: &CodexResultFields<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_entry(value: &CodexLogEntry<'_>) -> Option<String> {
    codex_model_from_parts(
        value.model.as_ref(),
        value.model_name.as_ref(),
        value.metadata.as_ref(),
    )
}

fn codex_model_from_parts(
    model: Option<&Cow<'_, str>>,
    model_name: Option<&Cow<'_, str>>,
    metadata: Option<&CodexModelMetadata<'_>>,
) -> Option<String> {
    non_empty_cow_string(model)
        .or_else(|| non_empty_cow_string(model_name))
        .or_else(|| metadata.and_then(|metadata| non_empty_cow_string(metadata.model.as_ref())))
}

fn codex_model_from_result_value(value: &Value) -> Option<String> {
    codex_model_from_value_fields(value)
        .or_else(|| value.get("data").and_then(codex_model_from_value_fields))
        .or_else(|| value.get("result").and_then(codex_model_from_value_fields))
        .or_else(|| {
            value
                .get("response")
                .and_then(codex_model_from_value_fields)
        })
}

fn codex_model_from_value_fields(value: &Value) -> Option<String> {
    non_empty_value_string(value.get("model"))
        .or_else(|| non_empty_value_string(value.get("model_name")))
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|metadata| non_empty_value_string(metadata.get("model")))
        })
}

fn non_empty_cow_string(value: Option<&Cow<'_, str>>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn non_empty_value_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn usage_from_result(value: &CodexLogEntry<'_>) -> Option<CodexRawUsage> {
    value
        .usage
        .as_ref()
        .copied()
        .or_else(|| {
            value
                .data
                .as_ref()
                .and_then(|data| data.usage.as_ref().copied())
        })
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(|result| result.usage.as_ref().copied())
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(|response| response.usage.as_ref().copied())
        })
}

fn usage_from_result_value(value: &Value) -> Option<CodexRawUsage> {
    usage_from_value(value.get("usage"))
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| usage_from_value(data.get("usage")))
        })
        .or_else(|| {
            value
                .get("result")
                .and_then(|result| usage_from_value(result.get("usage")))
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| usage_from_value(response.get("usage")))
        })
}

fn usage_from_value(value: Option<&Value>) -> Option<CodexRawUsage> {
    serde_json::from_value(value?.clone()).ok()
}

fn codex_timestamp_from_result(value: &CodexLogEntry<'_>) -> Option<String> {
    normalize_codex_timestamp(value.timestamp.as_ref())
        .or_else(|| normalize_codex_timestamp(value.created_at.as_ref()))
        .or_else(|| normalize_codex_timestamp(value.created_at_camel.as_ref()))
        .or_else(|| {
            value
                .data
                .as_ref()
                .and_then(|data| normalize_result_fields_timestamp(data))
        })
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(|result| normalize_result_fields_timestamp(result))
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(|response| normalize_result_fields_timestamp(response))
        })
}

fn codex_model_timestamp_from_result(value: &CodexLogEntry<'_>) -> Option<String> {
    raw_or_normalized_codex_timestamp(value.timestamp.as_ref())
        .or_else(|| raw_or_normalized_codex_timestamp(value.created_at.as_ref()))
        .or_else(|| raw_or_normalized_codex_timestamp(value.created_at_camel.as_ref()))
        .or_else(|| {
            value
                .data
                .as_ref()
                .and_then(raw_or_normalized_result_fields_timestamp)
        })
        .or_else(|| {
            value
                .result
                .as_ref()
                .and_then(raw_or_normalized_result_fields_timestamp)
        })
        .or_else(|| {
            value
                .response
                .as_ref()
                .and_then(raw_or_normalized_result_fields_timestamp)
        })
}

fn codex_timestamp_from_result_value(value: &Value) -> Option<String> {
    normalize_value_fields_timestamp(value)
        .or_else(|| value.get("data").and_then(normalize_value_fields_timestamp))
        .or_else(|| {
            value
                .get("result")
                .and_then(normalize_value_fields_timestamp)
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(normalize_value_fields_timestamp)
        })
}

fn codex_model_timestamp_from_result_value(value: &Value) -> Option<String> {
    raw_or_normalized_value_fields_timestamp(value)
        .or_else(|| {
            value
                .get("data")
                .and_then(raw_or_normalized_value_fields_timestamp)
        })
        .or_else(|| {
            value
                .get("result")
                .and_then(raw_or_normalized_value_fields_timestamp)
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(raw_or_normalized_value_fields_timestamp)
        })
}

fn normalize_result_fields_timestamp(value: &CodexResultFields<'_>) -> Option<String> {
    normalize_codex_timestamp(value.timestamp.as_ref())
        .or_else(|| normalize_codex_timestamp(value.created_at.as_ref()))
        .or_else(|| normalize_codex_timestamp(value.created_at_camel.as_ref()))
}

fn raw_or_normalized_result_fields_timestamp(value: &CodexResultFields<'_>) -> Option<String> {
    raw_or_normalized_codex_timestamp(value.timestamp.as_ref())
        .or_else(|| raw_or_normalized_codex_timestamp(value.created_at.as_ref()))
        .or_else(|| raw_or_normalized_codex_timestamp(value.created_at_camel.as_ref()))
}

fn normalize_value_fields_timestamp(value: &Value) -> Option<String> {
    normalize_value_timestamp(value.get("timestamp"))
        .or_else(|| normalize_value_timestamp(value.get("created_at")))
        .or_else(|| normalize_value_timestamp(value.get("createdAt")))
}

fn raw_or_normalized_value_fields_timestamp(value: &Value) -> Option<String> {
    raw_or_normalized_value_timestamp(value.get("timestamp"))
        .or_else(|| raw_or_normalized_value_timestamp(value.get("created_at")))
        .or_else(|| raw_or_normalized_value_timestamp(value.get("createdAt")))
}

fn raw_or_normalized_codex_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    match value? {
        CodexTimestamp::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            if codex_timestamp_date(text).is_some() {
                return Some(text.to_string());
            }
            // Malformed string: try parsing-based normalization. If that also
            // fails, return None so the caller's `or_else` chain can try the
            // next available timestamp field instead of locking in a string
            // that downstream date resolution will reject.
            normalize_codex_timestamp(value)
        }
        CodexTimestamp::Number(_) => normalize_codex_timestamp(value),
    }
}

fn normalize_codex_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    match value? {
        CodexTimestamp::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            crate::parse_ts_timestamp(text).map(crate::format_rfc3339_millis)
        }
        CodexTimestamp::Number(raw) => {
            let millis = if *raw > 10_000_000_000 {
                *raw
            } else {
                raw.checked_mul(1_000)?
            };
            Some(crate::format_rfc3339_millis(TimestampMs::from_millis(
                millis.min(i64::MAX as u64) as i64,
            )))
        }
    }
}

fn raw_or_normalized_value_timestamp(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if codex_timestamp_date(text).is_some() {
            return Some(text.to_string());
        }
        // Malformed string: try parsing-based normalization. If that also
        // fails, return None so the caller's `or_else` chain can try the
        // next available timestamp field instead of locking in a string
        // that downstream date resolution will reject.
        return normalize_value_timestamp(Some(value));
    }
    normalize_value_timestamp(Some(value))
}

fn normalize_value_timestamp(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return crate::parse_ts_timestamp(text).map(crate::format_rfc3339_millis);
    }
    let raw = value.as_u64()?;
    let millis = if raw > 10_000_000_000 {
        raw
    } else {
        raw.checked_mul(1_000)?
    };
    Some(crate::format_rfc3339_millis(TimestampMs::from_millis(
        millis.min(i64::MAX as u64) as i64,
    )))
}

fn normalize_headless_codex_usage(value: &CodexLogEntry<'_>) -> Option<CodexRawUsage> {
    let usage = usage_from_result(value)?;
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_output_tokens == 0
        && usage.total_tokens == 0
    {
        return None;
    }
    Some(usage)
}

fn normalize_headless_codex_usage_value(value: &Value) -> Option<CodexRawUsage> {
    let usage = usage_from_result_value(value)?;
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_output_tokens == 0
        && usage.total_tokens == 0
    {
        return None;
    }
    Some(usage)
}

fn file_modified_timestamp(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| {
            crate::format_rfc3339_millis(TimestampMs::from_millis(
                duration.as_millis().min(i64::MAX as u128) as i64,
            ))
        })
        .unwrap_or_else(|| crate::format_rfc3339_millis(TimestampMs::UNIX_EPOCH))
}

fn subtract_codex_raw_usage(
    current: &CodexRawUsage,
    previous: Option<&CodexRawUsage>,
) -> CodexRawUsage {
    CodexRawUsage {
        input_tokens: current
            .input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.input_tokens)),
        cached_input_tokens: current
            .cached_input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cached_input_tokens)),
        output_tokens: current
            .output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.output_tokens)),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.reasoning_output_tokens)),
        total_tokens: current
            .total_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.total_tokens)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_codex_auto_review_fallbacks_from_models_dev_snapshot() {
        let fallbacks = codex_auto_review_fallback_models();

        assert_eq!(fallbacks.len(), 7);
        assert_eq!(fallbacks[0].released_on, "2026-04-23");
        assert_eq!(fallbacks[0].model, "gpt-5.5");
        assert_eq!(fallbacks[6].released_on, "2025-08-07");
        assert_eq!(fallbacks[6].model, "gpt-5");
        assert!(
            fallbacks
                .windows(2)
                .all(|window| window[0].released_on > window[1].released_on)
        );
    }
}
