use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};

use ccusage_core::{Speed, TimestampMs, TokenUsageRaw, fast::FxHashMap};
use rustc_hash::FxHasher;
use smallvec::SmallVec;

use super::paths::usage_file_order;

pub(super) struct DedupeIndex {
    index: usize,
    session_alias: Option<Arc<str>>,
}

pub(super) type DedupeIndexVec = SmallVec<[DedupeIndex; 1]>;

// NOTE(2026-03-09, #886): Claude Code bills fast-mode usage at a higher rate. Prefer the fast record when otherwise equal copies disagree on speed.
fn reports_more_of_the_request<E: DedupEntry>(candidate: &E, existing: &E) -> bool {
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
fn usage_dedupe_hash(
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
fn is_per_response_message_id(message_id: &str) -> bool {
    message_id.starts_with("msg_") || message_id.starts_with("resp_")
}

fn sidechain_replay_dedupe_hash(message_id: &str, session_id: &str) -> u64 {
    let mut hasher = FxHasher::default();
    "sidechain-replay".hash(&mut hasher);
    message_id.hash(&mut hasher);
    session_id.hash(&mut hasher);
    hasher.finish()
}

fn sidechain_entry_replay_dedupe_hash(message_id: &str, session_id: &str) -> u64 {
    let mut hasher = FxHasher::default();
    "sidechain-replay-entry".hash(&mut hasher);
    message_id.hash(&mut hasher);
    session_id.hash(&mut hasher);
    hasher.finish()
}

fn push_deduped_index(
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

fn push_new_deduped_index(
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    hash: u64,
    index: usize,
) {
    deduped_indexes.entry(hash).or_default().push(DedupeIndex {
        index,
        session_alias: None,
    });
}

fn push_deduped_session_alias(
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

pub(super) fn dedupe_files<E: DedupEntry>(
    mut files: Vec<(PathBuf, Vec<E>)>,
) -> (Vec<E>, Vec<(usize, Arc<str>)>) {
    files.sort_by(|(left, _), (right, _)| usage_file_order(left, right));
    let entry_count = files.iter().map(|(_, entries)| entries.len()).sum();
    let mut deduped_indexes: FxHashMap<u64, DedupeIndexVec> =
        FxHashMap::with_capacity_and_hasher(entry_count, Default::default());
    let mut deduped = Vec::with_capacity(entry_count);
    let mut copies = Vec::new();
    for (_, entries) in files {
        for entry in entries {
            if let Some((index, discarded)) =
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
                && discarded.session_id() != deduped[index].session_id()
                && !discarded.is_sidechain()
            {
                copies.push((index, discarded.owned_session_id()));
            }
        }
    }
    (deduped, copies)
}

#[derive(Debug)]
pub(super) struct DedupKey {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: Arc<str>,
    pub(super) message_id: Option<String>,
    pub(super) request_id: Option<String>,
    pub(super) is_sidechain: Option<bool>,
}

pub(super) trait DedupEntry {
    fn timestamp(&self) -> TimestampMs;
    fn session_id(&self) -> &str;
    fn owned_session_id(&self) -> Arc<str>;
    fn message_id(&self) -> Option<&str>;
    fn request_id(&self) -> Option<&str>;
    fn is_sidechain(&self) -> bool;
    fn usage(&self) -> &TokenUsageRaw;
    fn records_cost(&self) -> bool;
}

fn usage_token_total(entry: &impl DedupEntry) -> u64 {
    let usage = entry.usage();
    usage.input_tokens
        + usage.output_tokens
        + usage.cache_creation_token_count()
        + usage.cache_read_input_tokens
}

pub(super) fn push_deduped_entry<E: DedupEntry>(
    entry: E,
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    deduped: &mut Vec<E>,
) -> Option<(usize, E)> {
    let dedupe_lookup = entry.message_id().map(|message_id| {
        let request_id = entry.request_id();
        let exact_hash = usage_dedupe_hash(
            message_id,
            request_id,
            entry.session_id(),
            entry.timestamp(),
        );
        let existing_index = deduped_indexes
            .get(&exact_hash)
            .and_then(|indexes| {
                indexes.iter().find_map(|dedupe_index| {
                    let existing = &deduped[dedupe_index.index];
                    (existing.message_id() == Some(message_id)
                        && existing.request_id() == request_id
                        && (request_id.is_some()
                            || (existing.session_id() == entry.session_id()
                                && (is_per_response_message_id(message_id)
                                    || existing.timestamp() == entry.timestamp()))))
                    .then_some(dedupe_index.index)
                })
            })
            .or_else(|| {
                // NOTE(2026-05-25, #913): Claude Code /btw sidechains can replay parent usage with the same message ID and a new request ID. Match across request IDs to avoid overcounting (https://github.com/ccusage/ccusage/issues/913).
                let candidate_is_sidechain = entry.is_sidechain();
                let route_hash = if candidate_is_sidechain {
                    sidechain_replay_dedupe_hash(message_id, entry.session_id())
                } else {
                    sidechain_entry_replay_dedupe_hash(message_id, entry.session_id())
                };
                deduped_indexes.get(&route_hash).and_then(|indexes| {
                    indexes.iter().find_map(|dedupe_index| {
                        let existing = &deduped[dedupe_index.index];
                        let indexed_session_id = dedupe_index
                            .session_alias
                            .as_deref()
                            .unwrap_or(existing.session_id());
                        (existing.message_id() == Some(message_id)
                            && indexed_session_id == entry.session_id()
                            && (candidate_is_sidechain || existing.is_sidechain()))
                        .then_some(dedupe_index.index)
                    })
                })
            });
        (exact_hash, existing_index)
    });

    if let Some((hash, Some(index))) = dedupe_lookup {
        if deduped[index].session_id() != entry.session_id()
            && let Some(message_id) = entry.message_id()
        {
            let existing_session_id = deduped[index].session_id();
            for session_id in [entry.session_id(), existing_session_id] {
                for route_hash in [
                    sidechain_replay_dedupe_hash(message_id, session_id),
                    sidechain_entry_replay_dedupe_hash(message_id, session_id),
                ] {
                    push_deduped_session_alias(deduped_indexes, route_hash, index, session_id);
                }
            }
        }
        let discarded = if should_replace_deduped_entry(&entry, &deduped[index]) {
            let previous = std::mem::replace(&mut deduped[index], entry);
            if entry_exact_hash(&previous) != Some(hash) {
                push_deduped_index(deduped_indexes, hash, index);
            }
            let survivor = &deduped[index];
            let replay_route_changed = previous.session_id() != survivor.session_id()
                || previous.is_sidechain() != survivor.is_sidechain()
                || previous.message_id() != survivor.message_id();
            if replay_route_changed && let Some(message_id) = survivor.message_id() {
                let session_id = survivor.session_id();
                push_deduped_index(
                    deduped_indexes,
                    sidechain_replay_dedupe_hash(message_id, session_id),
                    index,
                );
                if survivor.is_sidechain() {
                    push_deduped_index(
                        deduped_indexes,
                        sidechain_entry_replay_dedupe_hash(message_id, session_id),
                        index,
                    );
                }
            }
            previous
        } else {
            entry
        };
        return Some((index, discarded));
    }

    let index = deduped.len();
    deduped.push(entry);
    if let Some((hash, None)) = dedupe_lookup {
        push_new_deduped_index(deduped_indexes, hash, index);
        let survivor = &deduped[index];
        if let Some(message_id) = survivor.message_id() {
            let session_id = survivor.session_id();
            push_new_deduped_index(
                deduped_indexes,
                sidechain_replay_dedupe_hash(message_id, session_id),
                index,
            );
            if survivor.is_sidechain() {
                push_new_deduped_index(
                    deduped_indexes,
                    sidechain_entry_replay_dedupe_hash(message_id, session_id),
                    index,
                );
            }
        }
    }
    None
}

fn entry_exact_hash(entry: &impl DedupEntry) -> Option<u64> {
    entry.message_id().map(|message_id| {
        usage_dedupe_hash(
            message_id,
            entry.request_id(),
            entry.session_id(),
            entry.timestamp(),
        )
    })
}

fn should_replace_deduped_entry<E: DedupEntry>(candidate: &E, existing: &E) -> bool {
    let candidate_is_sidechain = candidate.is_sidechain();
    let existing_is_sidechain = existing.is_sidechain();
    if candidate_is_sidechain != existing_is_sidechain {
        return existing_is_sidechain;
    }

    let candidate_total = usage_token_total(candidate);
    let existing_total = usage_token_total(existing);
    if candidate_total != existing_total {
        return candidate_total > existing_total;
    }
    reports_more_of_the_request(candidate, existing)
}
