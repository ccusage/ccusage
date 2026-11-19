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
use jiff::tz::TimeZone as JiffTimeZone;
use rustc_hash::FxHasher;
use smallvec::SmallVec;

use crate::{
    CodexGroup, CodexServiceTier, CodexTokenUsageEvent, CodexUsageBucket, Result,
    cli::{AgentReportKind, SharedArgs, WeekDay},
    fast::FxHashMap,
    format_date_tz, merge_codex_service_tiers, resolve_tz, week_start,
};

use super::{
    parser, paths,
    replay::{CodexReplayFile, CodexReplayPlan, read_codex_session_metadata},
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
                        id: id.clone(),
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
        let timezone = resolve_tz(shared.timezone.as_deref());
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
                let Some(period) =
                    codex_period_for(key.timestamp, Some(session), kind, &timezone, shared)
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

struct CodexRollout<'a> {
    group: usize,
    sessions_dir: &'a Path,
    path: PathBuf,
    index: usize,
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
    let discover = |emit: &mut dyn FnMut(usize, PathBuf)| {
        paths::visit_deduped_codex_usage_files(sources, diagnostics, emit);
    };
    if !has_date_bounds(shared) {
        return aggregate_discovered(&dirs, shared, kind, pricing, diagnostics, discover);
    }

    let eligibility = paths::CodexFileEligibility::from_shared(shared);
    let loaded = read_discovered_rollouts(&dirs, shared.single_thread, discover, |file| {
        eligibility
            .is_none_or(|filter| filter.contains_file(file.sessions_dir, &file.path))
            .then(|| read_codex_session_metadata(&file.path, Some(diagnostics)))
    });
    let detected = !loaded.is_empty();
    let mut available = vec![Vec::new(); dirs.len()];
    let mut selected = vec![Vec::new(); dirs.len()];
    let mut probes = Vec::new();
    for (file, probe) in loaded {
        if let Some(metadata) = probe {
            selected[file.group].push(file.path.clone());
            probes.push((file.path.clone(), file.sessions_dir, metadata));
        }
        available[file.group].push(file.path);
    }
    let replay = CodexReplayPlan::for_probes(
        probes,
        dirs.iter()
            .copied()
            .zip(available.iter().map(Vec::as_slice)),
        shared.single_thread,
        diagnostics,
    );
    let selected = dirs
        .into_iter()
        .zip(selected.iter().map(Vec::as_slice))
        .collect::<Vec<_>>();
    aggregate_planned(&selected, shared, kind, pricing, diagnostics, &replay)
        .map(|groups| (groups, detected))
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
        return aggregate_discovered(&dirs, shared, kind, pricing, diagnostics, |emit| {
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
    discover: impl FnOnce(&mut dyn FnMut(usize, PathBuf)),
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let seen = CodexDedupe::new(usize::MAX, pricing);
    let loaded = read_discovered_rollouts(
        dirs,
        shared.single_thread,
        discover,
        |file: &CodexRollout<'_>| {
            let mut provenance = CodexEventProvenance::new(file.index);
            let mut result = Ok(());
            let metadata = parser::visit_codex_session_file_unless_fork(
                file.sessions_dir,
                &file.path,
                |event| {
                    if result.is_ok() {
                        result = seen.record(&event, &mut provenance);
                    }
                },
                diagnostics,
            );
            result.map(|()| metadata)
        },
    );
    let detected = !loaded.is_empty();
    let mut replay_files = Vec::with_capacity(loaded.len());
    let mut forks = Vec::new();
    for (file, metadata) in loaded {
        let metadata = metadata?;
        if metadata.is_fork() {
            forks.push(CodexRollout {
                path: file.path.clone(),
                ..file
            });
        }
        replay_files.push(CodexReplayFile {
            path: file.path,
            sessions_dir: file.sessions_dir,
            metadata,
        });
    }
    if !forks.is_empty() {
        let replay = CodexReplayPlan::new(replay_files, None, shared.single_thread, diagnostics);
        ccusage_adapter_common::map_parallel(
            &forks,
            shared.single_thread,
            ccusage_adapter_common::Split::BySize(|file| &file.path),
            |file| {
                fold_planned_rollout(
                    &seen,
                    &replay,
                    file.sessions_dir,
                    &file.path,
                    file.index,
                    diagnostics,
                )
            },
        )
        .into_iter()
        .collect::<Result<()>>()?;
    }
    seen.fold(shared, kind).map(|groups| (groups, detected))
}

fn read_discovered_rollouts<'a, T: Send>(
    dirs: &[&'a Path],
    single_thread: bool,
    discover: impl FnOnce(&mut dyn FnMut(usize, PathBuf)),
    read: impl Fn(&CodexRollout<'a>) -> T + Sync,
) -> Vec<(CodexRollout<'a>, T)> {
    let mut loaded = ccusage_adapter_common::read_discovered_files(
        single_thread,
        |emit| {
            let mut index = 0;
            discover(&mut |group, path| {
                emit(CodexRollout {
                    group,
                    sessions_dir: dirs[group],
                    path,
                    index,
                });
                index += 1;
            });
        },
        read,
    );
    loaded.sort_unstable_by_key(|(file, _)| file.index);
    loaded
}

fn aggregate_files_with_dedupe(run: &CodexAggregateRun<'_>, seen: &CodexDedupe) -> Result<()> {
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
                fold_planned_rollout(
                    seen,
                    run.replay,
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

fn fold_planned_rollout(
    seen: &CodexDedupe<'_>,
    replay: &CodexReplayPlan,
    sessions_dir: &Path,
    path: &Path,
    index: usize,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let mut provenance = CodexEventProvenance::new(index);
    let mut result = Ok(());
    parser::visit_codex_session_file_with_diagnostics(
        sessions_dir,
        path,
        replay.fork_replay(path),
        |event| {
            if result.is_ok() && !replay.is_replayed_compaction(path, event.response_id.as_deref())
            {
                result = seen.record(&event, &mut provenance);
            }
        },
        diagnostics,
    );
    result
}

fn codex_period_for(
    timestamp: crate::TimestampMs,
    session_id: Option<&str>,
    kind: AgentReportKind,
    timezone: &JiffTimeZone,
    shared: &SharedArgs,
) -> Option<String> {
    let date = format_date_tz(timestamp, timezone);
    if shared.since.is_some() || shared.until.is_some() {
        let date_key = date.replace('-', "");
        if shared.since.as_ref().is_some_and(|since| &date_key < since)
            || shared.until.as_ref().is_some_and(|until| &date_key > until)
        {
            return None;
        }
    }
    Some(match kind {
        AgentReportKind::Daily => date,
        AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
        AgentReportKind::Monthly => date[..7].to_string(),
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
mod tests {
    use super::*;
    use crate::parse_ts_timestamp;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use crate::{
        CodexSpeedPolicy, PricingMap,
        model_aliases::set_model_aliases_for_tests,
        paths::CodexUsageSource,
        test_support::{
            directory_source, load_groups_from_directory, load_groups_from_sources, load_rollouts,
            service_tier_line, token_count_line,
        },
    };

    #[test]
    fn prices_the_configured_alias_target_when_both_models_have_rates() {
        ccusage_test_support::install_pricing_snapshots();
        let _aliases = set_model_aliases_for_tests([("logged-model", "alias-model")]);
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "logged-model":{"input_cost_per_token":1,"output_cost_per_token":1},
                "alias-model":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
        );
        let mut event = CodexTokenUsageEvent {
            session_id: "session".into(),
            response_id: None,
            record_response_id: None,
            timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
            last_activity: "2026-09-01T00:00:00Z".into(),
            model: "logged-model".into(),
            input_tokens: 100,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 100,
            is_fallback_model: false,
            service_tier: None,
        };
        let seen = CodexDedupe::new(1, Some(&pricing));
        let mut provenance = CodexEventProvenance::new(0);
        seen.record(&event, &mut provenance).unwrap();
        event.model = "alias-model".into();
        seen.record(&event, &mut provenance).unwrap();
        let groups = seen
            .fold(&SharedArgs::default(), AgentReportKind::Session)
            .unwrap();
        let group = &groups["session"];
        assert_eq!(
            group.models.keys().cloned().collect::<Vec<_>>(),
            ["alias-model"]
        );
        assert_eq!(group.input_tokens, 200);
        assert_eq!(
            crate::report::calculate_group_cost(
                group,
                Some(&pricing),
                CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
            ),
            400.0,
        );
    }

    #[test]
    fn tier_split_uses_the_supplied_pricing_entry_instead_of_embedded_rates() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"gpt-5.5":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002,"input_cost_per_token_above_200k_tokens":0.000003}}"#);
        let event = CodexTokenUsageEvent {
            session_id: "session".into(),
            response_id: None,
            record_response_id: None,
            timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
            last_activity: "2026-09-01T00:00:00Z".into(),
            model: "gpt-5.5".into(),
            input_tokens: 250_000,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 10,
            reasoning_output_tokens: 0,
            total_tokens: 250_010,
            is_fallback_model: false,
            service_tier: None,
        };
        let seen = CodexDedupe::new(1, Some(&pricing));
        seen.record(&event, &mut CodexEventProvenance::new(0))
            .unwrap();
        let groups = seen
            .fold(&SharedArgs::default(), AgentReportKind::Session)
            .unwrap();
        assert_eq!(
            groups["session"].models["gpt-5.5"]
                .usage
                .long_context_input_tokens,
            250_000
        );
    }

    #[test]
    fn saturates_usage_totals_across_distinct_requests() {
        ccusage_test_support::install_pricing_snapshots();
        let line = token_count_line(
            "2026-09-01T08:00:00Z",
            "deepseek-v4-flash",
            json!({
                "input_tokens": u64::MAX,
                "cached_input_tokens": 1,
                "cache_write_input_tokens": 1,
                "output_tokens": u64::MAX,
                "reasoning_output_tokens": 1,
                "total_tokens": u64::MAX,
            }),
        );
        let rollout = [
            service_tier_line("2026-09-01T07:59:00Z", "priority"),
            line.clone(),
            line,
        ];
        let groups = load_rollouts(&[("session.jsonl", &rollout)], AgentReportKind::Daily);
        let group = &groups["2026-09-01"];
        let usage = &group.models["deepseek-v4-flash"];
        assert_eq!(group.input_tokens, u64::MAX);
        assert_eq!(group.output_tokens, u64::MAX);
        assert_eq!(group.total_tokens, u64::MAX);
        assert_eq!(usage.usage.input_tokens, u64::MAX);
        assert_eq!(usage.usage.output_tokens, u64::MAX);
        assert_eq!(usage.total_tokens, u64::MAX);
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
            u64::MAX
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].cached_input_tokens,
            2
        );
        assert_eq!(
            usage
                .timestamped_usage
                .values()
                .next()
                .unwrap()
                .usage
                .input_tokens,
            u64::MAX
        );
    }

    #[test]
    fn saturates_each_usage_bucket_component() {
        let event = crate::CodexRawUsage {
            input_tokens: u64::MAX,
            cached_input_tokens: u64::MAX,
            cache_creation_tokens: u64::MAX,
            output_tokens: u64::MAX,
            ..crate::CodexRawUsage::default()
        };
        let mut usage = CodexUsageBucket::default();
        usage += CodexUsageBucket::from_usage(event, true);
        usage += CodexUsageBucket::from_usage(event, true);
        assert_eq!(
            usage,
            CodexUsageBucket {
                input_tokens: u64::MAX,
                cached_input_tokens: u64::MAX,
                cache_creation_tokens: u64::MAX,
                output_tokens: u64::MAX,
                long_context_input_tokens: u64::MAX,
                long_context_cached_input_tokens: u64::MAX,
                long_context_cache_creation_tokens: u64::MAX,
                long_context_output_tokens: u64::MAX,
            }
        );
    }

    #[test]
    fn usage_attribution_uses_file_order_when_workers_finish_in_reverse() {
        ccusage_test_support::install_pricing_snapshots();
        let first = CodexTokenUsageEvent {
            session_id: "session".into(),
            response_id: None,
            record_response_id: None,
            timestamp: crate::parse_ts_timestamp("2026-09-01T00:00:01Z").unwrap(),
            last_activity: "2026-09-01T00:00:01Z".into(),
            model: "gpt-5".into(),
            input_tokens: 300,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 20,
            reasoning_output_tokens: 0,
            total_tokens: 320,
            is_fallback_model: false,
            service_tier: Some(CodexServiceTier::Standard),
        };
        let mut copied = first.clone();
        copied.last_activity = "2026-09-01T00:00:01.000Z".into();
        copied.is_fallback_model = true;
        copied.service_tier = Some(CodexServiceTier::Fast);
        let shared = SharedArgs {
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_embedded();
        let seen = CodexDedupe::new(2, Some(&pricing));
        for (event, file_index) in [(&copied, 1), (&first, 0)] {
            seen.record(event, &mut CodexEventProvenance::new(file_index))
                .unwrap();
        }
        let groups = seen.fold(&shared, AgentReportKind::Session).unwrap();

        assert_eq!(groups.len(), 1);
        let group = &groups["session"];
        assert_eq!(group.total_tokens, 320);
        assert_eq!(group.last_activity.as_deref(), Some("2026-09-01T00:00:01Z"));
        assert!(!group.models["gpt-5"].is_fallback);
        assert_eq!(
            group.models["gpt-5"].recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
            300
        );
    }

    #[test]
    fn compaction_attribution_uses_file_order_when_workers_finish_in_reverse() {
        ccusage_test_support::install_pricing_snapshots();
        let first = CodexTokenUsageEvent {
            session_id: "first-session".into(),
            response_id: Some("response-1".into()),
            record_response_id: None,
            timestamp: crate::parse_ts_timestamp("2026-09-01T00:00:01Z").unwrap(),
            last_activity: "2026-09-01T00:00:01Z".into(),
            model: "gpt-5".into(),
            input_tokens: 300,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 20,
            reasoning_output_tokens: 0,
            total_tokens: 320,
            is_fallback_model: true,
            service_tier: Some(CodexServiceTier::Standard),
        };
        let mut copied = first.clone();
        copied.session_id = "copied-session".into();
        copied.timestamp = crate::parse_ts_timestamp("2026-09-02T00:00:01Z").unwrap();
        copied.last_activity = "2026-09-02T00:00:01Z".into();
        copied.model = "gpt-5-mini".into();
        copied.input_tokens = 500;
        copied.total_tokens = 520;
        copied.service_tier = Some(CodexServiceTier::Fast);
        let shared = SharedArgs {
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_embedded();
        let seen = CodexDedupe::new(2, Some(&pricing));
        for (event, file_index) in [(&copied, 1), (&first, 0)] {
            seen.record(event, &mut CodexEventProvenance::new(file_index))
                .unwrap();
        }
        let groups = seen.fold(&shared, AgentReportKind::Session).unwrap();

        assert_eq!(groups.len(), 1);
        let group = &groups["first-session"];
        assert_eq!(group.total_tokens, 320);
        assert_eq!(group.last_activity.as_deref(), Some("2026-09-01T00:00:01Z"));
        assert_eq!(
            group.models["gpt-5"].recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
            300
        );
    }

    #[test]
    fn selects_codex_period_for_each_report_kind() {
        let timestamp = parse_ts_timestamp("2026-05-29T08:01:00.000Z").unwrap();
        let timezone = resolve_tz(Some("UTC"));
        let shared = SharedArgs::default();

        for (kind, expected) in [
            (AgentReportKind::Daily, "2026-05-29"),
            (AgentReportKind::Weekly, "2026-05-25"),
            (AgentReportKind::Monthly, "2026-05"),
            (AgentReportKind::Session, "sessions/child.jsonl"),
        ] {
            assert_eq!(
                codex_period_for(
                    timestamp,
                    Some("sessions/child.jsonl"),
                    kind,
                    &timezone,
                    &shared,
                )
                .as_deref(),
                Some(expected),
            );
        }
    }

    #[test]
    fn stores_timestamped_usage_only_for_time_dependent_models() {
        ccusage_test_support::install_pricing_snapshots();
        let line = |model: &str| {
            token_count_line(
                "2026-08-17T01:00:00.000Z",
                model,
                json!({"input_tokens": 1_000_000, "output_tokens": 0, "total_tokens": 1_000_000}),
            )
        };
        let groups = load_rollouts(
            &[(
                "session-1.jsonl",
                &[line("gpt-5"), line("deepseek-v4-flash")],
            )],
            AgentReportKind::Daily,
        );
        let models = &groups["2026-08-17"].models;

        assert!(models["gpt-5"].timestamped_usage.is_empty());
        assert_eq!(models["deepseek-v4-flash"].timestamped_usage.len(), 1);
    }

    #[test]
    fn records_flex_usage_in_model_buckets() {
        ccusage_test_support::install_pricing_snapshots();
        let rollout = [
            service_tier_line("2026-08-17T00:59:00.000Z", "flex"),
            token_count_line(
                "2026-08-17T01:00:00.000Z",
                "gpt-test",
                json!({
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "cache_write_input_tokens": 50,
                    "output_tokens": 25,
                    "reasoning_output_tokens": 5,
                    "total_tokens": 1_025,
                }),
            ),
        ];

        let groups = load_rollouts(&[("session-1.jsonl", &rollout)], AgentReportKind::Daily);
        let usage = &groups["2026-08-17"].models["gpt-test"];

        assert_eq!(usage.usage.input_tokens, 1_000);
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Flex].input_tokens,
            1_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Flex].cached_input_tokens,
            100
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Flex].cache_creation_tokens,
            50
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Flex].output_tokens,
            25
        );
    }

    #[test]
    fn omits_codex_period_outside_date_bounds() {
        let timezone = resolve_tz(Some("UTC"));
        let shared = SharedArgs {
            since: Some("20260528".to_string()),
            until: Some("20260530".to_string()),
            ..SharedArgs::default()
        };

        for timestamp in ["2026-05-27T23:59:59.000Z", "2026-05-31T00:00:00.000Z"] {
            assert_eq!(
                codex_period_for(
                    parse_ts_timestamp(timestamp).unwrap(),
                    Some("sessions/child.jsonl"),
                    AgentReportKind::Daily,
                    &timezone,
                    &shared,
                ),
                None,
            );
        }
    }

    #[test]
    fn omits_session_period_without_session_id() {
        let timestamp = parse_ts_timestamp("2026-05-29T08:01:00.000Z").unwrap();
        let timezone = resolve_tz(Some("UTC"));

        assert_eq!(
            codex_period_for(
                timestamp,
                None,
                AgentReportKind::Session,
                &timezone,
                &SharedArgs::default(),
            ),
            None,
        );
    }

    #[test]
    fn skips_historical_files_before_parsing_but_keeps_long_running_sessions() {
        ccusage_test_support::install_pricing_snapshots();
        let usage_line = |timestamp: &str, input_tokens: u64| {
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5",
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "cached_input_tokens": 0,
                            "output_tokens": 1,
                            "total_tokens": input_tokens + 1,
                        },
                    },
                },
            })
            .to_string()
        };
        let historical = [
            json!({
                "timestamp": "2025-01-01T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "historical"},
            })
            .to_string(),
            usage_line("2026-03-15T08:01:00.000Z", 999),
            r#"{"timestamp":"2026-03-15T08:02:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
        ]
        .join("\n");
        let long_running = [
            json!({
                "timestamp": "2026-01-01T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "long-running"},
            })
            .to_string(),
            usage_line("2026-01-01T08:01:00.000Z", 10),
            usage_line("2026-03-15T08:01:00.000Z", 100),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "sessions/2025/01/01/historical.jsonl": &historical,
            "sessions/2026/01/01/long-running.jsonl": &long_running,
        });
        let historical_path = fixture.path("sessions/2025/01/01/historical.jsonl");
        let long_running_path = fixture.path("sessions/2026/01/01/long-running.jsonl");
        crate::test_support::set_file_modified(
            &historical_path,
            parse_ts_timestamp("2025-01-01T08:01:00.000Z").unwrap(),
        );
        crate::test_support::set_file_modified(
            &long_running_path,
            parse_ts_timestamp("2026-03-15T08:01:00.000Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let load = |shared: &SharedArgs| {
            let diagnostics = Diagnostics::new("codex");
            let (groups, _) = load_groups_from_sources_with_diagnostics(
                &[directory_source(&fixture.path("sessions"))],
                shared,
                AgentReportKind::Daily,
                Some(&PricingMap::load_embedded()),
                &diagnostics,
            )
            .unwrap();
            (
                groups,
                diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
            )
        };

        for single_thread in [true, false] {
            let (bounded, bounded_skips) = load(&SharedArgs {
                single_thread,
                ..shared.clone()
            });
            let group = &bounded["2026-03-15"];
            assert_eq!(group.input_tokens, 100);
            assert_eq!(group.output_tokens, 1);
            assert_eq!(group.total_tokens, 101);
            assert_eq!(
                bounded_skips, 0,
                "the historical file was read past its first line"
            );

            let (unbounded, unbounded_skips) = load(&SharedArgs {
                single_thread,
                ..SharedArgs::default()
            });
            assert_eq!(unbounded_skips, 1);
            assert_eq!(unbounded["2026-03-15"].input_tokens, 1_099);
        }
    }

    #[test]
    fn retains_next_utc_path_day_for_local_until_boundary() {
        ccusage_test_support::install_pricing_snapshots();
        let late_utc = [
            json!({
                "timestamp": "2026-03-16T06:29:00.000Z",
                "type": "session_meta",
                "payload": {"id": "late-utc"},
            })
            .to_string(),
            json!({
                "timestamp": "2026-03-16T06:30:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5",
                        "last_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 0,
                            "output_tokens": 1,
                            "total_tokens": 101,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "sessions/2026/03/16/late-utc.jsonl": &late_utc,
        });
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        assert_eq!(groups["2026-03-15"].input_tokens, 100);
        assert_eq!(groups["2026-03-15"].total_tokens, 101);
    }

    #[test]
    fn dedupes_copied_token_usage_across_session_files() {
        ccusage_test_support::install_pricing_snapshots();
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let service_tier_line = json!({
            "timestamp": "2026-05-29T08:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "priority" },
            },
        })
        .to_string();
        let rollout = format!("{service_tier_line}\n{usage_line}");
        let unclassified_first = fs_fixture!({
            "sessions/a-unclassified.jsonl": &usage_line,
            "sessions/z-recorded.jsonl": &rollout,
        });
        let recorded_first = fs_fixture!({
            "sessions/a-recorded.jsonl": &rollout,
            "sessions/z-unclassified.jsonl": &usage_line,
        });
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.2": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "cache_read_input_token_cost": 0.0000005,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
        );
        let mut costs = Vec::new();
        for fixture in [&unclassified_first, &recorded_first] {
            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };

                let groups = load_groups_from_directory(
                    &fixture.path("sessions"),
                    &shared,
                    AgentReportKind::Daily,
                )
                .unwrap();

                assert_eq!(groups.len(), 1);
                let group = groups.get("2026-05-29").unwrap();
                assert_eq!(group.input_tokens, 1_000);
                assert_eq!(group.cached_input_tokens, 100);
                assert_eq!(group.output_tokens, 200);
                assert_eq!(group.reasoning_output_tokens, 20);
                assert_eq!(group.total_tokens, 1_200);
                assert_eq!(
                    group.models["gpt-5.2"].recorded_usage[crate::CodexServiceTier::Fast]
                        .input_tokens,
                    1_000
                );
                costs.push(crate::calculate_group_cost(
                    group,
                    Some(&pricing),
                    CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
                ));
            }
        }
        assert!(costs.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn resolves_conflicting_duplicate_tiers_as_standard() {
        ccusage_test_support::install_pricing_snapshots();
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let rollout = |service_tier: &str| {
            format!(
                "{}\n{usage_line}",
                json!({
                    "timestamp": "2026-05-29T08:00:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": service_tier },
                    },
                })
            )
        };
        let standard = rollout("default");
        let fast = rollout("priority");
        let standard_first = fs_fixture!({
            "sessions/a-standard.jsonl": &standard,
            "sessions/z-fast.jsonl": &fast,
        });
        let fast_first = fs_fixture!({
            "sessions/a-fast.jsonl": &fast,
            "sessions/z-standard.jsonl": &standard,
        });

        for fixture in [&standard_first, &fast_first] {
            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };
                let groups = load_groups_from_directory(
                    &fixture.path("sessions"),
                    &shared,
                    AgentReportKind::Daily,
                )
                .unwrap();
                let usage = &groups["2026-05-29"].models["gpt-5.2"];

                assert_eq!(usage.usage.input_tokens, 1_000);
                assert_eq!(
                    usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
                    1_000
                );
                assert_eq!(
                    usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
                    0
                );
            }
        }
    }

    #[test]
    fn tracks_long_context_token_split_per_request() {
        ccusage_test_support::install_pricing_snapshots();
        let usage_line = |input: u64, cached: u64, output: u64| {
            json!({
                "timestamp": "2026-07-09T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": cached,
                            "output_tokens": output,
                            "reasoning_output_tokens": 0,
                            "total_tokens": input + output,
                        },
                    },
                },
            })
            .to_string()
        };
        let long_line = usage_line(280_000, 20_000, 500);
        let short_line = usage_line(100_000, 50_000, 300);
        let fast_marker = json!({
            "timestamp": "2026-07-09T08:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "priority" },
            },
        })
        .to_string();
        let standard_marker = json!({
            "timestamp": "2026-07-09T08:02:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": "default" },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &format!(
                "{fast_marker}\n{long_line}\n{standard_marker}\n{short_line}"
            ),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        let group = groups.get("2026-07-09").unwrap();
        let usage = group.models.get("gpt-5.6-sol").unwrap();
        assert_eq!(usage.usage.input_tokens, 380_000);
        assert_eq!(usage.usage.cached_input_tokens, 70_000);
        assert_eq!(usage.usage.output_tokens, 800);
        assert_eq!(usage.usage.long_context_input_tokens, 280_000);
        assert_eq!(usage.usage.long_context_cached_input_tokens, 20_000);
        assert_eq!(usage.usage.long_context_output_tokens, 500);
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
            280_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].cached_input_tokens,
            20_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].output_tokens,
            500
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_input_tokens,
            280_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_cached_input_tokens,
            20_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_output_tokens,
            500
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
            100_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].cached_input_tokens,
            50_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].output_tokens,
            300
        );
    }

    #[test]
    fn parallel_merge_preserves_speed_and_long_context_buckets() {
        ccusage_test_support::install_pricing_snapshots();
        let rollout = |marker_timestamp: &str,
                       usage_timestamp: &str,
                       service_tier: &str,
                       input_tokens: u64,
                       cached_input_tokens: u64,
                       cache_creation_tokens: u64,
                       output_tokens: u64| {
            [
                json!({
                    "timestamp": marker_timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": { "service_tier": service_tier },
                    },
                })
                .to_string(),
                json!({
                    "timestamp": usage_timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "model": "gpt-5.6-sol",
                            "last_token_usage": {
                                "input_tokens": input_tokens,
                                "cached_input_tokens": cached_input_tokens,
                                "cache_write_input_tokens": cache_creation_tokens,
                                "output_tokens": output_tokens,
                                "total_tokens": input_tokens + output_tokens,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n")
        };
        let fast_long = rollout(
            "2026-07-09T08:00:00.000Z",
            "2026-07-09T08:01:00.000Z",
            "priority",
            280_000,
            20_000,
            30_000,
            500,
        );
        let standard_short = rollout(
            "2026-07-09T09:00:00.000Z",
            "2026-07-09T09:01:00.000Z",
            "default",
            100_000,
            50_000,
            10_000,
            300,
        );
        let fixture = fs_fixture!({
            "sessions/fast.jsonl": &fast_long,
            "sessions/standard.jsonl": &standard_short,
        });
        let mut observed = Vec::new();

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };
            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();
            let usage = &groups["2026-07-09"].models["gpt-5.6-sol"];

            assert_eq!(usage.usage.input_tokens, 380_000);
            assert_eq!(usage.usage.cache_creation_tokens, 40_000);
            assert_eq!(usage.usage.long_context_input_tokens, 280_000);
            assert_eq!(usage.usage.long_context_cache_creation_tokens, 30_000);
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
                280_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Fast].cache_creation_tokens,
                30_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_input_tokens,
                280_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Fast]
                    .long_context_cache_creation_tokens,
                30_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
                100_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Standard].cache_creation_tokens,
                10_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Standard].long_context_input_tokens,
                0
            );
            observed.push((
                usage.recorded_usage[crate::CodexServiceTier::Fast],
                usage.recorded_usage[crate::CodexServiceTier::Standard],
            ));
        }

        assert_eq!(observed[0], observed[1]);
    }

    #[test]
    fn folds_cache_creation_usage_into_groups_and_recorded_buckets() {
        ccusage_test_support::install_pricing_snapshots();
        let rollout = [
            service_tier_line("2026-07-09T08:00:00Z", "default"),
            token_count_line(
                "2026-07-09T08:01:00Z",
                "gpt-5.6-sol",
                json!({
                    "input_tokens": 280_000,
                    "cache_write_input_tokens": 30,
                    "output_tokens": 1,
                }),
            ),
        ];
        let groups = load_rollouts(&[("session.jsonl", &rollout)], AgentReportKind::Daily);
        let group = &groups["2026-07-09"];
        let usage = &group.models["gpt-5.6-sol"];
        assert_eq!(group.cache_creation_tokens, 30);
        assert_eq!(usage.usage.cache_creation_tokens, 30);
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard]
                .long_context_cache_creation_tokens,
            30
        );
    }

    #[test]
    fn dedupes_copied_token_usage_after_model_alias_resolution() {
        ccusage_test_support::install_pricing_snapshots();
        let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.2")]);
        let private_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "private-alpha",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let canonical_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &private_usage_line,
            "sessions/goal.jsonl": &canonical_usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();

            let group = groups.get("2026-05-29").unwrap();
            assert_eq!(group.input_tokens, 1_000);
            assert_eq!(group.models.len(), 1);
            assert_eq!(group.models["gpt-5.2"].usage.input_tokens, 1_000);
        }
    }

    #[test]
    fn keeps_matching_token_usage_in_distinct_session_groups() {
        ccusage_test_support::install_pricing_snapshots();
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &usage_line,
            "sessions/goal.jsonl": &usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Session,
            )
            .unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["root"].input_tokens, 1_000);
            assert_eq!(groups["goal"].input_tokens, 1_000);
        }
    }

    #[test]
    fn aggregates_active_copy_when_archived_file_has_same_relative_path() {
        ccusage_test_support::install_pricing_snapshots();
        let active_usage = [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 111,
                            "cached_input_tokens": 10,
                            "output_tokens": 20,
                            "reasoning_output_tokens": 1,
                            "total_tokens": 131,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let archived_usage = [
            json!({
                "timestamp": "2026-05-12T09:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T09:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 999,
                            "cached_input_tokens": 90,
                            "output_tokens": 80,
                            "reasoning_output_tokens": 7,
                            "total_tokens": 1_079,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "codex/sessions/duplicate.jsonl": active_usage,
            "codex/archived_sessions/duplicate.jsonl": archived_usage,
            "codex/archived_sessions/archived-only.jsonl": [
                json!({
                    "timestamp": "2026-05-13T08:00:00.000Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "gpt-5.2",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-13T08:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 222,
                                "cached_input_tokens": 20,
                                "output_tokens": 30,
                                "reasoning_output_tokens": 2,
                                "total_tokens": 252,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                ..SharedArgs::default()
            };
            let sources = vec![
                CodexUsageSource::new(fixture.path("codex/sessions"), fixture.path("codex")),
                CodexUsageSource::new(
                    fixture.path("codex/archived_sessions"),
                    fixture.path("codex"),
                ),
            ];
            let groups =
                load_groups_from_sources(&sources, &shared, AgentReportKind::Daily).unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["2026-05-12"].input_tokens, 111);
            assert_eq!(groups["2026-05-13"].input_tokens, 222);
        }
    }

    fn fork_token_count_line(timestamp: &str, last: u64, total: u64) -> String {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {"input_tokens": last, "output_tokens": 1, "total_tokens": last + 1},
                    "total_token_usage": {"input_tokens": total, "output_tokens": 1, "total_tokens": total + 1},
                },
            },
        })
        .to_string()
    }

    #[test]
    fn skips_replayed_history_of_forks_read_during_the_pass() {
        ccusage_test_support::install_pricing_snapshots();
        let parent = [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
            })
            .to_string(),
            fork_token_count_line("2026-05-12T08:01:00.000Z", 1_000, 1_000),
            fork_token_count_line("2026-05-12T08:02:00.000Z", 500, 1_500),
        ]
        .join("\n");
        let fork = [
            json!({
                "timestamp": "2026-05-12T08:03:00.000Z",
                "type": "session_meta",
                "payload": {"id": "fork-abc", "forked_from_id": "parent-xyz"},
            })
            .to_string(),
            fork_token_count_line("2026-05-12T08:03:00.000Z", 1_000, 1_000),
            fork_token_count_line("2026-05-12T08:03:00.000Z", 500, 1_500),
            fork_token_count_line("2026-05-12T08:04:00.000Z", 100, 1_600),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "codex/sessions/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent.clone(),
            "codex/archived_sessions/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork.clone(),
            "single/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent,
            "single/2026/05/12/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork,
        });
        let sources = [
            CodexUsageSource::new(fixture.path("codex/sessions"), fixture.path("codex")),
            CodexUsageSource::new(
                fixture.path("codex/archived_sessions"),
                fixture.path("codex"),
            ),
        ];

        for single_thread in [true, false] {
            for since in [None, Some("20260101".to_string())] {
                let shared = SharedArgs {
                    single_thread,
                    since,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };
                let context = format!("single_thread={single_thread} since={:?}", shared.since);
                let sessions = |groups: BTreeMap<String, CodexGroup>| {
                    groups
                        .into_iter()
                        .map(|(session, group)| (session, group.input_tokens))
                        .collect::<Vec<_>>()
                };
                let by_sources =
                    load_groups_from_sources(&sources, &shared, AgentReportKind::Session).unwrap();
                assert_eq!(
                    sessions(by_sources),
                    [
                        (
                            "2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz".to_string(),
                            1_500
                        ),
                        ("rollout-2026-05-12T08-03-00-fork-abc".to_string(), 100),
                    ],
                    "{context}"
                );
                let by_directory = load_groups_from_directory(
                    &fixture.path("single"),
                    &shared,
                    AgentReportKind::Session,
                )
                .unwrap();
                assert_eq!(
                    sessions(by_directory),
                    [
                        (
                            "2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz".to_string(),
                            1_500
                        ),
                        (
                            "2026/05/12/rollout-2026-05-12T08-03-00-fork-abc".to_string(),
                            100
                        ),
                    ],
                    "{context}"
                );
            }
        }
    }

    const IDENTICAL_REQUESTS_ROLLOUT: &str = r#"{"timestamp":"2026-11-01T09:42:09.923Z","type":"session_meta","payload":{"id":"01a1967c-0000-7000-8000-00000000000a","forked_from_id":"01a1967b-0000-7000-8000-000000000001","timestamp":"2026-11-01T09:42:09.923Z","originator":"codex_vscode","cli_version":"0.164.0-alpha.11.1","source":{"subagent":{"thread_spawn":{"parent_thread_id":"01a1967b-0000-7000-8000-000000000001","depth":1}}},"model_provider":"openai"}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"compacted","payload":{"message":"","replacement_history":[]}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"turn_context","payload":{"model":"gpt-6-astra"}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":990000,"cached_input_tokens":0,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":1000000},"last_token_usage":{"input_tokens":990000,"cached_input_tokens":0,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":1000000}}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"token_usage_record","payload":{"response_id":"resp_child_1","usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000000,"cached_input_tokens":2000,"output_tokens":10100,"reasoning_output_tokens":0,"total_tokens":1010100},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"token_usage_record","payload":{"response_id":"resp_child_2","usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1010000,"cached_input_tokens":4000,"output_tokens":10200,"reasoning_output_tokens":0,"total_tokens":1020200},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}}"#;

    fn identical_request_line() -> String {
        json!({
            "timestamp": "2026-11-01T09:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 10,
                        "total_tokens": 1_010,
                    },
                },
            },
        })
        .to_string()
    }

    fn daily_input_tokens(sessions_dir: &Path, single_thread: bool) -> Vec<(String, u64)> {
        let shared = SharedArgs {
            single_thread,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        load_groups_from_directory(sessions_dir, &shared, AgentReportKind::Daily)
            .unwrap()
            .into_iter()
            .map(|(period, group)| (period, group.input_tokens))
            .collect()
    }

    #[test]
    fn counts_identical_requests_within_one_rollout() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
                IDENTICAL_REQUESTS_ROLLOUT,
        });
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            let events = crate::test_support::rollout_events(&sessions)
                .into_iter()
                .map(|event| {
                    (
                        event.input_tokens,
                        event.cached_input_tokens,
                        event.output_tokens,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                events,
                [(10_000, 2_000, 100), (10_000, 2_000, 100)],
                "single_thread={single_thread}"
            );

            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };
            let groups =
                load_groups_from_directory(&sessions, &shared, AgentReportKind::Session).unwrap();
            let totals = groups
                .values()
                .map(|group| {
                    (
                        group.input_tokens - group.cached_input_tokens,
                        group.cached_input_tokens,
                        group.output_tokens,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                totals,
                [(16_000, 4_000, 200)],
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn counts_each_copied_request_once_across_rollouts() {
        ccusage_test_support::install_pricing_snapshots();
        let request = identical_request_line();
        let both_requests = format!("{request}\n{request}");
        let scenarios = [
            ("copy of both requests", both_requests.clone()),
            ("copy of the first request", request.clone()),
        ];

        for (name, copy) in scenarios {
            let fixture = fs_fixture!({
                "sessions/a-original.jsonl": &both_requests,
                "sessions/b-copy.jsonl": &copy,
            });
            let sessions = fixture.path("sessions");

            for single_thread in [true, false] {
                assert_eq!(
                    daily_input_tokens(&sessions, single_thread),
                    [("2026-11-01".to_string(), 2_000)],
                    "{name}, single_thread={single_thread}"
                );
            }
        }
    }

    #[test]
    fn counts_spawned_siblings_with_identical_usage_and_dedupes_copied_records() {
        ccusage_test_support::install_pricing_snapshots();
        let sibling = IDENTICAL_REQUESTS_ROLLOUT
            .replace("00000000000a", "00000000000b")
            .replace("resp_child_1", "resp_child_3")
            .replace("resp_child_2", "resp_child_4");
        let copy = IDENTICAL_REQUESTS_ROLLOUT.replace("00000000000a", "00000000000c");
        let scenarios = [
            ("distinct sibling", sibling, (32_000, 8_000, 400, 40_400)),
            ("copied records", copy, (16_000, 4_000, 200, 20_200)),
        ];

        for (name, second, expected) in scenarios {
            let fixture = fs_fixture!({
                "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
                    IDENTICAL_REQUESTS_ROLLOUT,
                "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000b.jsonl":
                    &second,
            });
            let sessions = fixture.path("sessions");

            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };
                let groups =
                    load_groups_from_directory(&sessions, &shared, AgentReportKind::Daily).unwrap();
                let group = &groups["2026-11-01"];
                assert_eq!(
                    (
                        group.input_tokens - group.cached_input_tokens,
                        group.cached_input_tokens,
                        group.output_tokens,
                        group.total_tokens,
                    ),
                    expected,
                    "{name}, single_thread={single_thread}"
                );
            }
        }
    }

    fn recorded_requests(service_tier: Option<&str>, response_ids: &[Option<&str>]) -> String {
        let mut lines = vec![
            json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "turn_context", "payload": {"model": "gpt-5.2"}})
                .to_string(),
        ];
        if let Some(service_tier) = service_tier {
            lines.push(
                json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "event_msg", "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": service_tier}}})
                    .to_string(),
            );
        }
        let timestamp = "2026-11-01T09:01:00.000Z";
        for (index, response_id) in response_ids.iter().enumerate() {
            let usage = json!({"input_tokens": 1_000, "cached_input_tokens": 100, "output_tokens": 10, "total_tokens": 1_010});
            let total = (u64::try_from(index).unwrap() + 1) * 1_000;
            if let Some(response_id) = response_id {
                lines.push(
                    json!({"timestamp": timestamp, "type": "token_usage_record", "payload": {"response_id": response_id, "usage": usage}})
                        .to_string(),
                );
            }
            lines.push(
                json!({
                    "timestamp": timestamp,
                    "type": "event_msg",
                    "payload": {"type": "token_count", "info": {
                        "last_token_usage": usage,
                        "total_token_usage": {"input_tokens": total, "cached_input_tokens": total / 10, "output_tokens": total / 100, "total_tokens": total + total / 100},
                    }},
                })
                .to_string(),
            );
        }
        lines.join("\n")
    }

    #[test]
    fn matches_recordless_copies_to_recorded_requests_in_either_file_order() {
        ccusage_test_support::install_pricing_snapshots();
        let original = recorded_requests(None, &[Some("resp_a1"), Some("resp_a2")]);
        let sibling = recorded_requests(None, &[Some("resp_b1"), Some("resp_b2")]);
        let copy = recorded_requests(None, &[None, None]);
        let partial = recorded_requests(None, &[Some("resp_a2")]);
        let scenarios = [
            ("recorded first", vec![&original, &copy], 2),
            ("recordless first", vec![&copy, &original], 2),
            ("siblings, then copy", vec![&original, &sibling, &copy], 4),
            ("copy between siblings", vec![&original, &copy, &sibling], 4),
            (
                "copy first, then siblings",
                vec![&copy, &original, &sibling],
                4,
            ),
            ("partial copy last", vec![&original, &partial], 2),
            ("partial copy first", vec![&partial, &original], 2),
            (
                "partial and recordless copies",
                vec![&partial, &copy, &original],
                2,
            ),
        ];

        for (name, files, event_count) in scenarios {
            let fixture = ccusage_test_support::Fixture::new();
            for (index, contents) in files.iter().enumerate() {
                fixture.write_file(format!("sessions/{index}-rollout.jsonl"), contents);
            }
            let sessions = fixture.path("sessions");

            for single_thread in [true, false] {
                assert_eq!(
                    daily_input_tokens(&sessions, single_thread),
                    [("2026-11-01".to_string(), 1_000 * event_count)],
                    "{name}, single_thread={single_thread}"
                );
            }
        }
    }

    #[test]
    fn keeps_recorded_tiers_when_a_recordless_copy_cannot_name_its_request() {
        ccusage_test_support::install_pricing_snapshots();
        let fast = recorded_requests(Some("priority"), &[Some("resp_fast")]);
        let flex = recorded_requests(Some("flex"), &[Some("resp_flex")]);
        let copy = recorded_requests(Some("priority"), &[None]);
        let orders = [
            [&fast, &flex, &copy],
            [&fast, &copy, &flex],
            [&flex, &fast, &copy],
            [&flex, &copy, &fast],
            [&copy, &fast, &flex],
            [&copy, &flex, &fast],
        ];

        for (order, files) in orders.iter().enumerate() {
            let fixture = ccusage_test_support::Fixture::new();
            for (index, contents) in files.iter().enumerate() {
                fixture.write_file(format!("sessions/{index}-rollout.jsonl"), contents);
            }
            let sessions = fixture.path("sessions");

            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".to_string()),
                    ..SharedArgs::default()
                };
                let groups =
                    load_groups_from_directory(&sessions, &shared, AgentReportKind::Daily).unwrap();
                let usage = &groups["2026-11-01"].models["gpt-5.2"];
                assert_eq!(
                    (
                        usage.usage.input_tokens,
                        usage.recorded_usage[CodexServiceTier::Fast].input_tokens,
                        usage.recorded_usage[CodexServiceTier::Flex].input_tokens,
                    ),
                    (2_000, 1_000, 1_000),
                    "order={order}, single_thread={single_thread}"
                );
            }
        }
    }
    #[test]
    fn credits_a_replaced_stand_in_to_the_recording_session() {
        ccusage_test_support::install_pricing_snapshots();
        let child = recorded_requests(None, &[None]);
        let parent = recorded_requests(None, &[Some("resp_parent")]);
        let orders = [
            [("0-child", &child), ("1-parent", &parent)],
            [("0-parent", &parent), ("1-child", &child)],
        ];

        for files in orders {
            let fixture = ccusage_test_support::Fixture::new();
            for (name, contents) in files {
                fixture.write_file(format!("sessions/{name}.jsonl"), contents);
            }
            let sessions = fixture.path("sessions");
            let parent_session = files
                .iter()
                .find(|(name, _)| name.ends_with("-parent"))
                .unwrap()
                .0;

            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".into()),
                    since: Some("20261101".into()),
                    until: Some("20261101".into()),
                    ..SharedArgs::default()
                };
                let (groups, _) = load_groups_from_sources_with_diagnostics(
                    &[directory_source(&sessions)],
                    &shared,
                    AgentReportKind::Session,
                    None,
                    &Diagnostics::new("codex"),
                )
                .unwrap();
                assert_eq!(
                    groups
                        .into_iter()
                        .map(|(session, group)| (session, group.input_tokens))
                        .collect::<Vec<_>>(),
                    [(parent_session.to_string(), 1_000)],
                    "parent={parent_session}, single_thread={single_thread}"
                );
                assert_eq!(
                    daily_input_tokens(&sessions, single_thread),
                    [("2026-11-01".to_string(), 1_000)],
                    "parent={parent_session}, single_thread={single_thread}"
                );
            }
        }
    }

    #[test]
    fn replaces_the_stand_in_attribution_regardless_of_arrival_order() {
        let stand_in = CodexTokenUsageEvent {
            session_id: "session-b".into(),
            response_id: None,
            record_response_id: None,
            timestamp: parse_ts_timestamp("2026-11-01T09:00:00Z").unwrap(),
            last_activity: "2026-11-01T09:00:00Z".into(),
            model: "gpt-5.2".into(),
            input_tokens: 100,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 100,
            is_fallback_model: true,
            service_tier: Some(CodexServiceTier::Fast),
        };
        let recorded = CodexTokenUsageEvent {
            session_id: "session-a".into(),
            record_response_id: Some("resp-a".into()),
            is_fallback_model: false,
            service_tier: Some(CodexServiceTier::Flex),
            last_activity: "2026-11-01T09:00:00.000Z".into(),
            ..stand_in.clone()
        };
        let events = [stand_in, recorded];
        for order in [[0, 1], [1, 0]] {
            let seen = CodexDedupe::new(2, None);
            for file_index in order {
                seen.record(
                    &events[file_index],
                    &mut CodexEventProvenance::new(file_index),
                )
                .unwrap();
            }
            let groups = seen
                .fold(&SharedArgs::default(), AgentReportKind::Session)
                .unwrap();
            assert_eq!(
                groups.keys().map(String::as_str).collect::<Vec<_>>(),
                ["session-a"]
            );
            let group = &groups["session-a"];
            assert_eq!(group.input_tokens, 100);
            assert_eq!(
                group.last_activity.as_deref(),
                Some("2026-11-01T09:00:00.000Z")
            );
            let model = &group.models["gpt-5.2"];
            assert!(!model.is_fallback);
            assert_eq!(
                model.recorded_usage[CodexServiceTier::Flex].input_tokens,
                100
            );
        }
    }

    #[test]
    fn preserves_recordless_requests_that_cooccur_with_their_recorded_match() {
        ccusage_test_support::install_pricing_snapshots();
        let recorded = recorded_requests(Some("priority"), &[Some("resp_x")]);
        let mixed_after = recorded_requests(Some("standard"), &[Some("resp_x"), None]);
        let mixed_before = recorded_requests(Some("standard"), &[None, Some("resp_x")]);
        let recordless = recorded_requests(Some("standard"), &[None, None]);
        for mixed in [&mixed_after, &mixed_before] {
            for files in [
                vec![&recorded, mixed],
                vec![mixed, &recorded],
                vec![&recorded, &recordless, mixed],
                vec![mixed, &recorded, &recordless],
                vec![&recordless, mixed, &recorded],
            ] {
                let fixture = ccusage_test_support::Fixture::new();
                for (index, contents) in files.iter().enumerate() {
                    fixture.write_file(format!("sessions/{index}.jsonl"), contents);
                }
                let sessions = fixture.path("sessions");
                for single_thread in [true, false] {
                    assert_eq!(
                        daily_input_tokens(&sessions, single_thread),
                        [("2026-11-01".to_string(), 2_000)],
                        "single_thread={single_thread}"
                    );
                }
            }
        }
    }

    #[test]
    fn settles_mixed_recordless_tiers_independently_of_file_order() {
        ccusage_test_support::install_pricing_snapshots();
        let change_last_tier = |mut contents: String, tier| {
            let last_count = contents.rfind('\n').unwrap() + 1;
            let settings = service_tier_line("2026-11-01T09:01:00.000Z", tier);
            contents.insert_str(last_count, &format!("{settings}\n"));
            contents
        };
        let named = recorded_requests(Some("priority"), &[Some("resp_x")]);
        let anonymous =
            change_last_tier(recorded_requests(Some("flex"), &[None, None]), "standard");
        let mixed = change_last_tier(
            recorded_requests(Some("priority"), &[Some("resp_x"), None]),
            "standard",
        );
        for files in [
            [&named, &anonymous, &mixed],
            [&named, &mixed, &anonymous],
            [&anonymous, &named, &mixed],
            [&anonymous, &mixed, &named],
            [&mixed, &named, &anonymous],
            [&mixed, &anonymous, &named],
        ] {
            let fixture = ccusage_test_support::Fixture::new();
            for (index, contents) in files.iter().enumerate() {
                fixture.write_file(format!("sessions/{index}.jsonl"), contents);
            }
            for single_thread in [true, false] {
                let shared = SharedArgs {
                    single_thread,
                    timezone: Some("UTC".into()),
                    ..SharedArgs::default()
                };
                let groups = load_groups_from_directory(
                    &fixture.path("sessions"),
                    &shared,
                    AgentReportKind::Daily,
                )
                .unwrap();
                let usage = &groups["2026-11-01"].models["gpt-5.2"];
                assert_eq!(
                    (
                        usage.usage.input_tokens,
                        usage.recorded_usage[CodexServiceTier::Fast].input_tokens,
                        usage.recorded_usage[CodexServiceTier::Standard].input_tokens,
                        usage.recorded_usage[CodexServiceTier::Flex].input_tokens,
                    ),
                    (2_000, 1_000, 1_000, 0),
                    "single_thread={single_thread}"
                );
            }
        }
    }
}
