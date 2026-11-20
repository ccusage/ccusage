use ccusage_adapter_common::{DiscoverFiles, DiscoveredFile, read_discovered_files};
use ccusage_core::{Diagnostics, PricingMap};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    hash::{Hash, Hasher},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use compact_str::CompactString;
use rustc_hash::FxHasher;
use smallvec::SmallVec;

use crate::{
    CodexGroup, CodexServiceTier, CodexTokenUsageEvent, CodexUsageBucket, Result,
    cli::{AgentReportKind, SharedArgs, WeekDay},
    fast::FxHashMap,
    merge_codex_service_tiers,
};

use super::{
    parser, paths,
    replay::{CodexReplayFile, CodexReplayPlan, CodexSessionMetadata},
};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct StringId(NonZeroUsize);

struct StringInterner<T> {
    ids: FxHashMap<CompactString, StringId>,
    values: Vec<T>,
}

impl<T> Default for StringInterner<T> {
    fn default() -> Self {
        Self {
            ids: FxHashMap::default(),
            values: Vec::new(),
        }
    }
}

impl<T> StringInterner<T> {
    fn intern(&mut self, value: &str, create: impl FnOnce() -> T) -> Result<StringId> {
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }
        let id = StringId(
            self.values
                .len()
                .checked_add(1)
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| crate::cli_error("too many interned Codex strings"))?,
        );
        self.values.push(create());
        self.ids.insert(CompactString::new(value), id);
        Ok(id)
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct CodexUsageKey {
    timestamp: crate::TimestampMs,
    model: StringId,
    usage: crate::CodexRawUsage,
    occurrence: u32,
}

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

struct CodexEventProvenance {
    file_index: usize,
    occurrences: FxHashMap<CodexUsageKey, CodexFileOccurrences>,
    models: FxHashMap<CompactString, StringId>,
    session: Option<StringId>,
}

impl CodexEventProvenance {
    fn new(file_index: usize) -> Self {
        Self {
            file_index,
            occurrences: FxHashMap::default(),
            models: FxHashMap::default(),
            session: None,
        }
    }
}

struct CodexAttribution {
    file_index: usize,
    rollout_count: u32,
    session: StringId,
    last_activity: CompactString,
    is_fallback: bool,
    service_tier: Option<CodexServiceTier>,
}

impl CodexAttribution {
    fn new(event: &CodexTokenUsageEvent, file_index: usize, session: StringId) -> Self {
        Self {
            file_index,
            rollout_count: 0,
            session,
            last_activity: CompactString::new(&event.last_activity),
            is_fallback: event.is_fallback_model,
            service_tier: event.service_tier,
        }
    }

    fn merge(&mut self, mut incoming: Self) -> bool {
        self.service_tier = merge_codex_service_tiers(self.service_tier, incoming.service_tier);
        self.rollout_count = self.rollout_count.max(incoming.rollout_count);
        if incoming.file_index >= self.file_index {
            return false;
        }
        incoming.service_tier = self.service_tier;
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
        kind: AgentReportKind,
    ) -> impl Iterator<Item = (CodexUsageKey, CodexAttribution)> {
        let Self {
            mut usage,
            responses,
            recorded,
            mut other_sessions,
        } = self;
        for (&signature, requests) in &recorded {
            let mut max_in_rollout = requests
                .iter()
                .map(|request| request.attribution.rollout_count)
                .max()
                .unwrap_or_default();
            let mut stand_in = signature;
            while let Some(item) = usage.get(&stand_in) {
                max_in_rollout = max_in_rollout.max(item.rollout_count);
                if let Some(sessions) = other_sessions.get(&stand_in) {
                    for session in sessions {
                        max_in_rollout = max_in_rollout.max(session.rollout_count);
                    }
                }
                stand_in.occurrence += 1;
            }
            let named = u32::try_from(requests.len()).unwrap_or(u32::MAX);
            let recordless = stand_in.occurrence;
            let unmatched = max_in_rollout.saturating_sub(named);
            let paired = recordless.saturating_sub(unmatched);
            for occurrence in 0..paired {
                stand_in.occurrence = occurrence;
                usage.remove(&stand_in);
                other_sessions.remove(&stand_in);
            }
        }
        if kind != AgentReportKind::Session {
            for (key, sessions) in other_sessions.drain() {
                if let Some(primary) = usage.get_mut(&key) {
                    for session in sessions {
                        primary.merge(session);
                    }
                }
            }
        }
        let legacy_sessions = other_sessions.into_iter().flat_map(|(key, sessions)| {
            sessions
                .into_iter()
                .map(move |attribution| (key, attribution))
        });
        let named = recorded.into_iter().flat_map(|(signature, requests)| {
            requests
                .into_iter()
                .map(move |request| (signature, request.attribution))
        });
        usage.into_iter().chain(legacy_sessions).chain(named).chain(
            responses
                .into_values()
                .map(|record| (record.key, record.attribution)),
        )
    }
}

