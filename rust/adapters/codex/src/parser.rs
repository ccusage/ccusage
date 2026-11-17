use ccusage_adapter_common::{UnitError, jsonl};
use ccusage_core::{Diagnostics, Skip, SkipKind, fast::LinePrefilter};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::Path,
    sync::LazyLock,
};

use memchr::memmem::Finder;
use serde_json::Value;

use crate::{
    CodexRawUsage, CodexServiceTier, CodexTokenUsageEvent, TimestampMs, parse_ts_timestamp,
};

use super::{
    reader::{READ_BUFFER_SIZE, open_rollout_lines, uncompressed_rollout_path},
    replay::{CodexSessionMetadata, session_metadata_from_line},
    types::{
        CodexInfo, CodexLogEntry, CodexModelMetadata, CodexPayload, CodexResultFields,
        CodexTimestamp, CodexUsageCandidate,
    },
};

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
const CODEX_AUTO_REVIEW_MODEL: &str = "codex-auto-review";
const CODEX_AUTO_REVIEW_DEFAULT_MODEL: &str = "gpt-5";
// NOTE(2026-09-17, #1753): auto-review routing changes independently of public model releases, so a new entry needs evidence of the routing change, not just a catalog release.
const CODEX_AUTO_REVIEW_FALLBACKS: &[CodexAutoReviewFallback] = &[
    CodexAutoReviewFallback::new((2026, 7, 30), "gpt-5.6-luna"),
    CodexAutoReviewFallback::new((2026, 3, 5), "gpt-5.4"),
    CodexAutoReviewFallback::new((2026, 2, 5), "gpt-5.3-codex"),
    CodexAutoReviewFallback::new((2025, 12, 11), "gpt-5.2-codex"),
    CodexAutoReviewFallback::new((2025, 11, 13), "gpt-5.1-codex"),
    CodexAutoReviewFallback::new((2025, 9, 15), "gpt-5-codex"),
    CodexAutoReviewFallback::new((2025, 8, 7), CODEX_AUTO_REVIEW_DEFAULT_MODEL),
];

struct CodexAutoReviewFallback {
    released_on: (i32, u32, u32),
    model: &'static str,
}

impl CodexAutoReviewFallback {
    const fn new(released_on: (i32, u32, u32), model: &'static str) -> Self {
        Self { released_on, model }
    }
}

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

/// History a forked session replayed from its parent.
#[derive(Clone, Copy)]
pub(super) struct CodexForkReplay<'a> {
    /// The parent's usage up to the fork; empty when the parent log is unavailable.
    pub(super) prefix: &'a [CodexRawUsage],
    /// Codex drops the parent's `token_usage_record` lines when it spawns a
    /// subagent, so every record in such a child is a request the child sent
    /// itself and nothing after it can be replayed history. A user fork copies
    /// them with the rest of the parent's history, so they prove nothing there.
    pub(super) ends_at_usage_record: bool,
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

#[derive(Default)]
struct CodexBurst {
    session_start: Option<TimestampMs>,
    first: Option<TimestampMs>,
    decision: Option<Option<TimestampMs>>,
}

impl CodexBurst {
    // NOTE(2026-07-27, #1501): a Codex session that opens with two usage events written back to back replayed a history it did not spend; one that pauses between them was recording its own turns from the start.
    fn observe(&mut self, timestamp: TimestampMs) {
        if self.decision.is_some() {
            return;
        }
        let Some(first) = self.first else {
            self.first = Some(timestamp);
            return;
        };
        let is_dense = (0..=CODEX_REWRITTEN_BURST_PAUSE_MS)
            .contains(&(timestamp.as_millis() - first.as_millis()));
        // NOTE(2026-10-26, #1971): a late first request can also produce a dense pair, so the pair must also start at the fork instant.
        let starts_at_fork = self.session_start.is_none_or(|start| {
            first.as_millis().abs_diff(start.as_millis())
                <= CODEX_REWRITTEN_BURST_PAUSE_MS.unsigned_abs()
        });
        self.decision = Some((is_dense && starts_at_fork).then_some(first));
    }
}

enum CodexPendingReplay {
    Event(CodexTokenUsageEvent),
    UsageRecord,
}

struct CodexReplayFilter<'a> {
    state: CodexReplayState<'a>,
    ends_at_usage_record: bool,
    burst: CodexBurst,
    pending: Vec<CodexPendingReplay>,
}

