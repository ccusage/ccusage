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
    foreign_before_own: bool,
}

impl<'a> CodexReplayFilter<'a> {
    pub(super) fn new(fork: Option<CodexForkReplay<'a>>) -> Self {
        Self {
            fork,
            pending: Vec::new(),
            own_start: None,
            foreign_before_own: false,
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
        let is_own = thread_id.is_some() && thread_id == fork.child_id;
        if is_own {
            self.own_start.get_or_insert(self.pending.len());
            if marker == CodexOwnershipMarker::UsageRecord {
                self.resolve_by_ownership(visit);
                return Some(CodexReplayResolution::Deterministic);
            }
            return None;
        }
        if marker == CodexOwnershipMarker::UsageRecord
            && thread_id.is_none()
            && fork.ends_at_usage_record
        {
            return Some(self.resolve_by_parent(visit));
        }
        let is_foreign = match marker {
            CodexOwnershipMarker::UsageRecord => thread_id.is_some(),
            CodexOwnershipMarker::ThreadSettings => true,
        };
        if is_foreign && self.own_start.is_none() {
            self.foreign_before_own = true;
        }
        None
    }

    pub(super) fn finish(
        &mut self,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) -> Option<CodexReplayResolution<'a>> {
        self.fork?;
        if self.own_start.is_some() {
            self.resolve_by_ownership(visit);
            return Some(CodexReplayResolution::Deterministic);
        }
        Some(self.resolve_by_parent(visit))
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

    fn resolve_by_ownership(&mut self, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        let inherited = if self.foreign_before_own {
            self.pending[..self.own_start.unwrap_or(0)]
                .iter()
                .filter(|event| is_replayable(event))
                .count()
        } else {
            0
        };
        self.emit_after(inherited, visit);
    }

    fn resolve_by_parent(
        &mut self,
        visit: &mut impl FnMut(CodexTokenUsageEvent),
    ) -> CodexReplayResolution<'a> {
        let Some(fork) = self.fork else {
            return CodexReplayResolution::Deterministic;
        };
        let resolution = match fork.missing_parent {
            Some(parent_id) if !self.pending.is_empty() => {
                CodexReplayResolution::MissingParent(parent_id)
            }
            _ => CodexReplayResolution::Deterministic,
        };
        let leading = self
            .pending
            .iter()
            .filter(|event| is_replayable(event))
            .map(CodexTokenUsageEvent::raw_usage)
            .collect::<Vec<_>>();
        let inherited = replayed_parent_len(fork.parent_usage, &leading);
        self.emit_after(inherited, visit);
        resolution
    }

    fn emit_after(&mut self, mut inherited: usize, visit: &mut impl FnMut(CodexTokenUsageEvent)) {
        self.fork = None;
        for event in std::mem::take(&mut self.pending) {
            if inherited > 0 && is_replayable(&event) {
                inherited -= 1;
            } else {
                visit(event);
            }
        }
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
