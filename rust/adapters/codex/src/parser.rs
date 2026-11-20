use ccusage_adapter_common::{json::JsonText, jsonl};
use ccusage_core::{Diagnostics, Skip, SkipKind, fast::LinePrefilter};
use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    path::Path,
    sync::LazyLock,
};

use memchr::memmem::Finder;
use serde_json::Value;

use crate::{
    CodexRawUsage, CodexServiceTier, CodexTokenUsageEvent, TimestampMs, parse_ts_timestamp,
};

use super::{
    reader::{READ_BUFFER_SIZE, rollout_policy, uncompressed_rollout_path},
    replay::{CodexSessionMetadata, session_metadata_from_line},
    types::{CodexLogEntry, CodexTimestamp, CodexUsageCandidate},
};

mod exec;
mod model;
mod replay_filter;
mod timestamp;
mod usage;

use exec::add_codex_exec_event;
use model::{codex_model_from_info, codex_model_from_payload, resolve_codex_usage_model};
use replay_filter::{CodexOwnershipMarker, CodexReplayFilter, CodexReplayResolution};
use timestamp::{codex_session_timestamp, codex_usage_timestamp, file_modified_timestamp};
use usage::{codex_token_count_usage, malformed_token_count_field, normalize_codex_raw_usage};

static TYPE_KEY_FINDER: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br#""type""#));
static KEY_END_FINDER: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br#"":"#));
static ESCAPED_USAGE_MARKERS: LazyLock<LinePrefilter> = LazyLock::new(|| {
    LinePrefilter::any(&[
        br#""type""#,
        b"turn_context",
        b"event_msg",
        b"token_count",
        b"thread_settings_applied",
        b"token_usage_record",
        b"compacted",
        br#""usage""#,
        br#""input_tokens""#,
        br#""prompt_tokens""#,
    ])
});
static SESSION_META_MARKER: LazyLock<LinePrefilter> =
    LazyLock::new(|| LinePrefilter::any(&[b"session_meta"]));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodexLineKind {
    Session,
    Headless,
    Escaped,
}

#[derive(Default)]
struct CodexCompactionUsageState {
    compacted_response_ids: HashSet<String>,
    pending_usage: HashMap<String, CodexPendingUsage>,
    emitted_response_ids: HashSet<String>,
    latest_usage_response_id: Option<String>,
    latest_record_awaits_snapshot: bool,
    recorded_usage_timestamps: Option<HashMap<String, Option<TimestampMs>>>,
}

struct CodexPendingUsage {
    event: CodexTokenUsageEvent,
    thread_token_usage: Option<CodexRawUsage>,
}

#[derive(Default)]
struct CodexSessionUsageState {
    previous_totals: Option<CodexRawUsage>,
    current_model: Option<String>,
    current_model_is_fallback: bool,
    current_service_tier: Option<CodexServiceTier>,
    compaction_usage: CodexCompactionUsageState,
}

struct CodexTokenCountUsage {
    total_usage: Option<CodexRawUsage>,
    cumulative_advanced: bool,
    delta: Option<CodexRawUsage>,
}

#[derive(Clone, Copy)]
pub(super) struct CodexForkReplay<'a> {
    pub(super) parent_usage: &'a [CodexRawUsage],
    pub(super) missing_parent: Option<&'a str>,
    pub(super) child_id: Option<&'a str>,
    // NOTE(2026-11-09, Codex VS Code 0.164.0-alpha.11.1, #2051): Spawned subagents drop inherited `token_usage_record` lines, so the first record ends replay even if the timestamps coincide. User forks copy those lines.
    pub(super) ends_at_usage_record: bool,
}

pub(super) fn visit_codex_session_file_with_compaction_history(
    sessions_dir: &Path,
    path: &Path,
    fork_replay: Option<CodexForkReplay<'_>>,
    compaction_history: Option<&mut HashMap<String, Option<TimestampMs>>>,
    visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
) {
    visit_codex_session_file_inner(
        sessions_dir,
        path,
        fork_replay,
        compaction_history,
        visit,
        diagnostics,
        None,
    );
}

#[cfg(test)]
pub(super) fn visit_codex_session_file_with_diagnostics(
    sessions_dir: &Path,
    path: &Path,
    fork_replay: Option<CodexForkReplay<'_>>,
    visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
) {
    visit_codex_session_file_inner(
        sessions_dir,
        path,
        fork_replay,
        None,
        visit,
        diagnostics,
        None,
    );
}