impl CodexReplayFilter<'_> {
    fn observe_usage_record(&mut self) {
        if self.ends_at_usage_record {
            if self.burst.decision.is_none() && !self.pending.is_empty() {
                self.pending.push(CodexPendingReplay::UsageRecord);
            } else {
                self.state = CodexReplayState::Done;
            }
        }
    }

    fn observe_token_count(
        &mut self,
        timestamp: Option<TimestampMs>,
        usage: Option<CodexRawUsage>,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) {
        if matches!(self.state, CodexReplayState::Done) && self.pending.is_empty() {
            return;
        }
        if usage.is_some_and(|usage| !normalize_codex_raw_usage(usage).is_empty())
            && let Some(timestamp) = timestamp
        {
            self.burst.observe(timestamp);
            if self.burst.decision.is_some() {
                self.flush(visit);
            }
        }
    }

    fn flush(&mut self, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        for pending in std::mem::take(&mut self.pending) {
            match pending {
                CodexPendingReplay::Event(event) => self.visit(event, visit),
                CodexPendingReplay::UsageRecord => self.observe_usage_record(),
            }
        }
    }

    fn finish(&mut self, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        self.burst.decision.get_or_insert(None);
        self.flush(visit);
    }

    fn visit(&mut self, event: CodexTokenUsageEvent, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        if self.burst.decision.is_none() && !self.pending.is_empty() {
            self.pending.push(CodexPendingReplay::Event(event));
            return;
        }
        // Compaction requests have their own response identity. Comparing them
        // with normal request totals could consume an unrelated replay entry;
        // copied compactions are deduplicated by ID after parsing instead.
        if event.response_id.is_some() {
            visit(event);
            return;
        }
        loop {
            match self.state {
                CodexReplayState::MatchingParent { prefix, index } => {
                    if prefix.get(index) == Some(&event.raw_usage()) {
                        self.state = CodexReplayState::MatchingParent {
                            prefix,
                            index: index + 1,
                        };
                        return;
                    }
                    // NOTE(2026-07-25, #1435): nothing matched, so the parent stream cannot anchor this replay: the parent log is unavailable, or Codex rewrote the copied history.
                    if index == 0 {
                        let Some(burst) = self.burst.decision else {
                            self.pending.push(CodexPendingReplay::Event(event));
                            return;
                        };
                        self.state = burst.map_or(
                            CodexReplayState::Done,
                            CodexReplayState::SkippingRewrittenBurst,
                        );
                    } else {
                        self.state = CodexReplayState::Done;
                    }
                }
                CodexReplayState::SkippingRewrittenBurst(previous) => {
                    let timestamp = event.timestamp;
                    if (0..=CODEX_REWRITTEN_BURST_PAUSE_MS)
                        .contains(&(timestamp.as_millis() - previous.as_millis()))
                    {
                        self.state = CodexReplayState::SkippingRewrittenBurst(timestamp);
                        return;
                    }
                    self.state = CodexReplayState::Done;
                }
                CodexReplayState::Done => {
                    visit(event);
                    return;
                }
            }
        }
    }
}

/// Visits usage while optionally retaining matched compaction identities,
/// including requests already covered by cumulative usage in a parent log.
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
    )
}

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
    )
}

/// Visits a rollout that its first line shows is not a fork, and returns that
/// line's session metadata. A fork is left unvisited: which of its usage it
/// replayed is known only once every rollout's metadata has been read.
pub(super) fn visit_codex_session_file_unless_fork(
    sessions_dir: &Path,
    path: &Path,
    visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
) -> CodexSessionMetadata {
    let mut metadata = CodexSessionMetadata::default();
    visit_codex_session_file_inner(
        sessions_dir,
        path,
        None,
        None,
        visit,
        diagnostics,
        Some(&mut metadata),
    );
    metadata
}

