use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::Path,
    sync::LazyLock,
};

use memchr::memmem::Finder;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    CodexRawUsage, CodexServiceTier, CodexTokenUsageEvent, Result, TimestampMs, parse_ts_timestamp,
};

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
static THREAD_SETTINGS_APPLIED_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"thread_settings_applied""#));
static TOKEN_USAGE_RECORD_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"token_usage_record""#));
static COMPACTED_TYPE_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"compacted""#));
static THREAD_SETTINGS_APPLIED_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(b"thread_settings_applied"));
static COMPACT_TYPE_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""type":"#));
static TYPE_KEY_FINDER: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br#""type""#));
static USAGE_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""usage":"#));
static INPUT_TOKENS_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""input_tokens":"#));
static PROMPT_TOKENS_FIELD_FINDER: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br#""prompt_tokens":"#));
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

#[derive(Default)]
struct CodexCompactionUsageState {
    compacted_response_ids: HashSet<String>,
    pending_usage: HashMap<String, CodexPendingUsage>,
    emitted_response_ids: HashSet<String>,
    latest_usage_response_id: Option<String>,
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

/// Tracks how far a forked session's leading events still match the history it
/// replayed from its parent.
enum CodexReplayState<'a> {
    /// Comparing the child's leading usage against the parent's usage prefix.
    MatchingParent {
        prefix: &'a [CodexRawUsage],
        index: usize,
    },
    /// The parent stream could not anchor the replay, so skip the leading burst
    /// that Codex rewrote to the fork instant. Carries the last skipped event so
    /// the run can be followed however long it takes to write.
    SkippingRewrittenBurst(TimestampMs),
    /// Past the replayed history: every remaining event is the child's own usage.
    Done,
}

/// Longest pause tolerated inside a burst of replayed usage.
///
/// Codex rewrites a replayed history to the fork instant and writes it in one
/// go, so the burst is dense while the child's own first turn follows a real
/// pause. Across the fork logs this was measured against, bursts spanned 10 to
/// 40ms and the pause that followed ran from 5.8 to 15.3 seconds, so a second
/// sits two orders of magnitude above the one and well below the other.
///
/// Bucketing by the recorded second instead would split any burst written across
/// a second tick, leaving the remainder counted as the child's own usage.
const CODEX_REWRITTEN_BURST_PAUSE_MS: i64 = 1_000;

/// Start of the burst of replayed usage at the head of `path`, if it has one.
///
/// A session that opens with two usage events written back to back replayed a
/// history it did not spend; one that pauses between them was recording its own
/// turns from the start.
fn detect_rewritten_burst(path: &Path) -> Option<TimestampMs> {
    let Ok(file) = fs::File::open(path) else {
        return None;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut first: Option<TimestampMs> = None;

    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            return None;
        };
        if bytes_read == 0 {
            return None;
        }
        let Some(CodexLineKind::Session) = codex_line_usage_kind(&line) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<CodexSessionLogEntry<'_>>(&line) else {
            continue;
        };
        let info = value.payload.as_ref().and_then(|payload| {
            (value.entry_type.as_deref() == Some("event_msg")
                && payload.payload_type.as_deref() == Some("token_count"))
            .then_some(payload.info.as_ref())
            .flatten()
        });
        if info
            .is_none_or(|info| info.last_token_usage.is_none() && info.total_token_usage.is_none())
        {
            continue;
        }
        let Some(timestamp) = codex_session_timestamp(value.timestamp.as_ref())
            .as_deref()
            .and_then(parse_ts_timestamp)
        else {
            continue;
        };
        match first {
            None => first = Some(timestamp),
            Some(first) => {
                return (0..=CODEX_REWRITTEN_BURST_PAUSE_MS)
                    .contains(&(timestamp.as_millis() - first.as_millis()))
                    .then_some(first);
            }
        }
    }
}

