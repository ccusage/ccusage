use std::{
    hash::{Hash, Hasher},
    num::NonZeroU64,
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

#[derive(Default)]
pub(super) struct DedupeIndexes {
    buckets: FxHashMap<u64, DedupeIndexVec>,
    merged: FxHashMap<usize, usize>,
    copies: Vec<(usize, Arc<str>)>,
}

impl DedupeIndexes {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
            merged: FxHashMap::default(),
            copies: Vec::new(),
        }
    }

    fn get(&self, hash: &u64) -> Option<&DedupeIndexVec> {
        self.buckets.get(hash)
    }

    fn resolve(&self, mut index: usize) -> usize {
        while let Some(&survivor) = self.merged.get(&index) {
            index = survivor;
        }
        index
    }
}

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
        timestamp.hash(&mut hasher);
    }
    hasher.finish()
}

// NOTE(2026-11-20, #2255): Claude Code writes each content block of a response on its own line with its own timestamp. Without a request ID, match blocks across timestamps only when the transcript chain places them in one response (#1838, #1935).
fn response_dedupe_hash(message_id: &str, session_id: &str, response: NonZeroU64) -> u64 {
    let mut hasher = FxHasher::default();
    "response".hash(&mut hasher);
    message_id.hash(&mut hasher);
    session_id.hash(&mut hasher);
    response.hash(&mut hasher);
    hasher.finish()
}

