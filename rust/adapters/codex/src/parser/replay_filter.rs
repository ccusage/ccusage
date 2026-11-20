use crate::{CodexRawUsage, CodexTokenUsageEvent, TimestampMs};

use super::normalize_codex_raw_usage;

pub(super) enum CodexReplayState<'a> {
    MatchingParent {
        prefix: &'a [CodexRawUsage],
        index: usize,
    },
    SkippingRewrittenBurst(TimestampMs),
    Done,
}

// NOTE(2026-07-27, #1501): Codex rewrites replay timestamps near the fork instant. Observed bursts spanned 10–40 ms, crossed second boundaries, and preceded a 5.8–15.3 s pause before the child's own usage.
const CODEX_REWRITTEN_BURST_PAUSE_MS: i64 = 1_000;

#[derive(Default)]
pub(super) struct CodexBurst {
    pub(super) session_start: Option<TimestampMs>,
    first: Option<TimestampMs>,
    decision: Option<Option<TimestampMs>>,
}

impl CodexBurst {
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
        // NOTE(2026-10-26, #1971): A child's first request can produce a dense pair after the fork instant. Density alone can mistake it for replay.
        let starts_at_fork = self.session_start.is_none_or(|start| {
            first.as_millis().abs_diff(start.as_millis())
                <= CODEX_REWRITTEN_BURST_PAUSE_MS.unsigned_abs()
        });
        self.decision = Some((is_dense && starts_at_fork).then_some(first));
    }
}

pub(super) enum CodexPendingReplay {
    Event(CodexTokenUsageEvent),
    UsageRecord,
}

pub(super) struct CodexReplayFilter<'a> {
    pub(super) state: CodexReplayState<'a>,
    pub(super) ends_at_usage_record: bool,
    pub(super) burst: CodexBurst,
    pub(super) pending: Vec<CodexPendingReplay>,
}

impl CodexReplayFilter<'_> {
    pub(super) fn observe_usage_record(&mut self) {
        if self.ends_at_usage_record {
            if self.burst.decision.is_none() && !self.pending.is_empty() {
                self.pending.push(CodexPendingReplay::UsageRecord);
            } else {
                self.state = CodexReplayState::Done;
            }
        }
    }

    pub(super) fn observe_token_count(
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

    pub(super) fn finish(&mut self, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        self.burst.decision.get_or_insert(None);
        self.flush(visit);
    }

    pub(super) fn visit(
        &mut self,
        event: CodexTokenUsageEvent,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) {
        if self.burst.decision.is_none() && !self.pending.is_empty() {
            self.pending.push(CodexPendingReplay::Event(event));
            return;
        }
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