/// Visits every usage event in a Codex session log.
///
/// `replayed_prefix` carries the usage a forked session copied from its parent so
/// it is not counted twice: `None` for sessions that are not forks, and an empty
/// slice for forks whose parent log is unavailable.
pub(super) fn visit_codex_session_file(
    sessions_dir: &Path,
    path: &Path,
    replayed_prefix: Option<&[CodexRawUsage]>,
    visit: impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    visit_codex_session_file_with_compaction_history(
        sessions_dir,
        path,
        replayed_prefix,
        None,
        visit,
    )
}

/// Visits usage while optionally retaining matched compaction identities,
/// including requests already covered by cumulative usage in a parent log.
pub(super) fn visit_codex_session_file_with_compaction_history(
    sessions_dir: &Path,
    path: &Path,
    replayed_prefix: Option<&[CodexRawUsage]>,
    compaction_history: Option<&mut HashMap<String, Option<TimestampMs>>>,
    mut visit: impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let Ok(file) = fs::File::open(path) else {
        return Ok(());
    };
    let mut reader = BufReader::with_capacity(128 * 1024, file);
    let mut line = Vec::new();
    let session_id = codex_session_id(sessions_dir, path);
    let mut state = CodexSessionUsageState::default();
    if compaction_history.is_some() {
        state.compaction_usage.recorded_usage_timestamps = Some(HashMap::new());
    }
    let fallback_timestamp = file_modified_timestamp(path);
    let mut replay = match replayed_prefix {
        Some(prefix) => CodexReplayState::MatchingParent { prefix, index: 0 },
        None => CodexReplayState::Done,
    };
    let mut visit_filtered = |event: CodexTokenUsageEvent| {
        // Compaction requests have their own response identity. Comparing them
        // with normal request totals could consume an unrelated replay entry;
        // copied compactions are deduplicated by ID after parsing instead.
        if event.response_id.is_some() {
            return visit(event);
        }
        // Each arm either returns or advances the state toward `Done`, so this
        // loop only re-runs to apply the event to the state it switched to.
        loop {
            match replay {
                CodexReplayState::MatchingParent { prefix, index } => {
                    let usage = event.raw_usage();
                    if prefix.get(index) == Some(&usage) {
                        replay = CodexReplayState::MatchingParent {
                            prefix,
                            index: index + 1,
                        };
                        return Ok(());
                    }
                    // Nothing matched, so the parent stream cannot anchor this
                    // replay: the log is unavailable, or Codex rewrote the copied
                    // history. Fall back to the rewritten burst instead.
                    replay = (index == 0)
                        .then(|| detect_rewritten_burst(path))
                        .flatten()
                        .map_or(
                            CodexReplayState::Done,
                            CodexReplayState::SkippingRewrittenBurst,
                        );
                }
                CodexReplayState::SkippingRewrittenBurst(previous) => {
                    if let Some(timestamp) = parse_ts_timestamp(&event.timestamp)
                        && (0..=CODEX_REWRITTEN_BURST_PAUSE_MS)
                            .contains(&(timestamp.as_millis() - previous.as_millis()))
                    {
                        replay = CodexReplayState::SkippingRewrittenBurst(timestamp);
                        return Ok(());
                    }
                    replay = CodexReplayState::Done;
                }
                CodexReplayState::Done => return visit(event),
            }
        }
    };

    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            break;
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
                visit_codex_session_entry(&session_id, value, &mut state, &mut visit_filtered)?;
            }
            CodexLineKind::Headless => {
                if let Ok(value) = serde_json::from_slice::<CodexLogEntry<'_>>(&line) {
                    add_codex_exec_event(
                        &session_id,
                        &value,
                        &fallback_timestamp,
                        &mut state.current_model,
                        &mut state.current_model_is_fallback,
                        &mut visit_filtered,
                    )?;
                } else {
                    add_codex_exec_event_from_value(
                        &session_id,
                        &line,
                        &fallback_timestamp,
                        &mut state.current_model,
                        &mut state.current_model_is_fallback,
                        &mut visit_filtered,
                    )?;
                };
            }
        }
    }

    if let Some(history) = compaction_history
        && let Some(mut timestamps) = state.compaction_usage.recorded_usage_timestamps
    {
        // Parent identity survives accounting suppression. A copied child can
        // retain a compaction pair while its cumulative snapshot is missing.
        timestamps.retain(|response_id, _| {
            state
                .compaction_usage
                .compacted_response_ids
                .contains(response_id)
        });
        history.extend(timestamps);
    }

    Ok(())
}

