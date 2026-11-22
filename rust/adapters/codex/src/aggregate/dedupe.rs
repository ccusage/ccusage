use ccusage_core::PricingMap;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};

use compact_str::CompactString;
use rustc_hash::FxHasher;
use smallvec::SmallVec;

use crate::{
    CodexGroup, CodexServiceTier, CodexTokenUsageEvent, Result,
    cli::{AgentReportKind, SharedArgs},
    fast::FxHashMap,
    merge_codex_service_tiers,
};

use super::{
    CodexSessionMetadata,
    accumulate::{CodexModels, CodexPeriod, accumulate_codex_usage_into_group, codex_period_for},
    intern::{StringId, StringInterner},
};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct CodexUsageKey {
    timestamp: crate::TimestampMs,
    model: StringId,
    usage: crate::CodexRawUsage,
    occurrence: u32,
}

const _: () = assert!(size_of::<CodexUsageKey>() == 64);

#[derive(Default)]
struct CodexFileOccurrences {
    total: u32,
    recordless: u32,
}

impl CodexFileOccurrences {
    fn add(&mut self, recorded: bool) -> (u32, u32) {
        let occurrence = self.recordless;
        self.total += 1;
        self.recordless += u32::from(!recorded);
        (occurrence, self.total)
    }
}

pub(super) struct CodexEventProvenance {
    file_index: u32,
    configured_tier: Option<CodexServiceTier>,
    occurrences: FxHashMap<CodexUsageKey, CodexFileOccurrences>,
    models: FxHashMap<CompactString, StringId>,
    session: Option<StringId>,
}

impl CodexEventProvenance {
    pub(super) fn new(file_index: usize, configured_tier: Option<CodexServiceTier>) -> Self {
        Self {
            file_index: u32::try_from(file_index).unwrap_or(u32::MAX),
            configured_tier,
            occurrences: FxHashMap::default(),
            models: FxHashMap::default(),
            session: None,
        }
    }
}

pub(super) struct CodexAttribution {
    file_index: u32,
    rollout_count: u32,
    session: StringId,
    pub(super) last_activity: CompactString,
    pub(super) is_fallback: bool,
    service_tier: Option<CodexServiceTier>,
    configured_tier: Option<CodexServiceTier>,
}

impl CodexAttribution {
    fn new(
        event: &CodexTokenUsageEvent,
        provenance: &CodexEventProvenance,
        session: StringId,
    ) -> Self {
        Self {
            file_index: provenance.file_index,
            rollout_count: 0,
            session,
            last_activity: CompactString::new(&event.last_activity),
            is_fallback: event.is_fallback_model,
            service_tier: event.service_tier,
            configured_tier: provenance.configured_tier,
        }
    }

    pub(super) fn pricing_tier(&self) -> Option<CodexServiceTier> {
        self.service_tier.or(self.configured_tier)
    }

    fn merge(&mut self, mut incoming: Self) -> bool {
        self.service_tier = merge_codex_service_tiers(self.service_tier, incoming.service_tier);
        self.configured_tier =
            merge_codex_service_tiers(self.configured_tier, incoming.configured_tier);
        self.rollout_count = self.rollout_count.max(incoming.rollout_count);
        if incoming.file_index >= self.file_index {
            return false;
        }
        incoming.service_tier = self.service_tier;
        incoming.configured_tier = self.configured_tier;
        incoming.rollout_count = self.rollout_count;
        *self = incoming;
        true
    }
}

struct CodexResponseUsage {
    key: CodexUsageKey,
    attribution: CodexAttribution,
}

struct CodexRecordedUsage {
    id: Arc<str>,
    attribution: CodexAttribution,
}

const _: () = assert!(size_of::<CodexAttribution>() == 40);
const _: () = assert!(size_of::<(CodexUsageKey, SmallVec<[CodexRecordedUsage; 1]>)>() == 128);

#[derive(Default)]
struct CodexLineage {
    threads: FxHashMap<StringId, CompactString>,
    parents: FxHashMap<CompactString, CompactString>,
}

impl CodexLineage {
    fn add_parent(&mut self, thread: &str, parent: &str) {
        self.parents
            .entry(CompactString::new(thread))
            .or_insert_with(|| CompactString::new(parent));
    }

    fn ancestors(&self, session: StringId) -> Vec<&CompactString> {
        let mut ancestors = Vec::new();
        let mut thread = self.threads.get(&session);
        while let Some(parent) = thread.and_then(|thread| self.parents.get(thread)) {
            if ancestors.contains(&parent) {
                break;
            }
            ancestors.push(parent);
            thread = Some(parent);
        }
        ancestors
    }