pub(super) fn visit_codex_session_file_unless_fork(
    sessions_dir: &Path,
    path: &Path,
    visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
) -> Option<CodexSessionMetadata> {
    let mut metadata = CodexSessionMetadata::default();
    let opened = visit_codex_session_file_inner(
        sessions_dir,
        path,
        None,
        None,
        visit,
        diagnostics,
        Some((&mut metadata, true)),
    );
    opened.then_some(metadata)
}

pub(super) fn visit_codex_session_file_with_metadata(
    sessions_dir: &Path,
    path: &Path,
    fork_replay: Option<CodexForkReplay<'_>>,
    visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
) -> Option<CodexSessionMetadata> {
    let mut metadata = CodexSessionMetadata::default();
    let opened = visit_codex_session_file_inner(
        sessions_dir,
        path,
        fork_replay,
        None,
        visit,
        diagnostics,
        Some((&mut metadata, false)),
    );
    opened.then_some(metadata)
}

fn visit_codex_session_file_inner(
    sessions_dir: &Path,
    path: &Path,
    fork_replay: Option<CodexForkReplay<'_>>,
    compaction_history: Option<&mut HashMap<String, Option<TimestampMs>>>,
    mut visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
    mut first_line: Option<(&mut CodexSessionMetadata, bool)>,
) -> bool {
    let session_id = codex_session_id(sessions_dir, path);
    let mut state = CodexSessionUsageState::default();
    if compaction_history.is_some() {
        state.compaction_usage.recorded_usage_timestamps = Some(HashMap::new());
    }
    let mut fallback_timestamp = None;
    let mut replay = CodexReplayFilter::new(fork_replay);
    let mut missing_parent = None;
    let mut seen_session_meta = false;

    let result = jsonl::read(
        path,
        Some(diagnostics),
        rollout_policy(path, READ_BUFFER_SIZE, true),
        |current, opened_path, _| {
            let line = current.raw;
            let mut stop_at_fork = false;
            let first_metadata = first_line.take().map(|(metadata, stop)| {
                stop_at_fork = stop;
                *metadata = session_metadata_from_line(path, line);
                metadata.clone()
            });
            if stop_at_fork
                && first_metadata
                    .as_ref()
                    .is_some_and(CodexSessionMetadata::is_fork)
            {
                return ControlFlow::Break(());
            }
            let has_metadata = first_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.is_session_meta);
            if has_metadata
                || (!seen_session_meta
                    && !replay.is_done()
                    && SESSION_META_MARKER.matches(line)
                    && session_metadata_from_line(path, line).is_session_meta)
            {
                seen_session_meta = true;
                return ControlFlow::Continue(());
            }
            let Some(line_kind) = codex_line_usage_kind(line) else {
                return ControlFlow::Continue(());
            };
            let text = JsonText::new(line);
            let value = match text.parse::<CodexLogEntry<'_>>() {
                Ok(value) => value,
                Err(error) => {
                    if line_kind == CodexLineKind::Escaped {
                        return ControlFlow::Continue(());
                    }
                    jsonl::record_decode_error(
                        line,
                        error,
                        path,
                        current.number(),
                        current.ending(),
                        diagnostics,
                        Some(is_usage_value),
                    );
                    return ControlFlow::Continue(());
                }
            };
            let location = RecordLocation {
                path,
                line: current.number(),
                diagnostics,
            };
            if is_session_usage_type(
                value.entry_type.as_deref(),
                value
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.payload_type.as_deref()),
            ) {
                if let Some(marker) = ownership_marker(&value)
                    && let Some(CodexReplayResolution::MissingParent(parent_id)) = replay
                        .observe_marker(
                            marker,
                            value
                                .payload
                                .as_ref()
                                .and_then(|payload| payload.thread_id.as_deref()),
                            &mut visit,
                        )
                {
                    missing_parent = Some(parent_id);
                }
                visit_codex_session_entry(
                    &session_id,
                    value,
                    &mut state,
                    &mut replay,
                    &mut visit,
                    &location,
                );
            } else {
                add_codex_exec_event(
                    &session_id,
                    &value,
                    || {
                        *fallback_timestamp
                            .get_or_insert_with(|| file_modified_timestamp(opened_path))
                    },
                    &mut state.current_model,
                    &mut state.current_model_is_fallback,
                    &mut |event| replay.visit(event, &mut visit),
                    &location,
                );
            }
            ControlFlow::Continue(())
        },
    );
    match result {
        Err(_) => return false,
        Ok(Some(())) => return true,
        Ok(None) => {}
    }

    if let Some(CodexReplayResolution::MissingParent(parent_id)) = replay.finish(&mut visit) {
        missing_parent = Some(parent_id);
    }
    if let Some(parent_id) = missing_parent {
        diagnostics.record(Skip::unit(
            path,
            SkipKind::MissingForkParent,
            format_args!("parent session {parent_id}"),
        ));
    }

    if let Some(history) = compaction_history
        && let Some(mut timestamps) = state.compaction_usage.recorded_usage_timestamps
    {
        timestamps.retain(|response_id, _| {
            state
                .compaction_usage
                .compacted_response_ids
                .contains(response_id)
        });
        history.extend(timestamps);
    }
    true
}