fn visit_codex_session_entry(
    session_id: &str,
    value: CodexSessionLogEntry<'_>,
    state: &mut CodexSessionUsageState,
    visit: &mut impl FnMut(CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
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
            return Ok(());
        };
        let response_id = response_id.to_string();
        compaction_usage
            .compacted_response_ids
            .insert(response_id.clone());
        if let Some(pending) = compaction_usage.pending_usage.remove(&response_id)
            && compaction_usage.emitted_response_ids.insert(response_id)
        {
            visit(pending.event)?;
        }
        return Ok(());
    }
    if entry_type == Some("token_usage_record") {
        let Some(payload) = value.payload.as_ref() else {
            return Ok(());
        };
        let Some(response_id) = payload
            .response_id
            .as_deref()
            .map(str::trim)
            .filter(|response_id| !response_id.is_empty())
        else {
            return Ok(());
        };
        let response_id = response_id.to_string();
        if compaction_usage.emitted_response_ids.contains(&response_id) {
            return Ok(());
        }
        let Some(timestamp) = codex_session_timestamp(value.timestamp.as_ref()) else {
            return Ok(());
        };
        let Some(raw_usage) = payload.usage.map(normalize_codex_raw_usage) else {
            return Ok(());
        };
        if raw_usage.input_tokens == 0
            && raw_usage.cached_input_tokens == 0
            && raw_usage.cache_creation_tokens == 0
            && raw_usage.output_tokens == 0
            && raw_usage.reasoning_output_tokens == 0
            && raw_usage.total_tokens == 0
        {
            return Ok(());
        }

        if let Some(timestamps) = compaction_usage.recorded_usage_timestamps.as_mut() {
            timestamps
                .entry(response_id.clone())
                .or_insert_with(|| parse_ts_timestamp(&timestamp));
        }

        let parsed_model = codex_model_from_payload(payload);
        let model_was_inferred = parsed_model.is_none();
        // A request record can name a different model, or never match a
        // compaction. Neither case changes the surrounding turn's model.
        let mut request_model = current_model.clone();
        let mut request_model_is_fallback = *current_model_is_fallback;
        let (model, is_fallback_model) = resolve_codex_usage_model(
            parsed_model,
            &timestamp,
            &mut request_model,
            &mut request_model_is_fallback,
        );
        let event = CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            response_id: Some(response_id.clone()),
            timestamp,
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

        if compaction_usage
            .compacted_response_ids
            .contains(&response_id)
        {
            if compaction_usage.emitted_response_ids.insert(response_id) {
                visit(event)?;
            }
        } else {
            compaction_usage
                .pending_usage
                .entry(response_id)
                .or_insert(CodexPendingUsage {
                    event,
                    thread_token_usage: payload.thread_token_usage.map(normalize_codex_raw_usage),
                });
        }
        return Ok(());
    }
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
    if payload.payload_type.as_deref() == Some("thread_settings_applied") {
        // A settings event that carries no `service_tier` at all says nothing
        // about the tier, so the previous one stands. Codex emits such events
        // for auto-review threads. A tier that is present but unrecognized is
        // different: it means the tier changed to something unknown, so the
        // stale value must not be inherited.
        if let Some(recorded) = payload
            .thread_settings
            .as_ref()
            .and_then(|settings| settings.service_tier.as_deref())
        {
            *current_service_tier = codex_service_tier(recorded);
        }
        return Ok(());
    }
    if payload.payload_type.as_deref() != Some("token_count") {
        return Ok(());
    }
    let info = payload.info.as_ref();
    let total_usage = info.and_then(|info| info.total_token_usage.as_ref().copied());
    let cumulative_advanced = total_usage
        .as_ref()
        .is_none_or(|total_usage| previous_totals.as_ref() != Some(total_usage));
    let raw_usage = info
        .and_then(|info| info.last_token_usage.as_ref().copied())
        .filter(|_| cumulative_advanced)
        .or_else(|| {
            total_usage
                .as_ref()
                .map(|usage| subtract_codex_raw_usage(usage, previous_totals.as_ref()))
        });
    if let Some(total_usage) = total_usage {
        *previous_totals = Some(total_usage);
    }
    let Some(raw_usage) = raw_usage.map(normalize_codex_raw_usage) else {
        return Ok(());
    };
    if raw_usage.input_tokens == 0
        && raw_usage.cached_input_tokens == 0
        && raw_usage.cache_creation_tokens == 0
        && raw_usage.output_tokens == 0
        && raw_usage.reasoning_output_tokens == 0
    {
        return Ok(());
    }

    // Local compaction emits an advancing token_count between the response
    // record and its marker. Remote v2 compaction omits that advance, so only
    // the latter needs an extra event. Matching the latest response keeps an
    // unrelated request with identical token counts from consuming it.
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
        compaction_usage.pending_usage.remove(response_id);
        compaction_usage
            .emitted_response_ids
            .insert(response_id.clone());
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
        response_id: None,
        timestamp,
        model,
        input_tokens: raw_usage.input_tokens,
        cached_input_tokens: raw_usage.cached_input_tokens,
        cache_creation_tokens: raw_usage.cache_creation_tokens,
        output_tokens: raw_usage.output_tokens,
        reasoning_output_tokens: raw_usage.reasoning_output_tokens,
        total_tokens: raw_usage.total_tokens,
        is_fallback_model,
        service_tier: *current_service_tier,
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
    let raw_usage = normalize_codex_raw_usage(raw_usage);
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        &timestamps.model,
        current_model,
        current_model_is_fallback,
    );
    visit(CodexTokenUsageEvent {
        session_id: session_id.to_string(),
        response_id: None,
        timestamp: timestamps.event,
        model,
        input_tokens: raw_usage.input_tokens,
        cached_input_tokens: raw_usage.cached_input_tokens,
        cache_creation_tokens: raw_usage.cache_creation_tokens,
        output_tokens: raw_usage.output_tokens,
        reasoning_output_tokens: raw_usage.reasoning_output_tokens,
        total_tokens: raw_usage.total_tokens,
        is_fallback_model,
        service_tier: None,
    })
}