    // NOTE(2026-11-20, #2208): A fork and its ancestors each send their own requests after the fork, while unrelated sessions with the same recordless usage hold copies of one request. Matches at the same fork depth never descend from one another, so each depth counts once.
    fn fork_depths(&self, sessions: &[StringId], related: &[StringId]) -> Vec<usize> {
        let threads = sessions
            .iter()
            .chain(related)
            .filter_map(|session| self.threads.get(session))
            .collect::<Vec<_>>();
        sessions
            .iter()
            .map(|session| {
                self.ancestors(*session)
                    .into_iter()
                    .filter(|ancestor| threads.contains(ancestor))
                    .count()
            })
            .collect()
    }

    fn collapse_copies(
        &self,
        attributions: Vec<CodexAttribution>,
        related: &[StringId],
    ) -> Vec<CodexAttribution> {
        let sessions = attributions
            .iter()
            .map(|attribution| attribution.session)
            .collect::<Vec<_>>();
        let depths = self.fork_depths(&sessions, related);
        let mut requests = Vec::<(usize, CodexAttribution)>::new();
        for (depth, attribution) in depths.into_iter().zip(attributions) {
            if let Some((_, request)) = requests.iter_mut().find(|(level, _)| *level == depth) {
                request.merge(attribution);
            } else {
                requests.push((depth, attribution));
            }
        }
        requests.into_iter().map(|(_, request)| request).collect()
    }
}

#[derive(Default)]
struct CodexDedupeMap {
    usage: FxHashMap<CodexUsageKey, CodexAttribution>,
    responses: FxHashMap<CompactString, CodexResponseUsage>,
    recorded: FxHashMap<CodexUsageKey, SmallVec<[CodexRecordedUsage; 1]>>,
    other_sessions: FxHashMap<CodexUsageKey, Vec<CodexAttribution>>,
}

impl CodexDedupeMap {
    fn record_legacy(&mut self, key: CodexUsageKey, attribution: CodexAttribution) {
        let Some(existing) = self.usage.get_mut(&key) else {
            self.usage.insert(key, attribution);
            return;
        };
        if existing.session == attribution.session {
            existing.merge(attribution);
        } else {
            let sessions = self.other_sessions.entry(key).or_default();
            if let Some(session) = sessions
                .iter_mut()
                .find(|session| session.session == attribution.session)
            {
                session.merge(attribution);
            } else {
                sessions.push(attribution);
            }
        }
    }