fn ownership_marker(value: &CodexLogEntry<'_>) -> Option<CodexOwnershipMarker> {
    match value.entry_type.as_deref() {
        Some("token_usage_record") => Some(CodexOwnershipMarker::UsageRecord),
        Some("event_msg")
            if value
                .payload
                .as_ref()
                .and_then(|payload| payload.payload_type.as_deref())
                == Some("thread_settings_applied") =>
        {
            Some(CodexOwnershipMarker::ThreadSettings)
        }
        _ => None,
    }
}

fn is_session_usage_type(entry_type: Option<&str>, payload_type: Option<&str>) -> bool {
    matches!(
        entry_type,
        Some("turn_context" | "token_usage_record" | "compacted")
    ) || (entry_type == Some("event_msg")
        && matches!(
            payload_type,
            Some("token_count" | "thread_settings_applied")
        ))
}

fn visit_codex_session_entry(
    session_id: &str,
    value: CodexLogEntry<'_>,
    state: &mut CodexSessionUsageState,
    replay: &mut CodexReplayFilter<'_>,
    visit: &mut impl FnMut(CodexTokenUsageEvent),
    location: &RecordLocation<'_>,
) {
    let CodexSessionUsageState {
        previous_totals,
        current_model,
        current_model_is_fallback,
        current_service_tier,
        compaction_usage,
    } = state;
    let entry_type = value.entry_type.as_deref();
    if entry_type == Some("compacted") {
        let Some(response_id) = value
            .payload
            .as_ref()
            .and_then(|payload| payload.compaction_response_id.as_deref())
            .map(str::trim)
            .filter(|response_id| !response_id.is_empty())
        else {
            return;
        };
        let response_id = response_id.to_string();
        compaction_usage
            .compacted_response_ids
            .insert(response_id.clone());
        if let Some(pending) = compaction_usage.pending_usage.remove(&response_id)
            && compaction_usage.emitted_response_ids.insert(response_id)
        {
            replay.visit(pending.event, visit);
        }
        return;
    }
    if entry_type == Some("token_usage_record") {
        let Some(payload) = value.payload.as_ref() else {
            return;
        };
        let Some(response_id) = payload
            .response_id
            .as_deref()
            .map(str::trim)
            .filter(|response_id| !response_id.is_empty())
        else {
            return;
        };
        let response_id = response_id.to_string();
        if compaction_usage.emitted_response_ids.contains(&response_id) {
            return;
        }
        let raw_usage = match payload.usage {
            CodexUsageCandidate::Missing => return,
            CodexUsageCandidate::Malformed => {
                location.invalid_usage("field `payload.usage` did not decode");
                return;
            }
            CodexUsageCandidate::Decoded(usage) => normalize_codex_raw_usage(usage),
        };
        if payload.thread_token_usage == CodexUsageCandidate::Malformed {
            location.invalid_usage("field `payload.thread_token_usage` did not decode");
            return;
        }
        let Some((timestamp, last_activity)) = codex_usage_timestamp(value.timestamp.as_ref())
        else {
            location.rejected_timestamp(value.timestamp.as_ref());
            return;
        };
        if raw_usage.is_empty() {
            return;
        }

        if let Some(timestamps) = compaction_usage.recorded_usage_timestamps.as_mut() {
            timestamps
                .entry(response_id.clone())
                .or_insert(Some(timestamp));
        }

        let parsed_model = codex_model_from_payload(payload);
        let model_was_inferred = parsed_model.is_none();
        let mut request_model = current_model.clone();
        let mut request_model_is_fallback = *current_model_is_fallback;
        let (model, is_fallback_model) = resolve_codex_usage_model(
            parsed_model,
            timestamp,
            &mut request_model,
            &mut request_model_is_fallback,
        );
        let event = CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            response_id: Some(response_id.clone()),
            record_response_id: None,
            timestamp,
            last_activity,
            model,
            input_tokens: raw_usage.input_tokens,
            cached_input_tokens: raw_usage.cached_input_tokens,
            cache_creation_tokens: raw_usage.cache_creation_tokens,
            output_tokens: raw_usage.output_tokens,
            reasoning_output_tokens: raw_usage.reasoning_output_tokens,
            total_tokens: raw_usage.total_tokens,
            is_fallback_model: is_fallback_model || model_was_inferred,
            service_tier: *current_service_tier,
        };

        compaction_usage.latest_usage_response_id = Some(response_id.clone());
        compaction_usage.latest_record_awaits_snapshot = true;

        if compaction_usage
            .compacted_response_ids
            .contains(&response_id)
        {
            if compaction_usage.emitted_response_ids.insert(response_id) {
                replay.visit(event, visit);
            }
        } else {
            compaction_usage
                .pending_usage
                .entry(response_id)
                .or_insert(CodexPendingUsage {
                    event,
                    thread_token_usage: payload
                        .thread_token_usage
                        .decoded()
                        .map(normalize_codex_raw_usage),
                });
        }
        return;
    }
    if entry_type == Some("turn_context") {
        if let Some(model) = value.payload.as_ref().and_then(codex_model_from_payload) {
            *current_model = Some(model);
            *current_model_is_fallback = false;
        }
        return;
    }
    if entry_type != Some("event_msg") {
        return;
    }
    let Some(payload) = value.payload.as_ref() else {
        return;
    };
    if payload.payload_type.as_deref() == Some("thread_settings_applied") {
        if codex_session_timestamp(value.timestamp.as_ref()).is_none() {
            return;
        }
        // NOTE(2026-07-26, Codex CLI 0.144.0, #1438): Auto-review rollouts can emit `thread_settings_applied` without `service_tier`. Keep the last recorded tier when that key is absent.
        if let Some(recorded) = payload
            .thread_settings
            .as_ref()
            .and_then(|settings| settings.service_tier.as_deref())
        {
            *current_service_tier = CodexServiceTier::from_codex_name(recorded);
        }
        return;
    }
    if payload.payload_type.as_deref() != Some("token_count") {
        return;
    }
    let info = payload.info.as_ref();
    if let Some(field) = info.and_then(malformed_token_count_field) {
        codex_token_count_usage(info, previous_totals);
        location.invalid_usage(field);
        return;
    }
    let parsed_timestamp = codex_usage_timestamp(value.timestamp.as_ref());
    let CodexTokenCountUsage {
        total_usage,
        cumulative_advanced,
        delta,
    } = codex_token_count_usage(info, previous_totals);
    let record_awaits_snapshot =
        cumulative_advanced && std::mem::take(&mut compaction_usage.latest_record_awaits_snapshot);
    let Some((timestamp, last_activity)) = parsed_timestamp else {
        if info.is_some_and(|info| {
            info.last_token_usage != CodexUsageCandidate::Missing
                || info.total_token_usage != CodexUsageCandidate::Missing
        }) {
            location.rejected_timestamp(value.timestamp.as_ref());
        }
        return;
    };
    let Some(raw_usage) = delta else {
        return;
    };
    if raw_usage.is_empty() {
        return;
    }

    // NOTE(2026-10-02, #1821): Local compaction emits an advancing `token_count` between its response record and compaction marker. Remote v2 omits that snapshot, so its usage can be missing from the cumulative counts.
    let mut record_response_id = None;
    if cumulative_advanced
        && let Some(response_id) = compaction_usage.latest_usage_response_id.as_ref()
        && compaction_usage
            .pending_usage
            .get(response_id)
            .is_some_and(|pending| {
                pending.event.raw_usage() == raw_usage
                    || pending.thread_token_usage.zip(total_usage).is_some_and(
                        |(recorded, total)| recorded == normalize_codex_raw_usage(total),
                    )
            })
    {
        record_response_id = record_awaits_snapshot.then(|| response_id.as_str().into());
        compaction_usage.pending_usage.remove(response_id);
        compaction_usage
            .emitted_response_ids
            .insert(response_id.clone());
    }

    let parsed_model =
        codex_model_from_payload(payload).or_else(|| info.and_then(codex_model_from_info));
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        timestamp,
        current_model,
        current_model_is_fallback,
    );

    replay.visit(
        CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            response_id: None,
            record_response_id,
            timestamp,
            last_activity,
            model,
            input_tokens: raw_usage.input_tokens,
            cached_input_tokens: raw_usage.cached_input_tokens,
            cache_creation_tokens: raw_usage.cache_creation_tokens,
            output_tokens: raw_usage.output_tokens,
            reasoning_output_tokens: raw_usage.reasoning_output_tokens,
            total_tokens: raw_usage.total_tokens,
            is_fallback_model,
            service_tier: *current_service_tier,
        },
        visit,
    )
}