fn codex_service_tier(value: &str) -> Option<CodexServiceTier> {
    match value {
        // Both spellings mean non-priority pricing and occur in the same Codex
        // version on the same day; which one is written depends on the client
        // (Codex Desktop writes "standard"), not on the CLI version.
        "default" | "standard" => Some(CodexServiceTier::Standard),
        "fast" | "priority" => Some(CodexServiceTier::Fast),
        _ => None,
    }
}

fn codex_line_usage_kind(line: &[u8]) -> Option<CodexLineKind> {
    let has_event_msg = EVENT_MSG_TYPE_FINDER.find(line).is_some();
    let has_token_count = has_event_msg && TOKEN_COUNT_TYPE_FINDER.find(line).is_some();
    let has_thread_settings_applied =
        has_event_msg && THREAD_SETTINGS_APPLIED_TYPE_FINDER.find(line).is_some();
    if TURN_CONTEXT_TYPE_FINDER.find(line).is_some()
        || has_token_count
        || has_thread_settings_applied
        || TOKEN_USAGE_RECORD_TYPE_FINDER.find(line).is_some()
        || COMPACTED_TYPE_FINDER.find(line).is_some()
    {
        return Some(CodexLineKind::Session);
    }
    let has_compact_type = COMPACT_TYPE_FIELD_FINDER.find(line).is_some();
    let has_nested_token_count = !has_event_msg
        && has_compact_type
        && line.len() < 64 * 1024
        && memchr::memmem::find(line, b"token_count").is_some();
    let has_nested_thread_settings_applied = !has_event_msg
        && has_compact_type
        && line.len() < 64 * 1024
        && THREAD_SETTINGS_APPLIED_FINDER.find(line).is_some();
    let has_compaction_type = memchr::memmem::find(line, b"token_usage_record").is_some()
        || memchr::memmem::find(line, b"compacted").is_some();
    if has_event_msg
        || memchr::memmem::find(line, b"turn_context").is_some()
        || has_nested_token_count
        || has_nested_thread_settings_applied
        || has_compaction_type
        || !has_compact_type
    {
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
    }
    if USAGE_FIELD_FINDER.find(line).is_some()
        || INPUT_TOKENS_FIELD_FINDER.find(line).is_some()
        || PROMPT_TOKENS_FIELD_FINDER.find(line).is_some()
    {
        return Some(CodexLineKind::Headless);
    }
    None
}

