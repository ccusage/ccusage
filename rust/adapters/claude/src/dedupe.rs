use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use ccusage_core::{Speed, TimestampMs, fast::FxHashMap};
use rustc_hash::FxHasher;
use smallvec::SmallVec;

use super::daily;

pub(super) struct DedupeIndex {
    pub(super) index: usize,
    pub(super) session_alias: Option<Arc<str>>,
}

pub(super) type DedupeIndexVec = SmallVec<[DedupeIndex; 1]>;

// NOTE(2026-03-09, #886): Claude Code bills fast-mode usage at a higher rate. Prefer the fast record when otherwise equal copies disagree on speed.
pub(super) fn reports_more_of_the_request<E: daily::DedupEntry>(
    candidate: &E,
    existing: &E,
) -> bool {
    let detail = |entry: &E| {
        let usage = entry.usage();
        let speed_rank = match usage.speed {
            None => 0,
            Some(Speed::Standard) => 1,
            Some(Speed::Fast) => 2,
        };
        (
            speed_rank,
            usage.cache_creation.is_some(),
            entry.records_cost(),
        )
    };
    detail(candidate) > detail(existing)
}

// NOTE(2026-09-18, #1765): Claude Code can copy a response into multiple transcripts with the same message and request IDs. Match those IDs across sessions.
pub(super) fn usage_dedupe_hash(
    message_id: &str,
    request_id: Option<&str>,
    session_id: &str,
    timestamp: TimestampMs,
) -> u64 {
    let mut hasher = FxHasher::default();
    message_id.hash(&mut hasher);
    request_id.hash(&mut hasher);
    if request_id.is_none() {
        session_id.hash(&mut hasher);
        // NOTE(2026-09-25, #1799): Some gateways reuse a message ID for independent responses without request IDs. Keep their distinct timestamps separate.
        if !is_per_response_message_id(message_id) {
            timestamp.hash(&mut hasher);
        }
    }
    hasher.finish()
}

// NOTE(2026-10-10, #1838): Claude Code writes content blocks of one response on separate lines with the same response ID and different timestamps. Match those IDs across timestamps, including OpenAI Responses IDs (#1935).
pub(crate) fn is_per_response_message_id(message_id: &str) -> bool {
    message_id.starts_with("msg_") || message_id.starts_with("resp_")
}

pub(super) fn sidechain_replay_dedupe_hash(message_id: &str, session_id: &str) -> u64 {
    let mut hasher = FxHasher::default();
    "sidechain-replay".hash(&mut hasher);
    message_id.hash(&mut hasher);
    session_id.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn sidechain_entry_replay_dedupe_hash(message_id: &str, session_id: &str) -> u64 {
    let mut hasher = FxHasher::default();
    "sidechain-replay-entry".hash(&mut hasher);
    message_id.hash(&mut hasher);
    session_id.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn push_deduped_index(
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    hash: u64,
    index: usize,
) {
    let indexes = deduped_indexes.entry(hash).or_default();
    if !indexes
        .iter()
        .any(|dedupe_index| dedupe_index.index == index && dedupe_index.session_alias.is_none())
    {
        indexes.push(DedupeIndex {
            index,
            session_alias: None,
        });
    }
}

pub(super) fn push_new_deduped_index(
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    hash: u64,
    index: usize,
) {
    deduped_indexes.entry(hash).or_default().push(DedupeIndex {
        index,
        session_alias: None,
    });
}

pub(super) fn push_deduped_session_alias(
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    hash: u64,
    index: usize,
    session_id: &str,
) {
    let indexes = deduped_indexes.entry(hash).or_default();
    if !indexes.iter().any(|dedupe_index| {
        dedupe_index.index == index && dedupe_index.session_alias.as_deref() == Some(session_id)
    }) {
        indexes.push(DedupeIndex {
            index,
            session_alias: Some(Arc::from(session_id)),
        });
    }
}