fn codex_line_usage_kind(line: &[u8]) -> Option<CodexLineKind> {
    verbatim_line_usage_kind(line).or_else(|| {
        ESCAPED_USAGE_MARKERS
            .matches_escaped(line)
            .then_some(CodexLineKind::Escaped)
    })
}

fn verbatim_line_usage_kind(line: &[u8]) -> Option<CodexLineKind> {
    let (
        has_turn_context,
        has_event_msg,
        has_token_count,
        has_thread_settings_applied,
        has_token_usage_record,
        has_compacted,
    ) = codex_line_type_flags(line);
    if has_turn_context
        || has_token_usage_record
        || has_compacted
        || (has_event_msg && (has_token_count || has_thread_settings_applied))
    {
        return Some(CodexLineKind::Session);
    }
    has_headless_usage_field(line).then_some(CodexLineKind::Headless)
}

fn has_headless_usage_field(line: &[u8]) -> bool {
    KEY_END_FINDER.find_iter(line).any(|end| {
        let key = &line[..end];
        key.ends_with(br#""usage"#)
            || key.ends_with(br#""input_tokens"#)
            || key.ends_with(br#""prompt_tokens"#)
    })
}

fn codex_line_type_flags(line: &[u8]) -> (bool, bool, bool, bool, bool, bool) {
    let mut start = 0;
    let mut has_turn_context = false;
    let mut has_event_msg = false;
    let mut has_token_count = false;
    let mut has_thread_settings_applied = false;
    let mut has_token_usage_record = false;
    let mut has_compacted = false;
    while start < line.len()
        && let Some(index) = TYPE_KEY_FINDER.find(&line[start..])
    {
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
        has_thread_settings_applied |=
            json_string_value_matches(line, cursor, b"thread_settings_applied");
        has_token_usage_record |= json_string_value_matches(line, cursor, b"token_usage_record");
        has_compacted |= json_string_value_matches(line, cursor, b"compacted");
        if has_turn_context
            || has_token_usage_record
            || has_compacted
            || (has_event_msg && (has_token_count || has_thread_settings_applied))
        {
            return (
                has_turn_context,
                has_event_msg,
                has_token_count,
                has_thread_settings_applied,
                has_token_usage_record,
                has_compacted,
            );
        }
        start = cursor.saturating_add(1);
    }
    (
        has_turn_context,
        has_event_msg,
        has_token_count,
        has_thread_settings_applied,
        has_token_usage_record,
        has_compacted,
    )
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

pub(super) fn codex_session_id(sessions_dir: &Path, path: &Path) -> String {
    let relative = path.trim_prefix(sessions_dir);
    let mut session_id = uncompressed_rollout_path(relative)
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

pub(super) fn codex_timestamp_instant(value: Option<&CodexTimestamp<'_>>) -> Option<TimestampMs> {
    match value? {
        CodexTimestamp::String(text) => parse_ts_timestamp(text.trim()),
        CodexTimestamp::Invalid => None,
        CodexTimestamp::Number(raw) => {
            let millis = if *raw > 10_000_000_000 {
                *raw
            } else {
                raw.checked_mul(1_000)?
            };
            TimestampMs::from_instant_millis(i64::try_from(millis).ok()?)
        }
    }
}

fn is_usage_value(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("token_usage_record") => true,
        Some("event_msg") => {
            value.pointer("/payload/type").and_then(Value::as_str) == Some("token_count")
                && value
                    .pointer("/payload/info")
                    .is_some_and(|info| !info.is_null())
        }
        _ => false,
    }
}

struct RecordLocation<'a> {
    path: &'a Path,
    line: u32,
    diagnostics: &'a Diagnostics,
}
impl RecordLocation<'_> {
    fn invalid_usage(&self, detail: &'static str) {
        self.diagnostics.record(Skip::record(
            self.path,
            self.line,
            SkipKind::InvalidData,
            detail,
        ));
    }

    fn rejected_timestamp(&self, timestamp: Option<&CodexTimestamp<'_>>) {
        if matches!(timestamp, Some(CodexTimestamp::Invalid)) {
            self.invalid_usage("field `timestamp` did not decode");
            return;
        }
        self.diagnostics.record(Skip::record(
            self.path,
            self.line,
            SkipKind::InvalidTimestamp,
            "invalid usage timestamp",
        ));
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod diagnostic_tests;