fn codex_line_type_flags(line: &[u8]) -> (bool, bool, bool, bool, bool, bool) {
    let mut start = 0;
    let mut has_turn_context = false;
    let mut has_event_msg = false;
    let mut has_token_count = false;
    let mut has_thread_settings_applied = false;
    let mut has_token_usage_record = false;
    let mut has_compacted = false;
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

/// Reads a Codex timestamp field that can hold an RFC3339 string or an epoch
/// number, using the same normalization as session events.
pub(super) fn codex_value_timestamp(value: Option<&Value>) -> Option<TimestampMs> {
    normalize_value_timestamp(value)
        .as_deref()
        .and_then(crate::parse_ts_timestamp)
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
    let usage = normalize_codex_raw_usage(usage_from_result(value)?);
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.cache_creation_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_output_tokens == 0
        && usage.total_tokens == 0
    {
        return None;
    }
    Some(usage)
}

fn normalize_headless_codex_usage_value(value: &Value) -> Option<CodexRawUsage> {
    let usage = normalize_codex_raw_usage(usage_from_result_value(value)?);
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.cache_creation_tokens == 0
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
    normalize_codex_raw_usage(CodexRawUsage {
        input_tokens: current
            .input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.input_tokens)),
        cached_input_tokens: current
            .cached_input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cached_input_tokens)),
        cache_creation_tokens: current
            .cache_creation_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cache_creation_tokens)),
        output_tokens: current
            .output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.output_tokens)),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.reasoning_output_tokens)),
        total_tokens: current
            .total_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.total_tokens)),
    })
}