    fn into_usage(
        self,
        lineage: &CodexLineage,
    ) -> impl Iterator<Item = (CodexUsageKey, CodexAttribution)> {
        let Self {
            mut usage,
            responses,
            recorded,
            mut other_sessions,
        } = self;
        let mut pairing_sessions = FxHashMap::<CodexUsageKey, Vec<StringId>>::default();
        // NOTE(2026-11-20, #2366): A recordless occurrence can only be a copy of a named request at the same fork depth. A fork's own request that shares a signature with an ancestor's recorded request is a separate request.
        for (&signature, requests) in &recorded {
            let mut recordless = Vec::new();
            let mut stand_in = signature;
            while let Some(item) = usage.get(&stand_in) {
                let sessions = other_sessions.get(&stand_in).into_iter().flatten();
                for attribution in std::iter::once(item).chain(sessions) {
                    recordless.push((
                        stand_in.occurrence,
                        attribution.session,
                        attribution.rollout_count,
                    ));
                }
                stand_in.occurrence += 1;
            }
            if recordless.is_empty() {
                continue;
            }
            let sessions = requests
                .iter()
                .map(|request| request.attribution.session)
                .chain(recordless.iter().map(|(_, session, _)| *session))
                .collect::<Vec<_>>();
            let depths = lineage.fork_depths(&sessions, &[]);
            let (named_depths, recordless_depths) = depths.split_at(requests.len());
            let depth_of = recordless
                .iter()
                .zip(recordless_depths)
                .map(|((_, session, _), depth)| (*session, *depth))
                .collect::<FxHashMap<_, _>>();
            let mut levels = recordless_depths.to_vec();
            levels.sort_unstable();
            levels.dedup();
            for depth in levels {
                let named = named_depths.iter().filter(|level| **level == depth).count();
                if named == 0 {
                    continue;
                }
                let at_depth = recordless
                    .iter()
                    .zip(recordless_depths)
                    .filter(|(_, level)| **level == depth)
                    .map(|(occurrence, _)| *occurrence);
                let recordless_count = at_depth
                    .clone()
                    .map(|(occurrence, _, _)| occurrence + 1)
                    .max()
                    .unwrap_or_default();
                let max_in_rollout = requests
                    .iter()
                    .zip(named_depths)
                    .filter(|(_, level)| **level == depth)
                    .map(|(request, _)| request.attribution.rollout_count)
                    .chain(at_depth.map(|(_, _, rollout_count)| rollout_count))
                    .max()
                    .unwrap_or_default();
                let named = u32::try_from(named).unwrap_or(u32::MAX);
                let unmatched = max_in_rollout.saturating_sub(named);
                let paired = recordless_count.saturating_sub(unmatched);
                let copy = |attribution: &CodexAttribution| {
                    depth_of.get(&attribution.session) == Some(&depth)
                };
                for occurrence in 0..paired {
                    stand_in.occurrence = occurrence;
                    if usage.get(&stand_in).is_some_and(copy) {
                        usage.remove(&stand_in);
                    }
                    if let Some(sessions) = other_sessions.get_mut(&stand_in) {
                        sessions.retain(|attribution| !copy(attribution));
                        if sessions.is_empty() {
                            other_sessions.remove(&stand_in);
                        }
                    }
                }
            }
            pairing_sessions.insert(signature, sessions);
        }
        let mut recordless_matches = Vec::new();
        for (key, sessions) in other_sessions {
            let matches = usage.remove(&key).into_iter().chain(sessions).collect();
            // NOTE(2026-11-20, #2366): Settlement measures fork depth against every session that matched the signature during pairing. Paired copies are already removed, so their threads would otherwise drop out and change a fork's depth.
            let related = pairing_sessions
                .get(&CodexUsageKey {
                    occurrence: 0,
                    ..key
                })
                .map_or(&[][..], Vec::as_slice);
            recordless_matches.extend(
                lineage
                    .collapse_copies(matches, related)
                    .into_iter()
                    .map(|attribution| (key, attribution)),
            );
        }
        let named = recorded.into_iter().flat_map(|(signature, requests)| {
            requests
                .into_iter()
                .map(move |request| (signature, request.attribution))
        });
        usage
            .into_iter()
            .chain(recordless_matches)
            .chain(named)
            .chain(
                responses
                    .into_values()
                    .map(|record| (record.key, record.attribution)),
            )
    }
}

pub(super) struct CodexDedupe<'a> {
    pricing: Option<&'a PricingMap>,
    shards: Vec<Mutex<CodexDedupeMap>>,
    models: Mutex<CodexModels>,
    sessions: Mutex<StringInterner<CompactString>>,
    lineage: Mutex<CodexLineage>,
}

impl<'a> CodexDedupe<'a> {
    pub(super) fn new(files: usize, pricing: Option<&'a PricingMap>) -> Self {
        let count = ccusage_adapter_common::worker_count(false, files);
        Self {
            pricing,
            shards: (0..count)
                .map(|_| Mutex::new(CodexDedupeMap::default()))
                .collect(),
            models: Mutex::new(CodexModels::default()),
            sessions: Mutex::new(StringInterner::default()),
            lineage: Mutex::new(CodexLineage::default()),
        }
    }

    fn shard(&self, key: &impl Hash) -> Result<&Mutex<CodexDedupeMap>> {
        let mut hasher = FxHasher::default();
        key.hash(&mut hasher);
        let count = u64::try_from(self.shards.len())
            .map_err(|error| crate::cli_error(error.to_string()))?;
        let index = usize::try_from(hasher.finish() % count)
            .map_err(|error| crate::cli_error(error.to_string()))?;
        Ok(&self.shards[index])
    }

