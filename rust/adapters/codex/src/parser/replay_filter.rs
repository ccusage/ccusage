use crate::{CodexRawUsage, CodexTokenUsageEvent};

use super::CodexForkReplay;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CodexOwnershipMarker {
    UsageRecord,
    ThreadSettings,
}

pub(super) enum CodexReplayResolution<'a> {
    Deterministic,
    MissingParent(&'a str),
}

pub(super) struct CodexReplayFilter<'a> {
    fork: Option<CodexForkReplay<'a>>,
    pending: Vec<CodexTokenUsageEvent>,
    own_start: Option<usize>,
    foreign_start: Option<usize>,
}

impl<'a> CodexReplayFilter<'a> {
    pub(super) fn new(fork: Option<CodexForkReplay<'a>>) -> Self {
        Self {
            fork,
            pending: Vec::new(),
            own_start: None,
            foreign_start: None,
        }
    }

    pub(super) fn is_done(&self) -> bool {
        self.fork.is_none()
    }

    pub(super) fn observe_marker(
        &mut self,
        marker: CodexOwnershipMarker,
        thread_id: Option<&str>,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) -> Option<CodexReplayResolution<'a>> {
        let fork = self.fork?;
        let thread_id = thread_id.filter(|id| !id.trim().is_empty());
        let is_own = thread_id.is_some() && thread_id == fork.child_id;
        if is_own {
            self.own_start.get_or_insert(self.pending.len());
            if marker == CodexOwnershipMarker::UsageRecord {
                return Some(self.resolve_leading(visit));
            }
            return None;
        }
        if marker == CodexOwnershipMarker::UsageRecord
            && thread_id.is_none()
            && fork.ends_at_usage_record
        {
            return Some(self.resolve_leading(visit));
        }
        if thread_id.is_some() && fork.child_id.is_some() && self.own_start.is_none() {
            self.foreign_start.get_or_insert(self.pending.len());
        }
        None
    }

    pub(super) fn finish(
        &mut self,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) -> Option<CodexReplayResolution<'a>> {
        self.fork?;
        Some(self.resolve_leading(visit))
    }

    pub(super) fn visit(
        &mut self,
        event: CodexTokenUsageEvent,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) {
        if self.fork.is_none() {
            visit(event);
            return;
        }
        self.pending.push(event);
    }

    fn resolve_leading(
        &mut self,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) -> CodexReplayResolution<'a> {
        let Some(fork) = self.fork else {
            return CodexReplayResolution::Deterministic;
        };
        let window_end = self.own_start.unwrap_or(self.pending.len());
        let (unproven_end, copied) = match (self.foreign_start, self.own_start) {
            (Some(foreign), Some(own)) => (foreign, foreign..own),
            _ => (window_end, 0..0),
        };
        let resolution = match fork.missing_parent {
            Some(parent_id) if self.pending[..unproven_end].iter().any(is_replayable) => {
                CodexReplayResolution::MissingParent(parent_id)
            }
            _ => CodexReplayResolution::Deterministic,
        };
        let leading = self.pending[..window_end]
            .iter()
            .filter(|event| is_replayable(event))
            .map(CodexTokenUsageEvent::raw_usage)
            .take(fork.parent_usage.len())
            .collect::<Vec<_>>();
        let mut inherited = replayed_parent_len(fork.parent_usage, &leading);
        self.fork = None;
        for (index, event) in std::mem::take(&mut self.pending).into_iter().enumerate() {
            if !is_replayable(&event) {
                visit(event);
            } else if index < window_end && inherited > 0 {
                inherited -= 1;
            } else if !copied.contains(&index) {
                visit(event);
            }
        }
        resolution
    }
}

fn is_replayable(event: &CodexTokenUsageEvent) -> bool {
    event.response_id.is_none()
}

fn replayed_parent_len(parent: &[CodexRawUsage], child: &[CodexRawUsage]) -> usize {
    let prefix = parent
        .iter()
        .zip(child)
        .take_while(|(parent, child)| parent == child)
        .count();
    prefix.max(longest_parent_suffix_prefix(parent, child))
}

fn longest_parent_suffix_prefix(parent: &[CodexRawUsage], child: &[CodexRawUsage]) -> usize {
    let pattern = &child[..child.len().min(parent.len())];
    if pattern.is_empty() {
        return 0;
    }
    let mut failure = vec![0; pattern.len()];
    let mut matched = 0;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = failure[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        failure[index] = matched;
    }
    matched = 0;
    for usage in &parent[parent.len() - pattern.len()..] {
        while matched > 0 && *usage != pattern[matched] {
            matched = failure[matched - 1];
        }
        if *usage == pattern[matched] {
            matched += 1;
            if matched == pattern.len() {
                return matched;
            }
        }
    }
    matched
}

#[cfg(test)]
mod tests;