fn normalize_codex_raw_usage(mut usage: CodexRawUsage) -> CodexRawUsage {
    usage.cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    usage.cache_creation_tokens = usage
        .cache_creation_tokens
        .min(usage.input_tokens.saturating_sub(usage.cached_input_tokens));
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    #[test]
    fn maps_recorded_service_tier_spellings() {
        for (value, expected) in [
            ("standard", Some(CodexServiceTier::Standard)),
            ("default", Some(CodexServiceTier::Standard)),
            ("priority", Some(CodexServiceTier::Fast)),
            ("fast", Some(CodexServiceTier::Fast)),
            ("flex", None),
            ("", None),
        ] {
            assert_eq!(
                codex_service_tier(value),
                expected,
                "unexpected tier for {value:?}"
            );
        }
    }

    #[test]
    fn recognizes_thread_settings_applied_as_session_line() {
        for line in [
            br#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#.as_slice(),
            br#"{ "type": "event_msg", "payload": { "type": "thread_settings_applied", "thread_settings": { "service_tier": "default" } } }"#.as_slice(),
        ] {
            assert!(matches!(
                codex_line_usage_kind(line),
                Some(CodexLineKind::Session)
            ));
        }
    }

    #[test]
    fn recognizes_compaction_usage_lines_with_json_whitespace() {
        for line in [
            br#"{"type":"token_usage_record","payload":{}}"#.as_slice(),
            br#"{ "type" : "token_usage_record", "payload" : {} }"#.as_slice(),
            br#"{ "type" : "compacted", "payload" : {} }"#.as_slice(),
        ] {
            assert!(matches!(
                codex_line_usage_kind(line),
                Some(CodexLineKind::Session)
            ));
        }
    }

    #[test]
    fn loads_codex_auto_review_fallbacks_from_models_dev_snapshot() {
        let fallbacks = codex_auto_review_fallback_models();

        assert_eq!(fallbacks.len(), 7);
        assert_eq!(fallbacks[0].released_on, "2026-07-30");
        assert_eq!(fallbacks[0].model, "gpt-5.6-luna");
        assert!(!fallbacks.iter().any(|fallback| fallback.model == "gpt-5.5"));
        assert_eq!(fallbacks[6].released_on, "2025-08-07");
        assert_eq!(fallbacks[6].model, "gpt-5");
        assert!(
            fallbacks
                .windows(2)
                .all(|window| window[0].released_on > window[1].released_on)
        );
    }

    #[test]
    fn normalizes_cache_usage_after_cumulative_resets_and_series_changes() {
        let reset = subtract_codex_raw_usage(
            &CodexRawUsage {
                input_tokens: 10,
                cached_input_tokens: 10,
                cache_creation_tokens: 0,
                ..CodexRawUsage::default()
            },
            Some(&CodexRawUsage {
                input_tokens: 100,
                cached_input_tokens: 0,
                cache_creation_tokens: 100,
                ..CodexRawUsage::default()
            }),
        );
        let series_change = subtract_codex_raw_usage(
            &CodexRawUsage {
                input_tokens: 110,
                cached_input_tokens: 0,
                cache_creation_tokens: 110,
                ..CodexRawUsage::default()
            },
            Some(&CodexRawUsage {
                input_tokens: 100,
                cached_input_tokens: 100,
                cache_creation_tokens: 0,
                ..CodexRawUsage::default()
            }),
        );

        assert_eq!(reset.input_tokens, 0);
        assert_eq!(reset.cached_input_tokens, 0);
        assert_eq!(reset.cache_creation_tokens, 0);
        assert_eq!(series_change.input_tokens, 10);
        assert_eq!(series_change.cached_input_tokens, 0);
        assert_eq!(series_change.cache_creation_tokens, 10);
    }

    #[test]
    fn counts_only_usage_records_linked_to_compaction_events() {
        let token_count = |timestamp: &str| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 80,
                            "output_tokens": 20,
                            "reasoning_output_tokens": 0,
                            "total_tokens": 120,
                        },
                    },
                },
            })
            .to_string()
        };
        let token_usage_record = |timestamp: &str, response_id: &str, input_tokens: u64| {
            json!({
                "timestamp": timestamp,
                "type": "token_usage_record",
                "payload": {
                    "response_id": response_id,
                    "usage": {
                        "input_tokens": input_tokens,
                        "cached_input_tokens": input_tokens - 20,
                        "output_tokens": 20,
                        "reasoning_output_tokens": 0,
                        "total_tokens": input_tokens + 20,
                    },
                },
            })
            .to_string()
        };
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({
                    "timestamp": "2026-09-01T00:00:00.000Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-reserve" },
                })
                .to_string(),
                token_count("2026-09-01T00:00:01.000Z"),
                token_usage_record("2026-09-01T00:00:02.000Z", "compact-before", 300),
                json!({
                    "timestamp": "2026-09-01T00:00:03.000Z",
                    "type": "compacted",
                    "payload": { "compaction_response_id": "compact-before" },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-09-01T00:00:04.000Z",
                    "type": "compacted",
                    "payload": { "compaction_response_id": "compact-before" },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-09-01T00:00:05.000Z",
                    "type": "compacted",
                    "payload": { "compaction_response_id": "compact-after" },
                })
                .to_string(),
                token_usage_record("2026-09-01T00:00:06.000Z", "compact-after", 200),
                token_usage_record("2026-09-01T00:00:07.000Z", "not-a-compaction", 500),
                token_count("2026-09-01T00:00:08.000Z"),
            ]
            .join("\n"),
        });

        let events = crate::load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 3);
        let compacted_usages = events
            .iter()
            .filter(|event| event.input_tokens > 100)
            .collect::<Vec<_>>();
        assert_eq!(compacted_usages.len(), 2);
        assert_eq!(compacted_usages[0].input_tokens, 300);
        assert_eq!(compacted_usages[0].cached_input_tokens, 280);
        assert_eq!(compacted_usages[0].output_tokens, 20);
        assert_eq!(compacted_usages[0].total_tokens, 320);
        assert_eq!(compacted_usages[0].model.as_deref(), Some("gpt-reserve"));
        assert!(compacted_usages[0].is_fallback_model);
        assert_eq!(compacted_usages[1].input_tokens, 200);
    }

    #[test]
    fn usage_record_models_do_not_replace_the_active_turn_model() {
        for marker in [
            r#"{"type":"compacted","payload":{"compaction_response_id":"response-1"}}"#,
            r#"{"type":"compacted","payload":{"compaction_response_id":"different-response"}}"#,
        ] {
            let fixture = fs_fixture!({
                "session.jsonl": [
                    r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
                    r#"{"timestamp":"2026-09-01T00:00:01Z","type":"token_usage_record","payload":{"response_id":"response-1","model":"gpt-5-mini","usage":{"input_tokens":300,"output_tokens":30}}}"#,
                    marker,
                    r#"{"timestamp":"2026-09-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
                ].join("\n"),
            });
            let events = crate::load_codex_events_from_directory(fixture.root(), true).unwrap();
            assert_eq!(events.last().unwrap().model.as_deref(), Some("gpt-5"));
            assert!(!events.last().unwrap().is_fallback_model);
        }
    }

    #[test]
    fn recognizes_spaced_compaction_types_with_compact_nested_types() {
        for line in [
            br#"{"type" : "token_usage_record","payload":{"type":"request"}}"#.as_slice(),
            br#"{"type" : "compacted","payload":{"type":"summary"}}"#.as_slice(),
        ] {
            assert!(matches!(
                codex_line_usage_kind(line),
                Some(CodexLineKind::Session)
            ));
        }
    }

    #[test]
    fn loads_token_counts_with_spaces_after_type_colons() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{"type": "turn_context", "payload": {"model": "gpt-5-mini"}}"#,
                r#"{"timestamp": "2026-09-01T00:00:01Z", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 20}}}}"#,
            ].join("\n"),
        });
        let events = crate::load_codex_events_from_directory(fixture.root(), true).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 120);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5-mini"));
    }
    #[test]
    fn counts_compaction_usage_without_changing_cumulative_baseline() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.jsonl": [
                r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
                r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
                r#"{"timestamp":"2026-09-01T00:00:02Z","type":"token_usage_record","payload":{"response_id":"compaction-1","usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":30}}}"#,
                r#"{"timestamp":"2026-09-01T00:00:03Z","type":"compacted","payload":{"compaction_response_id":"compaction-1"}}"#,
                r#"{"timestamp":"2026-09-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"output_tokens":30}}}}"#,
            ].join("\n"),
        });
        let events = crate::load_codex_events_from_directory(fixture.root(), true).unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event.total_tokens)
                .collect::<Vec<_>>(),
            [120, 330, 60]
        );
        assert_eq!(events[1].cached_input_tokens, 200);
        assert_eq!(events[1].model.as_deref(), Some("gpt-5"));
        assert!(events[1].is_fallback_model);
    }
}