    pub(super) fn record(
        &self,
        event: &CodexTokenUsageEvent,
        provenance: &mut CodexEventProvenance,
    ) -> Result<()> {
        let session = if let Some(session) = provenance.session {
            session
        } else {
            let session = self
                .sessions
                .lock()
                .map_err(|_| crate::cli_error("codex interner poisoned"))?
                .intern(&event.session_id, || CompactString::new(&event.session_id))?;
            provenance.session = Some(session);
            session
        };
        let mut key = CodexUsageKey {
            timestamp: event.timestamp,
            model: self.intern_model(provenance, &event.model)?,
            usage: event.raw_usage(),
            occurrence: 0,
        };
        let mut attribution = CodexAttribution::new(event, provenance, session);
        if let Some(response) = &event.response_id {
            let mut shard = self
                .shard(response)?
                .lock()
                .map_err(|_| crate::cli_error("codex dedupe poisoned"))?;
            if let Some(record) = shard.responses.get_mut(response.as_str()) {
                if record.attribution.merge(attribution) {
                    record.key = key;
                }
            } else {
                shard.responses.insert(
                    CompactString::new(response),
                    CodexResponseUsage { key, attribution },
                );
            }
        } else {
            let shard = self.shard(&key)?;
            let recorded = event.record_response_id.is_some();
            let (occurrence, rollout_count) =
                provenance.occurrences.entry(key).or_default().add(recorded);
            attribution.rollout_count = rollout_count;
            if !recorded {
                key.occurrence = occurrence;
            }
            let mut shard = shard
                .lock()
                .map_err(|_| crate::cli_error("codex dedupe poisoned"))?;
            if let Some(id) = &event.record_response_id {
                let requests = shard.recorded.entry(key).or_default();
                if let Some(request) = requests.iter_mut().find(|request| request.id == *id) {
                    request.attribution.merge(attribution);
                } else {
                    requests.push(CodexRecordedUsage {
                        id: Arc::clone(id),
                        attribution,
                    });
                }
            } else {
                shard.record_legacy(key, attribution);
            }
        }
        Ok(())
    }

    pub(super) fn record_lineage(
        &self,
        provenance: &CodexEventProvenance,
        metadata: &CodexSessionMetadata,
    ) -> Result<()> {
        let Some(thread) = metadata.session_id() else {
            return Ok(());
        };
        let mut lineage = self
            .lineage
            .lock()
            .map_err(|_| crate::cli_error("codex lineage poisoned"))?;
        if let Some(session) = provenance.session {
            lineage.threads.insert(session, CompactString::new(thread));
        }
        if let Some(parent) = metadata.lineage_parent_id() {
            lineage.add_parent(thread, parent);
        }
        Ok(())
    }

    pub(super) fn record_lineage_edges(&self, edges: &[(String, String)]) -> Result<()> {
        let mut lineage = self
            .lineage
            .lock()
            .map_err(|_| crate::cli_error("codex lineage poisoned"))?;
        for (thread, parent) in edges {
            lineage.add_parent(thread, parent);
        }
        Ok(())
    }

    fn intern_model(
        &self,
        provenance: &mut CodexEventProvenance,
        logged: &str,
    ) -> Result<StringId> {
        let cache = &mut provenance.models;
        if let Some(id) = cache.get(logged) {
            return Ok(*id);
        }

        let id = self
            .models
            .lock()
            .map_err(|_| crate::cli_error("codex interner poisoned"))?
            .intern(logged, self.pricing)?;
        cache.insert(CompactString::new(logged), id);
        Ok(id)
    }

    pub(super) fn fold(
        self,
        shared: &SharedArgs,
        kind: AgentReportKind,
    ) -> Result<BTreeMap<String, CodexGroup>> {
        let models = self
            .models
            .into_inner()
            .map_err(|_| crate::cli_error("codex interner poisoned"))?;
        let sessions = self
            .sessions
            .into_inner()
            .map_err(|_| crate::cli_error("codex interner poisoned"))?;
        let lineage = self
            .lineage
            .into_inner()
            .map_err(|_| crate::cli_error("codex lineage poisoned"))?;
        let window = ccusage_core::ReportWindow::from_shared(shared);
        let mut groups = FxHashMap::<CodexPeriod, CodexGroup>::default();
        for shard in self.shards {
            let records = shard
                .into_inner()
                .map_err(|_| crate::cli_error("codex dedupe poisoned"))?;
            for (key, attribution) in records.into_usage(&lineage) {
                let model = models
                    .resolved
                    .values
                    .get(key.model.index())
                    .ok_or_else(|| crate::cli_error("missing interned Codex model"))?;
                let Some(period) =
                    codex_period_for(key.timestamp, attribution.session, kind, &window)
                else {
                    continue;
                };
                accumulate_codex_usage_into_group(
                    groups.entry(period).or_default(),
                    key.usage,
                    model,
                    key.timestamp,
                    &attribution,
                );
            }
        }
        let mut labeled = BTreeMap::new();
        for (period, group) in groups {
            labeled.insert(period.label(kind, &sessions), group);
        }
        Ok(labeled)
    }
}