#[derive(Default)]
struct CodexModels {
    logged: FxHashMap<CompactString, StringId>,
    resolved: StringInterner<CodexModel>,
}

impl CodexModels {
    fn intern(&mut self, logged: &str, pricing: Option<&PricingMap>) -> Result<StringId> {
        if let Some(id) = self.logged.get(logged) {
            return Ok(*id);
        }

        let model = crate::model_aliases::resolve_model_name(logged);
        let id = self.resolved.intern(&model, || {
            let price = pricing.and_then(|table| table.find_handle(&model));
            CodexModel {
                name: CompactString::new(model.as_ref()),
                long_context_threshold: price.as_ref().map_or(
                    crate::pricing::DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS,
                    |handle| {
                        handle.with_rates_at(None, |entry| entry.long_context_split_threshold())
                    },
                ),
                time_dependent: price.is_some_and(|handle| handle.is_time_dependent()),
            }
        })?;
        self.logged.insert(CompactString::new(logged), id);
        Ok(id)
    }
}

struct CodexDedupe<'a> {
    pricing: Option<&'a PricingMap>,
    shards: Vec<Mutex<CodexDedupeMap>>,
    models: Mutex<CodexModels>,
    sessions: Mutex<StringInterner<CompactString>>,
}

impl<'a> CodexDedupe<'a> {
    fn new(files: usize, pricing: Option<&'a PricingMap>) -> Self {
        let count = ccusage_adapter_common::worker_count(false, files);
        Self {
            pricing,
            shards: (0..count)
                .map(|_| Mutex::new(CodexDedupeMap::default()))
                .collect(),
            models: Mutex::new(CodexModels::default()),
            sessions: Mutex::new(StringInterner::default()),
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

    fn record(
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
        let mut attribution = CodexAttribution::new(event, provenance.file_index, session);
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

    fn fold(
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
        let window = ccusage_core::ReportWindow::from_shared(shared);
        let mut groups = BTreeMap::new();
        for shard in self.shards {
            let records = shard
                .into_inner()
                .map_err(|_| crate::cli_error("codex dedupe poisoned"))?;
            for (key, attribution) in records.into_usage(kind) {
                let model = models
                    .resolved
                    .values
                    .get(key.model.0.get() - 1)
                    .ok_or_else(|| crate::cli_error("missing interned Codex model"))?;
                let session = sessions.values[attribution.session.0.get() - 1].as_str();
                let Some(period) = codex_period_for(key.timestamp, Some(session), kind, &window)
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
        Ok(groups)
    }
}

struct CodexModel {
    name: CompactString,
    long_context_threshold: u64,
    time_dependent: bool,
}

struct CodexAggregateRun<'a> {
    sessions_dir: &'a Path,
    files: &'a [PathBuf],
    shared: &'a SharedArgs,
    replay: &'a CodexReplayPlan,
    file_offset: usize,
    diagnostics: &'a Diagnostics,
}

pub(super) fn load_groups_with_diagnostics(
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    load_groups_from_sources_with_diagnostics(
        &paths::codex_usage_sources()?,
        shared,
        kind,
        pricing,
        diagnostics,
    )
}

pub(super) fn load_groups_from_sources_with_diagnostics(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let dirs = sources
        .iter()
        .map(paths::CodexUsageSource::dir)
        .collect::<Vec<_>>();
    let mut discover = |emit: &mut dyn FnMut(usize, PathBuf)| {
        paths::visit_deduped_codex_usage_files(sources, diagnostics, emit);
    };
    aggregate_discovered(&dirs, shared, kind, pricing, diagnostics, &mut discover)
}

pub(super) fn select_session_files(
    file_groups: &[paths::CodexUsageFileGroup],
    requested: &str,
    logical_only: bool,
) -> Vec<Vec<PathBuf>> {
    let requested = super::normalize_session_id(requested);
    let identity = |path: &Path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .and_then(super::rollout_identity)
            .map(|(thread, rollout)| (thread == requested, thread != rollout))
    };
    let logical_session = logical_only
        || file_groups
            .iter()
            .flat_map(|group| &group.files)
            .any(|path| identity(path) == Some((true, true)));
    file_groups
        .iter()
        .map(|group| {
            group
                .files
                .iter()
                .filter(|path| {
                    identity(path).is_some_and(|(owned, _)| owned)
                        || (!logical_session
                            && super::session_id_matches(
                                &parser::codex_session_id(&group.dir, path),
                                requested,
                            ))
                })
                .cloned()
                .collect()
        })
        .collect()
}

pub(super) fn load_groups_from_file_selection(
    file_groups: &[paths::CodexUsageFileGroup],
    files_by_group: &[Vec<PathBuf>],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let all_files = file_groups
        .iter()
        .map(|group| (group.dir.as_path(), group.files.as_slice()))
        .collect::<Vec<_>>();
    let selected_files = file_groups
        .iter()
        .zip(files_by_group)
        .map(|(group, files)| (group.dir.as_path(), files.as_slice()))
        .collect::<Vec<_>>();
    aggregate_file_groups(
        &selected_files,
        &all_files,
        shared,
        kind,
        pricing,
        diagnostics,
    )
}

fn aggregate_file_groups(
    selected: &[(&Path, &[PathBuf])],
    available: &[(&Path, &[PathBuf])],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let filtered = selected
        .iter()
        .zip(available)
        .any(|((_, files), (_, all_files))| files.len() != all_files.len());
    if !filtered && !has_date_bounds(shared) {
        let dirs = selected.iter().map(|(dir, _)| *dir).collect::<Vec<_>>();
        return aggregate_discovered(&dirs, shared, kind, pricing, diagnostics, &mut |emit| {
            for (group, (_, files)) in selected.iter().enumerate() {
                for file in *files {
                    emit(group, file.clone());
                }
            }
        })
        .map(|(groups, _)| groups);
    }

    let replay = CodexReplayPlan::for_selection(
        selected,
        available.iter().copied(),
        shared.single_thread,
        diagnostics,
    );
    aggregate_planned(selected, shared, kind, pricing, diagnostics, &replay)
}

fn aggregate_planned(
    selected: &[(&Path, &[PathBuf])],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
    replay: &CodexReplayPlan,
) -> Result<BTreeMap<String, CodexGroup>> {
    let seen = CodexDedupe::new(selected.iter().map(|(_, files)| files.len()).sum(), pricing);
    let mut file_offset = 0;
    for &(sessions_dir, files) in selected {
        let run = CodexAggregateRun {
            sessions_dir,
            files,
            shared,
            replay,
            file_offset,
            diagnostics,
        };
        aggregate_files_with_dedupe(&run, &seen)?;
        file_offset += files.len();
    }
    seen.fold(shared, kind)
}

fn aggregate_discovered(
    dirs: &[&Path],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
    discover: &mut DiscoverFiles<'_>,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let seen = CodexDedupe::new(usize::MAX, pricing);
    let eligibility = paths::CodexFileEligibility::from_shared(shared);
    let mut loaded = read_discovered_files(shared.single_thread, discover, |file| {
        let dir = dirs[file.group];
        eligibility
            .is_none_or(|filter| filter.contains_file(dir, &file.path))
            .then(|| fold_rollout(&seen, None, dir, &file.path, file.index, diagnostics))
            .transpose()
    });
    let detected = !loaded.is_empty();
    let has_forks = loaded.iter().any(|(_, result)| {
        result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(Option::as_ref)
            .is_some_and(CodexSessionMetadata::is_fork)
    });
    if has_forks {
        loaded.sort_unstable_by_key(|(file, _)| file.index);
        let bounded = has_date_bounds(shared);
        let mut available = vec![Vec::new(); dirs.len()];
        let mut probes = Vec::new();
        let mut replay_files = Vec::new();
        let mut forks = Vec::new();
        for (file, result) in loaded {
            let metadata = result?;
            let dir = dirs[file.group];
            if metadata
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(CodexSessionMetadata::is_fork)
            {
                forks.push(DiscoveredFile {
                    path: file.path.clone(),
                    ..file
                });
            }
            if bounded {
                if let Some(metadata) = metadata {
                    probes.push((file.path.clone(), dir, metadata));
                }
                available[file.group].push(file.path);
            } else if let Some(metadata) = metadata {
                replay_files.push(CodexReplayFile {
                    path: file.path,
                    sessions_dir: dir,
                    metadata: metadata.unwrap_or_default(),
                });
            }
        }
        let replay = if bounded {
            CodexReplayPlan::for_probes(
                probes,
                dirs.iter()
                    .copied()
                    .zip(available.iter().map(Vec::as_slice)),
                shared.single_thread,
                diagnostics,
            )
        } else {
            CodexReplayPlan::new(replay_files, None, shared.single_thread, diagnostics)
        };
        ccusage_adapter_common::map_parallel(
            &forks,
            shared.single_thread,
            ccusage_adapter_common::Split::BySize(|file| &file.path),
            |file| {
                fold_rollout(
                    &seen,
                    Some(&replay),
                    dirs[file.group],
                    &file.path,
                    file.index,
                    diagnostics,
                )
                .map(|_| ())
            },
        )
        .into_iter()
        .collect::<Result<()>>()?;
    } else {
        for (_, result) in loaded {
            result?;
        }
    }
    seen.fold(shared, kind).map(|groups| (groups, detected))
}

fn aggregate_files_with_dedupe(run: &CodexAggregateRun<'_>, seen: &CodexDedupe<'_>) -> Result<()> {
    let workers = ccusage_adapter_common::worker_count(run.shared.single_thread, run.files.len());
    let chunks = if workers == 1 {
        vec![(0..run.files.len()).collect()]
    } else {
        crate::chunk_file_indexes_by_size(run.files, workers)
    };
    ccusage_adapter_common::map_parallel(
        &chunks,
        run.shared.single_thread,
        ccusage_adapter_common::Split::ByCount,
        |chunk| {
            for &index in chunk {
                fold_rollout(
                    seen,
                    Some(run.replay),
                    run.sessions_dir,
                    &run.files[index],
                    run.file_offset + index,
                    run.diagnostics,
                )?;
            }
            Ok(())
        },
    )
    .into_iter()
    .collect()
}

fn fold_rollout(
    seen: &CodexDedupe<'_>,
    replay: Option<&CodexReplayPlan>,
    sessions_dir: &Path,
    path: &Path,
    index: usize,
    diagnostics: &Diagnostics,
) -> Result<Option<CodexSessionMetadata>> {
    let mut provenance = CodexEventProvenance::new(index);
    let mut result = Ok(());
    let mut visit = |event: CodexTokenUsageEvent| {
        if result.is_ok()
            && !replay.is_some_and(|replay| {
                replay.is_replayed_compaction(path, event.response_id.as_deref())
            })
        {
            result = seen.record(&event, &mut provenance);
        }
    };
    let metadata = if let Some(replay) = replay {
        parser::visit_codex_session_file_with_diagnostics(
            sessions_dir,
            path,
            replay.fork_replay(path),
            &mut visit,
            diagnostics,
        );
        None
    } else {
        parser::visit_codex_session_file_unless_fork(sessions_dir, path, &mut visit, diagnostics)
    };
    result.map(|()| metadata)
}

fn codex_period_for(
    timestamp: crate::TimestampMs,
    session_id: Option<&str>,
    kind: AgentReportKind,
    window: &ccusage_core::ReportWindow,
) -> Option<String> {
    let date = ccusage_core::date_at(timestamp, window.timezone());
    if !window.contains_day(date) {
        return None;
    }
    Some(match kind {
        AgentReportKind::Daily => date.to_string(),
        AgentReportKind::Weekly => date.week_start(WeekDay::Monday).to_string(),
        AgentReportKind::Monthly => date.format_month(),
        AgentReportKind::Session => session_id?.to_string(),
    })
}

fn accumulate_codex_usage_into_group(
    group: &mut CodexGroup,
    event: crate::CodexRawUsage,
    model: &CodexModel,
    timestamp: crate::TimestampMs,
    attribution: &CodexAttribution,
) {
    group.input_tokens = group.input_tokens.saturating_add(event.input_tokens);
    group.cached_input_tokens = group
        .cached_input_tokens
        .saturating_add(event.cached_input_tokens);
    group.cache_creation_tokens = group
        .cache_creation_tokens
        .saturating_add(event.cache_creation_tokens);
    group.output_tokens = group.output_tokens.saturating_add(event.output_tokens);
    group.reasoning_output_tokens = group
        .reasoning_output_tokens
        .saturating_add(event.reasoning_output_tokens);
    group.total_tokens = group.total_tokens.saturating_add(event.total_tokens);
    if group
        .last_activity
        .as_deref()
        .is_none_or(|current| attribution.last_activity.as_str() > current)
    {
        group.last_activity = Some(attribution.last_activity.to_string());
    }

    if let Some(model_usage) = group.models.get_mut(model.name.as_str()) {
        accumulate_codex_usage_into_model_usage(model_usage, event, model, timestamp, attribution);
    } else {
        accumulate_codex_usage_into_model_usage(
            group.models.entry(model.name.to_string()).or_default(),
            event,
            model,
            timestamp,
            attribution,
        );
    }
}

fn accumulate_codex_usage_into_model_usage(
    model_usage: &mut crate::CodexModelUsage,
    event: crate::CodexRawUsage,
    model: &CodexModel,
    timestamp: crate::TimestampMs,
    attribution: &CodexAttribution,
) {
    model_usage.reasoning_output_tokens = model_usage
        .reasoning_output_tokens
        .saturating_add(event.reasoning_output_tokens);
    model_usage.total_tokens = model_usage.total_tokens.saturating_add(event.total_tokens);
    // NOTE(2026-07-09, #1414): OpenAI applies long-context rates to every token in a request once its input exceeds that model's threshold. The split has to happen before requests are summed.
    let is_long_context = event.input_tokens > model.long_context_threshold;
    let usage = CodexUsageBucket::from_usage(event, is_long_context);
    model_usage.add_usage(usage, attribution.service_tier);
    if model.time_dependent {
        let timestamped_usage = model_usage
            .timestamped_usage
            .entry(timestamp.as_millis())
            .or_default();
        timestamped_usage.usage += usage;
        if let Some(tier) = attribution.service_tier {
            timestamped_usage.recorded_usage[tier] += usage;
        }
    }
    model_usage.is_fallback |= attribution.is_fallback;
}

fn has_date_bounds(shared: &SharedArgs) -> bool {
    shared.since.is_some() || shared.until.is_some()
}

#[cfg(test)]
mod tests;