fn requestless_response_hash(entry: &impl DedupEntry) -> Option<u64> {
    let response = entry.request().response()?;
    Some(response_dedupe_hash(
        entry.message_id()?,
        entry.session_id(),
        response,
    ))
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

fn push_deduped_index(deduped_indexes: &mut DedupeIndexes, hash: u64, index: usize) {
    let indexes = deduped_indexes.buckets.entry(hash).or_default();
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

fn push_new_deduped_index(deduped_indexes: &mut DedupeIndexes, hash: u64, index: usize) {
    deduped_indexes
        .buckets
        .entry(hash)
        .or_default()
        .push(DedupeIndex {
            index,
            session_alias: None,
        });
}

fn push_deduped_session_alias(
    deduped_indexes: &mut DedupeIndexes,
    hash: u64,
    index: usize,
    session_id: &str,
) {
    let indexes = deduped_indexes.buckets.entry(hash).or_default();
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
    let mut deduped_indexes = DedupeIndexes::with_capacity(entry_count);
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
    if deduped_indexes.merged.is_empty() {
        return (deduped, copies);
    }
    copies.append(&mut deduped_indexes.copies);
    let mut kept = vec![usize::MAX; deduped.len()];
    let mut survivors = Vec::with_capacity(deduped.len() - deduped_indexes.merged.len());
    for (index, entry) in deduped.into_iter().enumerate() {
        if !deduped_indexes.merged.contains_key(&index) {
            kept[index] = survivors.len();
            survivors.push(entry);
        }
    }
    for (index, _) in &mut copies {
        *index = kept[deduped_indexes.resolve(*index)];
    }
    (survivors, copies)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Request {
    Id(Box<str>),
    Missing(Option<NonZeroU64>),
}

const _: () = assert!(size_of::<Request>() == size_of::<Option<Box<str>>>());

impl Request {
    pub(super) fn new(request_id: Option<String>, response: Option<NonZeroU64>) -> Self {
        request_id.map_or(Self::Missing(response), |id| Self::Id(id.into_boxed_str()))
    }

    pub(super) fn id(&self) -> Option<&str> {
        match self {
            Self::Id(id) => Some(id),
            Self::Missing(_) => None,
        }
    }

    pub(super) fn response(&self) -> Option<NonZeroU64> {
        match self {
            Self::Id(_) => None,
            Self::Missing(response) => *response,
        }
    }
}

#[derive(Debug)]
pub(super) struct DedupKey {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: Arc<str>,
    pub(super) message_id: Option<Box<str>>,
    pub(super) request: Request,
}

impl DedupKey {
    pub(super) fn advisor(&self, index: usize) -> Self {
        Self {
            timestamp: self.timestamp,
            session_id: Arc::clone(&self.session_id),
            message_id: self
                .message_id
                .as_ref()
                .map(|id| format!("{id}:advisor:{index}").into_boxed_str()),
            request: self.request.clone(),
        }
    }
}

pub(super) trait DedupEntry {
    fn timestamp(&self) -> TimestampMs;
    fn session_id(&self) -> &str;
    fn owned_session_id(&self) -> Arc<str>;
    fn message_id(&self) -> Option<&str>;
    fn request(&self) -> &Request;
    fn request_id(&self) -> Option<&str> {
        self.request().id()
    }
    fn is_sidechain(&self) -> bool;
    fn usage(&self) -> &TokenUsageRaw;
    fn records_cost(&self) -> bool;
}

fn usage_token_total(entry: &impl DedupEntry) -> u64 {
    ccusage_core::total_usage_tokens(*entry.usage())
}

pub(super) fn push_deduped_entry<E: DedupEntry>(
    entry: E,
    deduped_indexes: &mut DedupeIndexes,
    deduped: &mut Vec<E>,
) -> Option<(usize, E)> {
    let requestless_hashes = requestless_hashes(&entry);
    let dedupe_lookup = entry.message_id().map(|message_id| {
        let request_id = entry.request_id();
        let exact_hash = usage_dedupe_hash(
            message_id,
            request_id,
            entry.session_id(),
            entry.timestamp(),
        );
        let requestless_match = |hash: u64| {
            deduped_indexes.get(&hash)?.iter().find_map(|dedupe_index| {
                let index = deduped_indexes.resolve(dedupe_index.index);
                let existing = &deduped[index];
                (existing.message_id() == Some(message_id)).then_some(index)
            })
        };
        let existing_index = if request_id.is_none() {
            let exact = requestless_match(exact_hash);
            let response = requestless_response_hash(&entry).and_then(requestless_match);
            match (exact, response) {
                (Some(exact), Some(response)) if exact != response => {
                    Some(merge_slots(deduped_indexes, deduped, exact, response))
                }
                (exact, response) => exact.or(response),
            }
        } else {
            deduped_indexes.get(&exact_hash).and_then(|indexes| {
                indexes.iter().find_map(|dedupe_index| {
                    let index = deduped_indexes.resolve(dedupe_index.index);
                    let existing = &deduped[index];
                    (existing.message_id() == Some(message_id)).then_some(index)
                })
            })
        };
        let replayed = |deduped_indexes: &DedupeIndexes, deduped: &[E]| {
            // NOTE(2026-05-25, #913): Claude Code /btw sidechains can replay parent usage with the same message ID and a new request ID. Match across request IDs to avoid overcounting (https://github.com/ccusage/ccusage/issues/913).
            let candidate_is_sidechain = entry.is_sidechain();
            let route_hash = if candidate_is_sidechain {
                sidechain_replay_dedupe_hash(message_id, entry.session_id())
            } else {
                sidechain_entry_replay_dedupe_hash(message_id, entry.session_id())
            };
            deduped_indexes.get(&route_hash).and_then(|indexes| {
                indexes.iter().find_map(|dedupe_index| {
                    let index = deduped_indexes.resolve(dedupe_index.index);
                    let existing = &deduped[index];
                    let indexed_session_id = dedupe_index
                        .session_alias
                        .as_deref()
                        .unwrap_or(existing.session_id());
                    (existing.message_id() == Some(message_id)
                        && indexed_session_id == entry.session_id()
                        && (candidate_is_sidechain
                            || existing.is_sidechain()
                            || absorbs_unclaimed_replay(&entry, existing)))
                    .then_some(index)
                })
            })
        };
        let existing_index = match existing_index {
            // NOTE(2026-11-20, #2255): A record can match its slot through a copy in another session while a sidechain replay of it in its own session still survives in a slot of its own. A record that is not itself a replay absorbs that slot, as the replay route would have.
            Some(index) if !entry.is_sidechain() => Some(
                match replayed(deduped_indexes, deduped).filter(|&replay| replay != index) {
                    Some(replay) => merge_slots(deduped_indexes, deduped, index, replay),
                    None => index,
                },
            ),
            Some(index) => Some(index),
            None => replayed(deduped_indexes, deduped),
        };
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
        for hash in requestless_hashes.into_iter().flatten() {
            push_deduped_index(deduped_indexes, hash, index);
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
        if let Some(response_hash) = requestless_response_hash(&deduped[index]) {
            push_new_deduped_index(deduped_indexes, response_hash, index);
        }
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

// NOTE(2026-11-20, #2255): A slot can hold records at several timestamps, of several transcript responses and of several sessions, for example blocks of one response, a same-timestamp rewrite, or a sidechain replay absorbed by a request-bearing parent that a copy in another session later replaced. Each requestless record stays findable by its own timestamp and response key, whose hashes already name its message and session, so a match is checked against the bucket rather than the survivor, and an explicit request ID likewise stays findable once a requestless record wins its slot. A record matching two slots by those keys connects them, so the slots merge.
fn requestless_hashes(entry: &impl DedupEntry) -> [Option<u64>; 2] {
    if entry.request_id().is_some() {
        return [None, None];
    }
    [entry_exact_hash(entry), requestless_response_hash(entry)]
}

// NOTE(2026-11-20, #2255): A requestless record at a replay's timestamp can win the replay's slot before any parent absorbs the replay. The slot then still waits for its parent, so a request-bearing record may absorb it although the survivor is no longer a sidechain record.
fn absorbs_unclaimed_replay(candidate: &impl DedupEntry, existing: &impl DedupEntry) -> bool {
    candidate.request_id().is_some() && existing.request_id().is_none()
}

fn merge_slots<E: DedupEntry>(
    deduped_indexes: &mut DedupeIndexes,
    deduped: &mut [E],
    kept: usize,
    merged: usize,
) -> usize {
    if should_replace_deduped_entry(&deduped[merged], &deduped[kept]) {
        deduped.swap(kept, merged);
    }
    let forwarded = &deduped[merged];
    if !forwarded.is_sidechain() && forwarded.session_id() != deduped[kept].session_id() {
        deduped_indexes
            .copies
            .push((kept, forwarded.owned_session_id()));
    }
    deduped_indexes.merged.insert(merged, kept);
    kept
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

#[cfg(test)]
mod tests;