fn visit_codex_session_file_inner(
    sessions_dir: &Path,
    path: &Path,
    fork_replay: Option<CodexForkReplay<'_>>,
    compaction_history: Option<&mut HashMap<String, Option<TimestampMs>>>,
    mut visit: impl FnMut(CodexTokenUsageEvent),
    diagnostics: &Diagnostics,
    mut stop_at_fork: Option<&mut CodexSessionMetadata>,
) {
    let (mut lines, opened_path) = match open_rollout_lines(path, READ_BUFFER_SIZE) {
        Ok(opened) => opened,
        Err(failed_path) => {
            UnitError::new(SkipKind::Unreadable, "rollout open failed")
                .record(&failed_path, diagnostics);
            return;
        }
    };
    let session_id = codex_session_id(sessions_dir, path);
    let mut state = CodexSessionUsageState::default();
    if compaction_history.is_some() {
        state.compaction_usage.recorded_usage_timestamps = Some(HashMap::new());
    }
    let fallback_timestamp = file_modified_timestamp(&opened_path);
    let mut replay = CodexReplayFilter {
        state: fork_replay.map_or(CodexReplayState::Done, |fork_replay| {
            CodexReplayState::MatchingParent {
                prefix: fork_replay.prefix,
                index: 0,
            }
        }),
        burst: CodexBurst::default(),
        pending: Vec::new(),
        ends_at_usage_record: fork_replay
            .is_some_and(|fork_replay| fork_replay.ends_at_usage_record),
    };

    let mut line_number = 0_u32;
    loop {
        let line = match lines.next_line() {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => {
                diagnostics.record(Skip::unit(
                    path,
                    SkipKind::ReadInterrupted,
                    "rollout read failed",
                ));
                break;
            }
        };
        line_number = line_number.saturating_add(1);
        let first_metadata = stop_at_fork.take().map(|metadata| {
            *metadata = session_metadata_from_line(line);
            metadata.clone()
        });
        if first_metadata
            .as_ref()
            .is_some_and(CodexSessionMetadata::is_fork)
        {
            return;
        }
        let has_metadata = first_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_session_meta);
        if has_metadata
            || (replay.burst.session_start.is_none()
                && !matches!(replay.state, CodexReplayState::Done)
                && SESSION_META_MARKER.matches(line))
        {
            let metadata = first_metadata.unwrap_or_else(|| session_metadata_from_line(line));
            if metadata.is_session_meta {
                if replay.burst.session_start.is_none() {
                    replay.burst.session_start = metadata.timestamp;
                }
                continue;
            }
        }
        let Some(line_kind) = codex_line_usage_kind(line) else {
            continue;
        };
        let value = match serde_json::from_slice::<CodexLogEntry<'_>>(line) {
            Ok(value) => value,
            Err(error) => {
                if line_kind == CodexLineKind::Escaped && verbatim_line_usage_kind(line).is_none() {
                    continue;
                }
                record_decode_failure(
                    path,
                    line_number,
                    line,
                    error,
                    diagnostics,
                    Some(&mut state.previous_totals),
                );
                continue;
            }
        };
        let location = RecordLocation {
            path,
            line: line_number,
            diagnostics,
        };
        if is_session_usage_type(
            value.entry_type.as_deref(),
            value
                .payload
                .as_ref()
                .and_then(|payload| payload.payload_type.as_deref()),
        ) {
            if value.entry_type.as_deref() == Some("token_usage_record") {
                replay.observe_usage_record();
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
                fallback_timestamp,
                &mut state.current_model,
                &mut state.current_model_is_fallback,
                &mut |event| replay.visit(event, &mut visit),
                &location,
            );
        }
    }

    replay.finish(&mut visit);

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
        // A request record can name a different model, or never match a
        // compaction. Neither case changes the surrounding turn's model.
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
    replay.observe_token_count(
        parsed_timestamp.as_ref().map(|(timestamp, _)| *timestamp),
        info.and_then(|info| {
            info.last_token_usage
                .decoded()
                .or(info.total_token_usage.decoded())
        }),
        visit,
    );
    let CodexTokenCountUsage {
        total_usage,
        cumulative_advanced,
        delta,
    } = codex_token_count_usage(info, previous_totals);
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
        timestamp,
        current_model,
        current_model_is_fallback,
    );

    replay.visit(
        CodexTokenUsageEvent {
            session_id: session_id.to_string(),
            response_id: None,
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

fn add_codex_exec_event(
    session_id: &str,
    value: &CodexLogEntry<'_>,
    fallback_timestamp: TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent),
    location: &RecordLocation<'_>,
) {
    let raw_usage = match normalize_headless_codex_usage(value) {
        Ok(Some(raw_usage)) => raw_usage,
        Ok(None) => return,
        Err(()) => {
            location.invalid_usage("headless usage did not decode");
            return;
        }
    };
    let parsed_model = codex_model_from_result(value);
    visit_codex_exec_usage_event(
        session_id,
        raw_usage,
        parsed_model,
        codex_timestamp_from_result(value).unwrap_or(fallback_timestamp),
        current_model,
        current_model_is_fallback,
        visit,
    )
}

fn visit_codex_exec_usage_event(
    session_id: &str,
    raw_usage: CodexRawUsage,
    parsed_model: Option<String>,
    timestamp: TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
    visit: &mut impl FnMut(CodexTokenUsageEvent),
) {
    let raw_usage = normalize_codex_raw_usage(raw_usage);
    let (model, is_fallback_model) = resolve_codex_usage_model(
        parsed_model,
        timestamp,
        current_model,
        current_model_is_fallback,
    );
    visit(CodexTokenUsageEvent {
        session_id: session_id.to_string(),
        response_id: None,
        timestamp,
        last_activity: crate::format_rfc3339_millis(timestamp),
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

fn codex_line_usage_kind(line: &[u8]) -> Option<CodexLineKind> {
    if ESCAPED_USAGE_MARKERS.matches_escaped(line) {
        Some(CodexLineKind::Escaped)
    } else {
        verbatim_line_usage_kind(line)
    }
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

fn codex_session_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    match value? {
        CodexTimestamp::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        CodexTimestamp::Number(_) => normalize_codex_timestamp(value),
        CodexTimestamp::Invalid => None,
    }
}

fn codex_usage_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<(TimestampMs, String)> {
    let instant = codex_timestamp_instant(value)?;
    let raw = match value? {
        CodexTimestamp::String(text) => text.trim().to_string(),
        CodexTimestamp::Number(_) => crate::format_rfc3339_millis(instant),
        CodexTimestamp::Invalid => return None,
    };
    Some((instant, raw))
}

fn resolve_codex_usage_model(
    parsed_model: Option<String>,
    timestamp: TimestampMs,
    current_model: &mut Option<String>,
    current_model_is_fallback: &mut bool,
) -> (String, bool) {
    if let Some(model) = parsed_model {
        *current_model = Some(model);
        *current_model_is_fallback = false;
    }
    let model = current_model.get_or_insert_with(|| {
        *current_model_is_fallback = true;
        "gpt-5".to_string()
    });
    match codex_log_model_fallback(model, timestamp) {
        Some(fallback) => (fallback.to_string(), true),
        None => (model.clone(), *current_model_is_fallback),
    }
}

fn codex_log_model_fallback(model: &str, timestamp: TimestampMs) -> Option<&'static str> {
    if model != CODEX_AUTO_REVIEW_MODEL {
        return None;
    }
    let parts = timestamp.utc_parts();
    let date = (parts.year, parts.month, parts.day);
    let routed = CODEX_AUTO_REVIEW_FALLBACKS
        .iter()
        .filter(|fallback| fallback.released_on <= date)
        .max_by_key(|fallback| fallback.released_on);
    Some(routed.map_or(CODEX_AUTO_REVIEW_DEFAULT_MODEL, |fallback| fallback.model))
}

fn codex_session_id(sessions_dir: &Path, path: &Path) -> String {
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

fn non_empty_cow_string(value: Option<&Cow<'_, str>>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn usage_from_result(value: &CodexLogEntry<'_>) -> CodexUsageCandidate {
    first_decoded_usage([
        value.usage,
        value
            .data
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |data| data.usage),
        value
            .result
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |result| result.usage),
        value
            .response
            .as_ref()
            .map_or(CodexUsageCandidate::Missing, |response| response.usage),
    ])
}

fn first_decoded_usage(
    candidates: impl IntoIterator<Item = CodexUsageCandidate>,
) -> CodexUsageCandidate {
    let mut malformed = false;
    for candidate in candidates {
        match candidate {
            decoded @ CodexUsageCandidate::Decoded(_) => return decoded,
            CodexUsageCandidate::Malformed => malformed = true,
            CodexUsageCandidate::Missing => {}
        }
    }
    if malformed {
        CodexUsageCandidate::Malformed
    } else {
        CodexUsageCandidate::Missing
    }
}

fn codex_timestamp_from_result(value: &CodexLogEntry<'_>) -> Option<TimestampMs> {
    codex_timestamp_instant(value.timestamp.as_ref())
        .or_else(|| codex_timestamp_instant(value.created_at.as_ref()))
        .or_else(|| codex_timestamp_instant(value.created_at_camel.as_ref()))
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

fn normalize_result_fields_timestamp(value: &CodexResultFields<'_>) -> Option<TimestampMs> {
    codex_timestamp_instant(value.timestamp.as_ref())
        .or_else(|| codex_timestamp_instant(value.created_at.as_ref()))
        .or_else(|| codex_timestamp_instant(value.created_at_camel.as_ref()))
}

fn normalize_codex_timestamp(value: Option<&CodexTimestamp<'_>>) -> Option<String> {
    codex_timestamp_instant(value).map(crate::format_rfc3339_millis)
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

fn normalize_headless_codex_usage(
    value: &CodexLogEntry<'_>,
) -> std::result::Result<Option<CodexRawUsage>, ()> {
    normalize_headless_usage_candidate(usage_from_result(value))
}

fn normalize_headless_usage_candidate(
    candidate: CodexUsageCandidate,
) -> std::result::Result<Option<CodexRawUsage>, ()> {
    let usage = match candidate {
        CodexUsageCandidate::Decoded(usage) => normalize_codex_raw_usage(usage),
        CodexUsageCandidate::Malformed => return Err(()),
        CodexUsageCandidate::Missing => return Ok(None),
    };
    if usage.is_empty() {
        return Ok(None);
    }
    Ok(Some(usage))
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    ccusage_adapter_common::timestamps::file_mtime(path).unwrap_or(TimestampMs::UNIX_EPOCH)
}

fn malformed_token_count_field(info: &CodexInfo<'_>) -> Option<&'static str> {
    if info.last_token_usage == CodexUsageCandidate::Malformed {
        Some("field `payload.info.last_token_usage` did not decode")
    } else if info.total_token_usage == CodexUsageCandidate::Malformed {
        Some("field `payload.info.total_token_usage` did not decode")
    } else {
        None
    }
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

fn codex_token_count_usage(
    info: Option<&CodexInfo<'_>>,
    previous_totals: &mut Option<CodexRawUsage>,
) -> CodexTokenCountUsage {
    let total_usage = info.and_then(|info| info.total_token_usage.decoded());
    let cumulative_advanced = total_usage
        .as_ref()
        .is_none_or(|total_usage| previous_totals.as_ref() != Some(total_usage));
    let raw_usage = info
        .and_then(|info| info.last_token_usage.decoded())
        .filter(|_| cumulative_advanced)
        .or_else(|| {
            total_usage
                .as_ref()
                .map(|usage| subtract_codex_raw_usage(usage, previous_totals.as_ref()))
        });
    if let Some(total_usage) = total_usage {
        *previous_totals = Some(total_usage);
    }
    CodexTokenCountUsage {
        total_usage,
        cumulative_advanced,
        delta: raw_usage.map(normalize_codex_raw_usage),
    }
}

fn normalize_codex_raw_usage(mut usage: CodexRawUsage) -> CodexRawUsage {
    usage.cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    usage.cache_creation_tokens = usage
        .cache_creation_tokens
        .min(usage.input_tokens.saturating_sub(usage.cached_input_tokens));
    usage
}

/// Classifies a rollout line that did not decode like every other JSONL
/// source does. A rejected `token_count` still advances the cumulative
/// baseline, so the next record's delta does not count its tokens again.
fn record_decode_failure(
    path: &Path,
    line: u32,
    content: &[u8],
    error: serde_json::Error,
    diagnostics: &Diagnostics,
    previous_totals: Option<&mut Option<CodexRawUsage>>,
) {
    jsonl::record_decode_error(
        content,
        error,
        path,
        line,
        !content.ends_with(b"\n"),
        diagnostics,
        Some(|value: &Value| {
            let token_count = value.get("type").and_then(Value::as_str) == Some("event_msg")
                && value.pointer("/payload/type").and_then(Value::as_str) == Some("token_count");
            if let Some(previous_totals) = previous_totals
                && token_count
                && let Some(total) = value.pointer("/payload/info/total_token_usage")
                && let Ok(total) = serde_json::from_value::<CodexRawUsage>(total.clone())
            {
                *previous_totals = Some(total);
            }
            value.get("type").and_then(Value::as_str) == Some("token_usage_record")
                || (token_count
                    && value
                        .pointer("/payload/info")
                        .is_some_and(|info| !info.is_null()))
        }),
    );
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
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;
    use serde_json::json;
    use std::assert_matches;

    #[test]
    fn usage_with_only_a_total_is_empty_in_every_record_kind() {
        let total_only = json!({"total_tokens": 50});
        for record in [
            json!({"timestamp": "2026-01-02T00:00:00Z", "type": "event_msg", "payload": {
                "type": "token_count", "info": {"model": "gpt-5", "last_token_usage": total_only},
            }}),
            json!({"timestamp": "2026-01-02T00:00:00Z", "type": "turn.completed", "usage": total_only}),
        ] {
            let fixture = fs_fixture!({ "session.jsonl": record.to_string() });
            assert!(
                crate::test_support::rollout_events(fixture.root()).is_empty(),
                "{record}"
            );
        }
        let fixture = fs_fixture!({
            "session.jsonl": [
                json!({"type": "turn_context", "payload": {"model": "gpt-5"}}),
                json!({"timestamp": "2026-01-02T00:00:00Z", "type": "token_usage_record", "payload": {
                    "response_id": "response-1", "usage": total_only,
                }}),
                json!({"type": "compacted", "payload": {"compaction_response_id": "response-1"}}),
            ]
            .map(|line| line.to_string())
            .join("\n"),
        });
        assert!(crate::test_support::rollout_events(fixture.root()).is_empty());
    }

    #[test]
    fn maps_recorded_service_tier_spellings() {
        for (value, expected) in [
            ("standard", Some(CodexServiceTier::Standard)),
            ("default", Some(CodexServiceTier::Standard)),
            ("priority", Some(CodexServiceTier::Fast)),
            ("fast", Some(CodexServiceTier::Fast)),
            ("flex", Some(CodexServiceTier::Flex)),
            ("ultrafast", Some(CodexServiceTier::Ultrafast)),
            ("", None),
        ] {
            assert_eq!(
                CodexServiceTier::from_codex_name(value),
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
            assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
        }
    }

    #[test]
    fn loads_large_spaced_token_counts_with_compact_payload_types() {
        let line = serde_json::json!({
            "timestamp": "2026-09-01T00:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "last_token_usage": { "input_tokens": 100, "output_tokens": 20 } },
                "padding": "x".repeat(70 * 1024),
            },
        })
        .to_string()
        .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#);
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{"type":"turn_context","payload":{"model":"gpt-5-mini"}}"#,
                line.as_str(),
            ].join("\n"),
        });

        let events = crate::test_support::rollout_events(fixture.root());

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 120);
    }

    #[test]
    fn loads_headless_usage_with_nested_session_type_names() {
        for (entry_type, padding) in [
            (Some("result"), 70 * 1024),
            (None, 70 * 1024),
            (Some("result"), 0),
        ] {
            let mut record = json!({
                "timestamp": "2026-09-01T00:00:01Z",
                "model": "gpt-5-mini",
                "usage": { "input_tokens": 100, "output_tokens": 20 },
                "data": {
                    "type": "event_msg",
                    "nested": { "type": "token_count" },
                    "padding": "x".repeat(padding),
                },
            });
            if let Some(entry_type) = entry_type {
                record["type"] = json!(entry_type);
            }
            let line = record
                .to_string()
                .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#)
                .replace(r#""type":"token_count""#, r#""type" : "token_count""#);
            let fixture = fs_fixture!({ "headless.jsonl": line });

            let events = crate::test_support::rollout_events(fixture.root());

            assert_eq!(events.len(), 1);
            assert_eq!(events[0].input_tokens, 100);
            assert_eq!(events[0].output_tokens, 20);
            assert_eq!(events[0].total_tokens, 120);
        }
    }

    #[test]
    fn loads_headless_created_at_after_session_candidate_decode_fails() {
        for timestamp in [json!(false), json!({"SECRET_CANARY": true})] {
            let record = json!({
                "type": "result",
                "timestamp": timestamp,
                "created_at": "2026-09-01T00:00:01Z",
                "model": "gpt-5-mini",
                "usage": { "input_tokens": 100, "output_tokens": 20 },
                "data": {
                    "type": "event_msg",
                    "nested": { "type": "token_count" },
                    "padding": "x".repeat(70 * 1024),
                },
            });
            let line = record
                .to_string()
                .replace(r#""type":"event_msg""#, r#""type" : "event_msg""#)
                .replace(r#""type":"token_count""#, r#""type" : "token_count""#);
            let fixture = fs_fixture!({ "headless.jsonl": line });

            let events = crate::test_support::rollout_events(fixture.root());

            assert_eq!(events.len(), 1);
            assert_eq!(events[0].last_activity, "2026-09-01T00:00:01.000Z");
            assert_eq!(events[0].input_tokens, 100);
            assert_eq!(events[0].output_tokens, 20);
            assert_eq!(events[0].total_tokens, 120);
        }
    }

    #[test]
    fn headless_decoder_recovers_malformed_optional_fields_and_usage() {
        for line in [
            r#"{"timestamp":false,"created_at":"2026-09-01T00:00:01Z","model":{},"model_name":"gpt-5-mini","usage":{"input_tokens":10,"output_tokens":2}}"#,
            r#"{"timestamp":[],"metadata":{"model":false},"data":{"model_name":"gpt-5-mini","createdAt":1788220801000,"usage":{"input_tokens":10,"output_tokens":2}}}"#,
            r#"{"usage":{"input_tokens":[]},"response":{"usage":{"input_tokens":10,"output_tokens":2}}}"#,
            r#"{"usage":{"input_tokens":{}},"result":{"usage":{"input_tokens":10,"output_tokens":2}}}"#,
            r#"{"payload":{"info":{"last_token_usage":{"input_tokens":{}}}},"usage":{"input_tokens":10,"output_tokens":2}}"#,
            r#"{"metadata":{"model":[]},"response":{"metadata":{"model":{}},"usage":{"input_tokens":10,"output_tokens":2}}}"#,
        ] {
            let typed: CodexLogEntry<'_> = serde_json::from_str(line).unwrap();
            let usage = normalize_headless_codex_usage(&typed).unwrap().unwrap();
            assert_eq!(usage.input_tokens, 10, "{line}");
            assert_eq!(usage.output_tokens, 2, "{line}");
            let fixture = fs_fixture!({ "headless.jsonl": line });
            let events = crate::test_support::rollout_events(fixture.root());
            assert_eq!(events.len(), 1, "{line}");
            assert_eq!(events[0].input_tokens, 10, "{line}");
            assert_eq!(events[0].output_tokens, 2, "{line}");
        }
    }

    #[test]
    fn headless_decoder_keeps_the_last_duplicate_field() {
        for line in [
            r#"{"model":"old","model":"gpt-5-mini","usage":{"input_tokens":999},"usage":{"input_tokens":10,"output_tokens":2}}"#,
            r#"{"data":{"model":"old","model":"gpt-5-mini","usage":{"input_tokens":999,"input_tokens":10,"output_tokens":2}}}"#,
            r#"{"data":{"usage":{"input_tokens":999}},"data":{"model":"gpt-5-mini","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        ] {
            let typed: CodexLogEntry<'_> = serde_json::from_str(line).unwrap();
            assert_eq!(
                codex_model_from_result(&typed).as_deref(),
                Some("gpt-5-mini")
            );
            assert_eq!(
                normalize_headless_codex_usage(&typed)
                    .unwrap()
                    .unwrap()
                    .input_tokens,
                10
            );
            let fixture = fs_fixture!({ "headless.jsonl": line });
            let events = crate::test_support::rollout_events(fixture.root());
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].model, "gpt-5-mini");
            assert_eq!(events[0].input_tokens, 10);
        }
    }

    #[test]
    fn session_usage_ignores_unrelated_headless_fields() {
        let fixture = fs_fixture!({
            "session.jsonl": r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","model":{},"created_at":false,"data":{"model":[]},"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
        });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 10);
        assert_eq!(events[0].model, "gpt-5");
    }

    #[test]
    fn recognizes_compaction_usage_lines_with_json_whitespace() {
        for line in [
            br#"{"type":"token_usage_record","payload":{}}"#.as_slice(),
            br#"{ "type" : "token_usage_record", "payload" : {} }"#.as_slice(),
            br#"{ "type" : "compacted", "payload" : {} }"#.as_slice(),
        ] {
            assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
        }
    }

    #[test]
    fn routes_codex_auto_review_through_models_the_embedded_pricing_knows() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = crate::PricingMap::load_embedded();

        for fallback in CODEX_AUTO_REVIEW_FALLBACKS {
            assert!(
                pricing.find_exact(fallback.model).is_some(),
                "{} has no embedded pricing",
                fallback.model
            );
        }
        assert!(
            !CODEX_AUTO_REVIEW_FALLBACKS
                .iter()
                .any(|fallback| fallback.model == "gpt-5.5")
        );
        for (timestamp, expected) in [
            ("2025-01-01T00:00:00Z", "gpt-5"),
            ("2025-09-15T00:00:00Z", "gpt-5-codex"),
            ("2026-07-29T23:59:59Z", "gpt-5.4"),
            ("2026-07-30T00:00:00Z", "gpt-5.6-luna"),
            ("2026-07-30T00:30:00+01:00", "gpt-5.4"),
        ] {
            assert_eq!(
                codex_log_model_fallback(
                    CODEX_AUTO_REVIEW_MODEL,
                    parse_ts_timestamp(timestamp).unwrap()
                ),
                Some(expected),
                "{timestamp}"
            );
        }
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

        let events = crate::test_support::rollout_events(fixture.root());

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
        assert_eq!(compacted_usages[0].model.as_str(), "gpt-reserve");
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
            let events = crate::test_support::rollout_events(fixture.root());
            assert_eq!(events.last().unwrap().model.as_str(), "gpt-5");
            assert!(!events.last().unwrap().is_fallback_model);
        }
    }

    #[test]
    fn recognizes_spaced_compaction_types_with_compact_nested_types() {
        for line in [
            br#"{"type" : "token_usage_record","payload":{"type":"request"}}"#.as_slice(),
            br#"{"type" : "compacted","payload":{"type":"summary"}}"#.as_slice(),
        ] {
            assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Session));
        }
    }

    #[test]
    fn headless_usage_field_search_matches_the_previous_implementation() {
        let markers: [&[u8]; 3] = [
            br#""usage":"#,
            br#""input_tokens":"#,
            br#""prompt_tokens":"#,
        ];
        let fragments: &[&[u8]] = &[
            br#""usage":"#,
            br#""input_tokens":"#,
            br#""prompt_tokens":"#,
            br#""usage" :"#,
            br#""cached_input_tokens":"#,
            br#""my_usage":"#,
            br#""usage_details":"#,
            br#"\"usage\":"#,
            br#"\\u0061"#,
            br#"\u0061"#,
            br#""#,
            b":",
            b"{",
            b"}",
            b"\\",
            b"\\\\",
            b" ",
        ];
        let finders = markers.map(Finder::new);
        let mut state = 0xbb67_ae85_84ca_a73b_u64;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % bound as u64).unwrap_or_default()
        };
        for _ in 0..50_000 {
            let line = (0..next(32))
                .flat_map(|_| fragments[next(fragments.len())].iter().copied())
                .collect::<Vec<_>>();
            assert_eq!(
                has_headless_usage_field(&line),
                finders.iter().any(|finder| finder.find(&line).is_some()),
                "{}",
                String::from_utf8_lossy(&line)
            );
        }
    }

    #[test]
    fn recognizes_headless_usage_fields_only_as_whole_keys() {
        for line in [
            br#"{"type":"turn.completed","usage":{"input_tokens":1}}"#.as_slice(),
            br#"{"stats":{"input_tokens":1}}"#.as_slice(),
            br#"{"x":1,"prompt_tokens":2}"#.as_slice(),
        ] {
            assert_matches!(codex_line_usage_kind(line), Some(CodexLineKind::Headless));
        }
        for line in [
            br#"{"usage" :{"input_tokens" :1}}"#.as_slice(),
            br#"{"text":"\"usage\":1","cached_input_tokens":1}"#.as_slice(),
            br#"{"my_usage":1,"prompt_tokens_details":{}}"#.as_slice(),
        ] {
            assert_matches!(codex_line_usage_kind(line), None);
        }
    }

    #[test]
    fn recognizes_usage_lines_whose_markers_are_spelled_with_escapes() {
        for (line, kind) in [
            (
                br#"{"type":"turn\u005fcontext","payload":{}}"#.as_slice(),
                CodexLineKind::Escaped,
            ),
            (
                br#"{"type":"event_msg","payload":{"type":"token\u005fcount"}}"#.as_slice(),
                CodexLineKind::Escaped,
            ),
            (
                br#"{"type":"compact\u0065d","payload":{}}"#.as_slice(),
                CodexLineKind::Escaped,
            ),
            (
                br#"{"type":"turn.completed","us\u0061ge":{"input_tokens":1}}"#.as_slice(),
                CodexLineKind::Escaped,
            ),
        ] {
            assert_eq!(
                codex_line_usage_kind(line),
                Some(kind),
                "{}",
                String::from_utf8_lossy(line)
            );
        }
        assert_eq!(
            codex_line_usage_kind(
                br#"{"type":"response_item","payload":{"text":"a\nb \u001b[1m"}}"#
            ),
            None
        );
    }

    #[test]
    fn loads_token_counts_whose_types_are_spelled_with_escapes() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{"type":"turn\u005fcontext","payload":{"model":"gpt-5-mini"}}"#,
                r#"{"timestamp":"2026-09-01T00:00:01Z","type":"event\u005fmsg","payload":{"type":"token\u005fcount","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}"#,
            ].join("\n"),
        });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 120);
        assert_eq!(events[0].model.as_str(), "gpt-5-mini");
    }

    #[test]
    fn loads_token_counts_with_spaces_after_type_colons() {
        let fixture = fs_fixture!({
            "session.jsonl": [
                r#"{"type": "turn_context", "payload": {"model": "gpt-5-mini"}}"#,
                r#"{"timestamp": "2026-09-01T00:00:01Z", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 20}}}}"#,
            ].join("\n"),
        });
        let events = crate::test_support::rollout_events(fixture.root());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 120);
        assert_eq!(events[0].model.as_str(), "gpt-5-mini");
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
        let events = crate::test_support::rollout_events(fixture.root());

        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event.total_tokens)
                .collect::<Vec<_>>(),
            [120, 330, 60]
        );
        assert_eq!(events[1].cached_input_tokens, 200);
        assert_eq!(events[1].model.as_str(), "gpt-5");
        assert!(events[1].is_fallback_model);
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_test_support::Fixture;

    #[test]
    fn a_rollout_removed_after_discovery_is_noted_not_skipped() {
        let fixture = Fixture::new();
        let path = fixture.path("session.jsonl");
        let diagnostics = Diagnostics::new("codex");

        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |_| {},
            &diagnostics,
        );

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
        let debug = crate::cli::SharedArgs {
            debug: true,
            ..Default::default()
        };
        assert_eq!(
            diagnostics.warnings(&debug),
            [format!(
                "codex: {}: removed after discovery, read as absent",
                path.display()
            )]
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_sibling_encoding_of_a_removed_rollout_is_a_skip() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let fixture = Fixture::new();
        let path = fixture.path("session.jsonl");
        let compressed = fixture.write_file("session.jsonl.zst", "rollout");
        fs::set_permissions(&compressed, fs::Permissions::from_mode(0o000)).unwrap();
        let diagnostics = Diagnostics::new("codex");

        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |_| {},
            &diagnostics,
        );
        fs::set_permissions(&compressed, fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
        let warnings = diagnostics
            .warnings(&crate::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(
            warnings.contains(&format!("{}: rollout open failed", compressed.display())),
            "{warnings}"
        );
    }

    #[test]
    fn a_rollout_line_still_being_written_is_not_a_skip() {
        let fixture = Fixture::new();
        let path = fixture.write_file(
            "session.jsonl",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_tok"#,
        );
        let diagnostics = Diagnostics::new("codex");
        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |_| {},
            &diagnostics,
        );

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn malformed_usage_lines_skip_the_same_whether_or_not_a_marker_is_escaped() {
        for malformed in [
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","note":"compact\u0065d","info":{"secret":"#,
        ] {
            let fixture = Fixture::new();
            let path = fixture.write_file(
                "session.jsonl",
                [
                    malformed,
                    r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
                ]
                .join("\n"),
            );
            let diagnostics = Diagnostics::new("codex");
            visit_codex_session_file_with_diagnostics(
                fixture.root(),
                &path,
                None,
                |_| {},
                &diagnostics,
            );

            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1, "{malformed}");
        }
    }

    #[test]
    fn nonusage_and_suppressed_session_records_stay_quiet() {
        let fixture = Fixture::new();
        let path = fixture.write_file("session.jsonl", [
            r#"{"type":"turn_context","timestamp":{},"payload":{"model":"SECRET_CANARY"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0}}}}"#,
        ].join("\n"));
        let diagnostics = Diagnostics::new("codex");
        let mut events = Vec::new();
        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |event| {
                events.push(event);
            },
            &diagnostics,
        );
        assert!(events.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
        let _log_level = ccusage_test_support::EnvVarGuard::set("LOG_LEVEL", "1");
        assert!(
            diagnostics
                .warnings(&crate::cli::SharedArgs::default())
                .is_empty()
        );
    }

    fn session_events(lines: &[&str]) -> (Vec<CodexTokenUsageEvent>, Diagnostics) {
        let fixture = Fixture::new();
        let path = fixture.write_file("session.jsonl", lines.join("\n"));
        let diagnostics = Diagnostics::new("codex");
        let mut events = Vec::new();
        visit_codex_session_file_with_diagnostics(
            fixture.root(),
            &path,
            None,
            |event| events.push(event),
            &diagnostics,
        );
        (events, diagnostics)
    }

    #[test]
    fn malformed_descriptive_session_fields_keep_the_usage() {
        let (events, diagnostics) = session_events(&[
            r#"{"type":"turn_context","timestamp":{},"payload":{"model":"gpt-5.4","metadata":[]}}"#,
            r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","model":{},"model_name":7,"metadata":"x","info":{"model":[],"last_token_usage":{"input_tokens":10,"output_tokens":2},"total_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:01Z","payload":{"type":"thread_settings_applied","thread_settings":"fast"}}"#,
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].input_tokens, events[0].output_tokens), (10, 2));
        assert_eq!(events[0].model, "gpt-5.4");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }

    #[test]
    fn session_usage_objects_keep_the_last_value_of_a_repeated_key() {
        let (events, diagnostics) = session_events(&[
            r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":999,"input_tokens":10,"output_tokens":2}}}}"#,
            r#"{"type":"token_usage_record","timestamp":"2026-01-02T00:00:01Z","payload":{"response_id":"r","usage":{"input_tokens":999,"input_tokens":20,"output_tokens":3}}}"#,
            r#"{"type":"compacted","timestamp":"2026-01-02T00:00:02Z","payload":{"compaction_response_id":"r"}}"#,
        ]);
        let usage: Vec<_> = events
            .iter()
            .map(|event| (event.input_tokens, event.output_tokens))
            .collect();
        assert_eq!(usage, [(10, 2), (20, 3)]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn a_malformed_usage_field_is_named_by_its_static_key_path() {
        let (events, diagnostics) = session_events(&[
            r#"{"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":{"x":"SECRET_CANARY"}}}}}"#,
            r#"{"type":"event_msg","timestamp":{"x":"SECRET_CANARY"},"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10}}}}"#,
            r#"{"type":"token_usage_record","timestamp":"2026-01-02T00:00:01Z","payload":{"response_id":"r","usage":{"input_tokens":10},"thread_token_usage":{"input_tokens":[]}}}"#,
        ]);
        assert!(events.is_empty());
        let warnings = diagnostics
            .warnings(&crate::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 3, "{warnings}");
        for field in [
            "payload.info.last_token_usage",
            "timestamp",
            "payload.thread_token_usage",
        ] {
            assert!(
                warnings.contains(&format!("field `{field}` did not decode")),
                "{warnings}"
            );
        }
        assert!(!warnings.contains("SECRET_CANARY"), "{warnings}");
    }

    #[test]
    fn numeric_timestamps_beyond_the_supported_range_do_not_date_usage() {
        let (events, diagnostics) = session_events(&[
            r#"{"type":"event_msg","timestamp":1767312000000000,"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
        ]);
        assert!(events.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);

        let (events, _) = session_events(&[
            r#"{"type":"turn.completed","timestamp":1767312000000000,"created_at":"2026-01-02T00:00:00Z","usage":{"input_tokens":10,"output_tokens":2}}"#,
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].timestamp,
            parse_ts_timestamp("2026-01-02T00:00:00Z").unwrap()
        );
    }
}
