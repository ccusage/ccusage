use ccusage_core::Diagnostics;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use compact_str::CompactString;
use jiff::tz::TimeZone as JiffTimeZone;
use rustc_hash::FxHasher;

use crate::{
    CodexGroup, CodexServiceTier, CodexTokenUsageEvent, CodexUsageBucket, Result,
    cli::{AgentReportKind, SharedArgs, WeekDay},
    fast::{FxHashMap, FxHashSet},
    format_date_tz, merge_codex_service_tiers, parse_ts_timestamp, resolve_tz, wants_json,
    week_start,
};

use super::{
    loader::has_date_bounds,
    parser, paths,
    replay::{CodexReplayPlan, CodexSessionMetadata},
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum CodexEventKey {
    Response(CompactString),
    Usage(CodexUsageKey),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CodexUsageKey {
    session_hash: u64,
    session_len: usize,
    timestamp: crate::TimestampMs,
    model_hash: u64,
    model_len: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    occurrence: u32,
}

/// Numbers the identical usage events of one rollout file.
///
/// A rollout records each request once, so identical events in one file are
/// distinct requests and get distinct ordinals. Usage copied into another file
/// repeats the ordinals of the original, which is what lets the dedupe drop it.
pub(super) struct CodexFileOccurrences<K>(FxHashMap<K, u32>);

impl<K> Default for CodexFileOccurrences<K> {
    fn default() -> Self {
        Self(FxHashMap::default())
    }
}

impl<K: Hash + Eq + Clone> CodexFileOccurrences<K> {
    pub(super) fn next(&mut self, key: &K) -> u32 {
        if let Some(count) = self.0.get_mut(key) {
            let occurrence = *count;
            *count += 1;
            return occurrence;
        }
        self.0.insert(key.clone(), 1);
        0
    }
}

/// Where an event was read from: the file's position in the run, and the
/// ordinals of the identical usage events seen in it so far.
struct CodexEventProvenance {
    file_index: usize,
    occurrences: CodexFileOccurrences<CodexUsageKey>,
}

impl CodexEventProvenance {
    fn new(file_index: usize) -> Self {
        Self {
            file_index,
            occurrences: CodexFileOccurrences::default(),
        }
    }

    fn event_key(
        &mut self,
        event: &CodexTokenUsageEvent,
        timestamp: crate::TimestampMs,
        model: &str,
        kind: AgentReportKind,
    ) -> CodexEventKey {
        let mut key = codex_event_key(event, timestamp, model, kind);
        if let CodexEventKey::Usage(usage) = &mut key {
            usage.occurrence = self.occurrences.next(usage);
        }
        key
    }
}

struct CodexDedupeRecord {
    service_tier: Option<CodexServiceTier>,
    model: CompactString,
    session_id: Option<CompactString>,
    compaction: Option<Box<CodexCompactionEvent>>,
}

struct CodexCompactionEvent {
    file_index: usize,
    timestamp: crate::TimestampMs,
    event: CodexTokenUsageEvent,
}

type CodexDedupeMap = FxHashMap<CodexEventKey, CodexDedupeRecord>;
type CodexDedupeShards = [Mutex<CodexDedupeMap>];

struct CodexAggregation {
    groups: BTreeMap<String, CodexGroup>,
    seen: CodexDedupeMap,
}

/// How a run tells which rollouts replayed a parent's history.
#[derive(Clone, Copy)]
enum CodexReplay<'a> {
    Planned(&'a CodexReplayPlan),
    /// Reads each rollout's metadata from its own first line and leaves forks
    /// for a planned run, so no separate pass opens every rollout up front.
    Discover,
}

/// Read-only inputs every file of one aggregation run shares.
struct CodexAggregateRun<'a> {
    sessions_dir: &'a Path,
    files: &'a [PathBuf],
    shared: &'a SharedArgs,
    kind: AgentReportKind,
    replay: CodexReplay<'a>,
    /// Indexes into `files` this run reads; `None` reads all of them.
    selection: Option<&'a [usize]>,
    file_offset: usize,
    diagnostics: &'a Diagnostics,
}

impl CodexAggregateRun<'_> {
    fn file_indexes(&self) -> Vec<usize> {
        self.selection
            .map_or_else(|| (0..self.files.len()).collect(), <[usize]>::to_vec)
    }

    fn chunk_file_indexes(&self, worker_count: usize) -> Vec<Vec<usize>> {
        let Some(selection) = self.selection else {
            return crate::chunk_file_indexes_by_size(self.files, worker_count);
        };
        let files = selection
            .iter()
            .map(|&index| self.files[index].clone())
            .collect::<Vec<_>>();
        crate::chunk_file_indexes_by_size(&files, worker_count)
            .into_iter()
            .map(|chunk| chunk.into_iter().map(|index| selection[index]).collect())
            .collect()
    }
}

/// Session metadata a discovering run read, by index into its files.
type CodexFileMetadata = Vec<(usize, CodexSessionMetadata)>;

/// Orders a discovering run's metadata by file index and lists the forks.
fn fork_indexes(
    file_count: usize,
    metadata: CodexFileMetadata,
) -> (Vec<usize>, Vec<CodexSessionMetadata>) {
    let mut ordered = vec![CodexSessionMetadata::default(); file_count];
    for (index, file_metadata) in metadata {
        ordered[index] = file_metadata;
    }
    let forks = ordered
        .iter()
        .enumerate()
        .filter_map(|(index, file_metadata)| file_metadata.is_fork().then_some(index))
        .collect();
    (forks, ordered)
}

pub fn load_groups(
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    let diagnostics = Diagnostics::new("codex");
    let (groups, _) = load_groups_with_diagnostics(shared, kind, wants_json(shared), &diagnostics)?;
    diagnostics.emit(shared);
    Ok(groups)
}

pub(super) fn load_groups_with_diagnostics(
    shared: &SharedArgs,
    kind: AgentReportKind,
    multi_source_driver: bool,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let sources = paths::codex_usage_sources()?;
    if sources.len() == 1 && !multi_source_driver {
        return load_groups_from_directory_with_diagnostics(
            &sources[0].dir,
            shared,
            kind,
            diagnostics,
        );
    }
    load_groups_from_sources_with_diagnostics(&sources, shared, kind, diagnostics)
}

fn load_groups_from_sources_with_diagnostics(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let file_groups = paths::collect_deduped_codex_usage_files(sources, diagnostics);
    let detected = file_groups.iter().any(|group| !group.files.is_empty());
    let files_by_group = file_groups
        .iter()
        .map(|group| paths::filter_codex_usage_files(&group.dir, &group.files, shared))
        .collect::<Vec<_>>();
    let all_files = file_groups
        .iter()
        .map(|group| (group.dir.as_path(), group.files.as_slice()))
        .collect::<Vec<_>>();
    let selected_files = file_groups
        .iter()
        .zip(&files_by_group)
        .map(|(group, files)| (group.dir.as_path(), files.as_slice()))
        .collect::<Vec<_>>();
    aggregate_file_groups(&selected_files, &all_files, shared, kind, diagnostics)
        .map(|groups| (groups, detected))
}

pub(super) fn load_groups_from_directory_with_diagnostics(
    sessions_dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let all_files = paths::collect_codex_usage_files(sessions_dir, diagnostics);
    let detected = !all_files.is_empty();
    let files = paths::filter_codex_usage_files(sessions_dir, &all_files, shared);
    if shared.single_thread {
        return aggregate_files_local(sessions_dir, &files, &all_files, shared, kind, diagnostics)
            .map(|groups| (groups, detected));
    }
    aggregate_file_groups(
        &[(sessions_dir, files.as_slice())],
        &[(sessions_dir, all_files.as_slice())],
        shared,
        kind,
        diagnostics,
    )
    .map(|groups| (groups, detected))
}

/// Aggregates `selected` against the replay history of `available`, the
/// unfiltered files of the same groups.
fn aggregate_file_groups(
    selected: &[(&Path, &[PathBuf])],
    available: &[(&Path, &[PathBuf])],
    shared: &SharedArgs,
    kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let bounded_plan = has_date_bounds(shared).then(|| {
        CodexReplayPlan::for_bounded_files(
            selected.iter().copied(),
            available.iter().copied(),
            shared.single_thread,
            diagnostics,
        )
    });
    let replay = bounded_plan
        .as_ref()
        .map_or(CodexReplay::Discover, CodexReplay::Planned);
    let mut groups = BTreeMap::new();
    let seen = create_dedupe_shards();
    let mut metadata_by_group = Vec::with_capacity(selected.len());
    let mut file_offset = 0;
    for &(sessions_dir, files) in selected {
        let run = CodexAggregateRun {
            sessions_dir,
            files,
            shared,
            kind,
            replay,
            selection: None,
            file_offset,
            diagnostics,
        };
        let (group_rows, metadata) = aggregate_files_with_dedupe(&run, &seen)?;
        merge_groups(&mut groups, group_rows);
        metadata_by_group.push(fork_indexes(files.len(), metadata));
        file_offset += files.len();
    }
    if matches!(replay, CodexReplay::Discover) {
        // Unbounded runs select every available file, so the metadata covers
        // the whole plan in its order.
        let (forks, metadata): (Vec<_>, Vec<_>) = metadata_by_group.into_iter().unzip();
        let plan = CodexReplayPlan::with_metadata(
            available.iter().copied(),
            metadata.into_iter().flatten().collect(),
            shared.single_thread,
            diagnostics,
        );
        let mut file_offset = 0;
        for (&(sessions_dir, files), forks) in selected.iter().zip(&forks) {
            if !forks.is_empty() {
                let run = CodexAggregateRun {
                    sessions_dir,
                    files,
                    shared,
                    kind,
                    replay: CodexReplay::Planned(&plan),
                    selection: Some(forks),
                    file_offset,
                    diagnostics,
                };
                merge_groups(&mut groups, aggregate_files_with_dedupe(&run, &seen)?.0);
            }
            file_offset += files.len();
        }
    }
    apply_recorded_usage_from_shards(&mut groups, &seen, shared, kind);
    Ok(groups)
}

fn aggregate_files_with_dedupe(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<(BTreeMap<String, CodexGroup>, CodexFileMetadata)> {
    if run.shared.single_thread {
        return aggregate_files(run, seen);
    }
    aggregate_files_parallel(run, seen)
}

fn aggregate_files(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<(BTreeMap<String, CodexGroup>, CodexFileMetadata)> {
    let mut groups = BTreeMap::new();
    let mut metadata = Vec::new();
    let timezone = resolve_tz(run.shared.timezone.as_deref());
    for index in run.file_indexes() {
        aggregate_file(run, index, &timezone, seen, &mut groups, &mut metadata)?;
    }
    Ok((groups, metadata))
}

fn aggregate_files_parallel(
    run: &CodexAggregateRun<'_>,
    seen: &CodexDedupeShards,
) -> Result<(BTreeMap<String, CodexGroup>, CodexFileMetadata)> {
    let file_count = run.selection.map_or(run.files.len(), <[usize]>::len);
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(file_count);
    if worker_count <= 1 {
        return aggregate_files(run, seen);
    }

    let chunks = run.chunk_file_indexes(worker_count);
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                let mut groups = BTreeMap::new();
                let mut metadata = Vec::new();
                let timezone = resolve_tz(run.shared.timezone.as_deref());
                for index in chunk {
                    aggregate_file(run, index, &timezone, seen, &mut groups, &mut metadata)?;
                }
                Result::<_>::Ok((groups, metadata))
            }));
        }

        let mut groups = BTreeMap::new();
        let mut metadata = Vec::new();
        for handle in handles {
            let (worker_groups, worker_metadata) = handle
                .join()
                .map_err(|_| crate::cli_error("codex worker panicked"))??;
            merge_groups(&mut groups, worker_groups);
            metadata.extend(worker_metadata);
        }
        Ok((groups, metadata))
    })
}

fn aggregate_file(
    run: &CodexAggregateRun<'_>,
    file_index: usize,
    timezone: &JiffTimeZone,
    seen: &CodexDedupeShards,
    groups: &mut BTreeMap<String, CodexGroup>,
    metadata: &mut CodexFileMetadata,
) -> Result<()> {
    let mut provenance = CodexEventProvenance::new(run.file_offset + file_index);
    let add = |event: &CodexTokenUsageEvent| {
        add_event_to_groups(
            event,
            run.kind,
            timezone,
            run.shared,
            seen,
            groups,
            &mut provenance,
        )
    };
    visit_run_file(run, file_index, metadata, add)
}

/// Visits one file of `run`, recording its metadata when the run discovers
/// forks as it goes.
fn visit_run_file(
    run: &CodexAggregateRun<'_>,
    file_index: usize,
    metadata: &mut CodexFileMetadata,
    mut add: impl FnMut(&CodexTokenUsageEvent) -> Result<()>,
) -> Result<()> {
    let file = &run.files[file_index];
    match run.replay {
        CodexReplay::Planned(plan) => parser::visit_codex_session_file_with_diagnostics(
            run.sessions_dir,
            file,
            plan.fork_replay(file),
            |event| {
                if plan.is_replayed_compaction(file, event.response_id.as_deref()) {
                    return Ok(());
                }
                add(&event)
            },
            run.diagnostics,
        ),
        CodexReplay::Discover => {
            let file_metadata = parser::visit_codex_session_file_unless_fork(
                run.sessions_dir,
                file,
                |event| add(&event),
                run.diagnostics,
            )?;
            metadata.push((file_index, file_metadata));
            Ok(())
        }
    }
}

fn aggregate_files_local(
    sessions_dir: &Path,
    files: &[PathBuf],
    all_files: &[PathBuf],
    shared: &SharedArgs,
    kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let bounded_plan = has_date_bounds(shared).then(|| {
        CodexReplayPlan::for_bounded_files(
            [(sessions_dir, files)],
            [(sessions_dir, all_files)],
            shared.single_thread,
            diagnostics,
        )
    });
    let run = CodexAggregateRun {
        sessions_dir,
        files,
        shared,
        kind,
        replay: bounded_plan
            .as_ref()
            .map_or(CodexReplay::Discover, CodexReplay::Planned),
        selection: None,
        file_offset: 0,
        diagnostics,
    };
    let (mut aggregation, metadata) = aggregate_files_local_with_seen(&run, None)?;
    if matches!(run.replay, CodexReplay::Discover) {
        let (forks, metadata) = fork_indexes(files.len(), metadata);
        let plan = CodexReplayPlan::with_metadata(
            [(sessions_dir, all_files)],
            metadata,
            true,
            run.diagnostics,
        );
        let fork_run = CodexAggregateRun {
            replay: CodexReplay::Planned(&plan),
            selection: Some(&forks),
            ..run
        };
        aggregation = aggregate_files_local_with_seen(&fork_run, Some(aggregation))?.0;
    }
    let CodexAggregation { mut groups, seen } = aggregation;
    apply_recorded_usage_entries(&mut groups, seen.iter(), shared, kind);
    Ok(groups)
}

fn aggregate_files_local_with_seen(
    run: &CodexAggregateRun<'_>,
    aggregation: Option<CodexAggregation>,
) -> Result<(CodexAggregation, CodexFileMetadata)> {
    let mut aggregation = aggregation.unwrap_or_else(|| CodexAggregation {
        groups: BTreeMap::new(),
        seen: FxHashMap::default(),
    });
    let mut metadata = Vec::new();
    let timezone = resolve_tz(run.shared.timezone.as_deref());
    for index in run.file_indexes() {
        aggregate_file_local(run, index, &timezone, &mut aggregation, &mut metadata)?;
    }
    Ok((aggregation, metadata))
}

fn aggregate_file_local(
    run: &CodexAggregateRun<'_>,
    file_index: usize,
    timezone: &JiffTimeZone,
    aggregation: &mut CodexAggregation,
    metadata: &mut CodexFileMetadata,
) -> Result<()> {
    let mut provenance = CodexEventProvenance::new(run.file_offset + file_index);
    let add = |event: &CodexTokenUsageEvent| {
        add_event_to_groups_local(
            event,
            run.kind,
            timezone,
            run.shared,
            aggregation,
            &mut provenance,
        )
    };
    visit_run_file(run, file_index, metadata, add)
}

fn add_event_to_groups(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: &JiffTimeZone,
    shared: &SharedArgs,
    seen: &CodexDedupeShards,
    groups: &mut BTreeMap<String, CodexGroup>,
    provenance: &mut CodexEventProvenance,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    if !insert_event_key(event, timestamp, model.as_ref(), kind, seen, provenance) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        groups,
    )
}

fn add_event_to_groups_local(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: &JiffTimeZone,
    shared: &SharedArgs,
    aggregation: &mut CodexAggregation,
    provenance: &mut CodexEventProvenance,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    let key = provenance.event_key(event, timestamp, model.as_ref(), kind);
    if !insert_dedupe_record(
        &mut aggregation.seen,
        key,
        event,
        model.as_ref(),
        kind,
        timestamp,
        provenance.file_index,
    ) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        &mut aggregation.groups,
    )
}

fn add_deduped_event_to_groups(
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    kind: AgentReportKind,
    timezone: &JiffTimeZone,
    shared: &SharedArgs,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let Some(period) = codex_period_for(
        timestamp,
        Some(event.session_id.as_str()),
        kind,
        timezone,
        shared,
    ) else {
        return Ok(());
    };
    let group = groups.entry(period).or_default();
    accumulate_codex_event_into_group(group, event, model, timestamp, false);
    Ok(())
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

fn accumulate_codex_event_into_group(
    group: &mut CodexGroup,
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    record_service_tier: bool,
) {
    group.input_tokens += event.input_tokens;
    group.cached_input_tokens += event.cached_input_tokens;
    group.cache_creation_tokens += event.cache_creation_tokens;
    group.output_tokens += event.output_tokens;
    group.reasoning_output_tokens += event.reasoning_output_tokens;
    group.total_tokens += event.total_tokens;
    if group
        .last_activity
        .as_deref()
        .is_none_or(|current| event.timestamp.as_str() > current)
    {
        group.last_activity = Some(event.timestamp.clone());
    }

    let model_usage = group.models.entry(model.to_string()).or_default();
    accumulate_codex_event_into_model_usage(
        model_usage,
        event,
        model,
        timestamp,
        record_service_tier,
    );
}

fn accumulate_codex_event_into_model_usage(
    model_usage: &mut crate::CodexModelUsage,
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    record_service_tier: bool,
) {
    model_usage.input_tokens += event.input_tokens;
    model_usage.cached_input_tokens += event.cached_input_tokens;
    model_usage.cache_creation_tokens += event.cache_creation_tokens;
    model_usage.output_tokens += event.output_tokens;
    model_usage.reasoning_output_tokens += event.reasoning_output_tokens;
    model_usage.total_tokens += event.total_tokens;
    // Each event is one request, so its input size decides the pricing tier
    // here; the summed totals cannot recover per-request context sizes. The
    // boundary is per model (OpenAI's 272K models and any future tier with a
    // different threshold) rather than a single global constant, matching the
    // threshold used to price the long-context buckets.
    let is_long_context = event.input_tokens > crate::pricing::long_context_split_threshold(model);
    if is_long_context {
        model_usage.long_context_input_tokens += event.input_tokens;
        model_usage.long_context_cached_input_tokens += event.cached_input_tokens;
        model_usage.long_context_cache_creation_tokens += event.cache_creation_tokens;
        model_usage.long_context_output_tokens += event.output_tokens;
    }
    if crate::has_time_dependent_pricing(model) {
        let timestamped_usage = model_usage
            .timestamped_usage
            .entry(timestamp.as_millis())
            .or_default();
        accumulate_codex_event_into_usage_bucket(
            &mut timestamped_usage.usage,
            event,
            is_long_context,
        );
        if record_service_tier {
            let recorded_usage = match event.service_tier {
                Some(CodexServiceTier::Standard) => {
                    Some(&mut timestamped_usage.recorded_standard_usage)
                }
                Some(CodexServiceTier::Fast) => Some(&mut timestamped_usage.recorded_fast_usage),
                Some(CodexServiceTier::Flex) => Some(&mut timestamped_usage.recorded_flex_usage),
                Some(CodexServiceTier::Ultrafast) => {
                    Some(&mut timestamped_usage.recorded_ultrafast_usage)
                }
                None => None,
            };
            if let Some(recorded_usage) = recorded_usage {
                accumulate_codex_event_into_usage_bucket(recorded_usage, event, is_long_context);
            }
        }
    }
    if record_service_tier {
        let recorded_usage = match event.service_tier {
            Some(CodexServiceTier::Standard) => Some(&mut model_usage.recorded_standard_usage),
            Some(CodexServiceTier::Fast) => Some(&mut model_usage.recorded_fast_usage),
            Some(CodexServiceTier::Flex) => Some(&mut model_usage.recorded_flex_usage),
            Some(CodexServiceTier::Ultrafast) => Some(&mut model_usage.recorded_ultrafast_usage),
            None => None,
        };
        if let Some(recorded_usage) = recorded_usage {
            accumulate_codex_event_into_usage_bucket(recorded_usage, event, is_long_context);
        }
    }
    model_usage.is_fallback |= event.is_fallback_model;
}

fn accumulate_codex_event_into_usage_bucket(
    usage: &mut CodexUsageBucket,
    event: &CodexTokenUsageEvent,
    is_long_context: bool,
) {
    usage.input_tokens += event.input_tokens;
    usage.cached_input_tokens += event.cached_input_tokens;
    usage.cache_creation_tokens += event.cache_creation_tokens;
    usage.output_tokens += event.output_tokens;
    if is_long_context {
        usage.long_context_input_tokens += event.input_tokens;
        usage.long_context_cached_input_tokens += event.cached_input_tokens;
        usage.long_context_cache_creation_tokens += event.cache_creation_tokens;
        usage.long_context_output_tokens += event.output_tokens;
    }
}

fn merge_codex_usage_bucket(target: &mut CodexUsageBucket, source: CodexUsageBucket) {
    target.input_tokens += source.input_tokens;
    target.cached_input_tokens += source.cached_input_tokens;
    target.cache_creation_tokens += source.cache_creation_tokens;
    target.output_tokens += source.output_tokens;
    target.long_context_input_tokens += source.long_context_input_tokens;
    target.long_context_cached_input_tokens += source.long_context_cached_input_tokens;
    target.long_context_cache_creation_tokens += source.long_context_cache_creation_tokens;
    target.long_context_output_tokens += source.long_context_output_tokens;
}

fn merge_recorded_codex_usage(
    model_usage: &mut crate::CodexModelUsage,
    model: &str,
    timestamp: crate::TimestampMs,
    service_tier: CodexServiceTier,
    usage: CodexUsageBucket,
) {
    let recorded_usage = match service_tier {
        CodexServiceTier::Standard => &mut model_usage.recorded_standard_usage,
        CodexServiceTier::Fast => &mut model_usage.recorded_fast_usage,
        CodexServiceTier::Flex => &mut model_usage.recorded_flex_usage,
        CodexServiceTier::Ultrafast => &mut model_usage.recorded_ultrafast_usage,
    };
    merge_codex_usage_bucket(recorded_usage, usage);

    if crate::has_time_dependent_pricing(model) {
        let timestamped_usage = model_usage
            .timestamped_usage
            .entry(timestamp.as_millis())
            .or_default();
        let recorded_usage = match service_tier {
            CodexServiceTier::Standard => &mut timestamped_usage.recorded_standard_usage,
            CodexServiceTier::Fast => &mut timestamped_usage.recorded_fast_usage,
            CodexServiceTier::Flex => &mut timestamped_usage.recorded_flex_usage,
            CodexServiceTier::Ultrafast => &mut timestamped_usage.recorded_ultrafast_usage,
        };
        merge_codex_usage_bucket(recorded_usage, usage);
    }
}

fn apply_recorded_usage_from_shards(
    groups: &mut BTreeMap<String, CodexGroup>,
    seen: &CodexDedupeShards,
    shared: &SharedArgs,
    kind: AgentReportKind,
) {
    for shard in seen {
        let records = shard.lock().unwrap();
        apply_recorded_usage_entries(groups, records.iter(), shared, kind);
    }
}

fn apply_recorded_usage_entries<'a>(
    groups: &mut BTreeMap<String, CodexGroup>,
    records: impl IntoIterator<Item = (&'a CodexEventKey, &'a CodexDedupeRecord)>,
    shared: &SharedArgs,
    kind: AgentReportKind,
) {
    let timezone = resolve_tz(shared.timezone.as_deref());
    for (key, record) in records {
        if let Some(compaction) = &record.compaction {
            // Workers may encounter a later file first. Compaction usage is
            // added only after every worker has selected the first file's copy.
            let mut event = compaction.event.clone();
            event.service_tier = record.service_tier;
            if let Some(period) = codex_period_for(
                compaction.timestamp,
                Some(&event.session_id),
                kind,
                &timezone,
                shared,
            ) {
                accumulate_codex_event_into_group(
                    groups.entry(period).or_default(),
                    &event,
                    &record.model,
                    compaction.timestamp,
                    true,
                );
            }
            continue;
        }
        let CodexEventKey::Usage(key) = key else {
            continue;
        };
        let Some(service_tier) = record.service_tier else {
            continue;
        };
        let Some(period) = codex_period_for(
            key.timestamp,
            record.session_id.as_deref(),
            kind,
            &timezone,
            shared,
        ) else {
            continue;
        };
        let Some(group) = groups.get_mut(&period) else {
            continue;
        };
        let Some(model_usage) = group.models.get_mut(record.model.as_str()) else {
            continue;
        };
        let is_long_context =
            key.input_tokens > crate::pricing::long_context_split_threshold(record.model.as_str());
        let usage = CodexUsageBucket {
            input_tokens: key.input_tokens,
            cached_input_tokens: key.cached_input_tokens,
            cache_creation_tokens: key.cache_creation_tokens,
            output_tokens: key.output_tokens,
            long_context_input_tokens: if is_long_context { key.input_tokens } else { 0 },
            long_context_cached_input_tokens: if is_long_context {
                key.cached_input_tokens
            } else {
                0
            },
            long_context_cache_creation_tokens: if is_long_context {
                key.cache_creation_tokens
            } else {
                0
            },
            long_context_output_tokens: if is_long_context {
                key.output_tokens
            } else {
                0
            },
        };
        merge_recorded_codex_usage(
            model_usage,
            record.model.as_str(),
            key.timestamp,
            service_tier,
            usage,
        );
    }
}

fn create_dedupe_shards() -> Vec<Mutex<CodexDedupeMap>> {
    let shard_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    (0..shard_count.max(1))
        .map(|_| Mutex::new(FxHashMap::default()))
        .collect()
}

fn insert_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
    seen: &CodexDedupeShards,
    provenance: &mut CodexEventProvenance,
) -> bool {
    let key = provenance.event_key(event, timestamp, model, kind);
    let mut hasher = FxHasher::default();
    key.hash(&mut hasher);
    let shard_index = hasher.finish() as usize % seen.len();
    insert_dedupe_record(
        &mut seen[shard_index].lock().unwrap(),
        key,
        event,
        model,
        kind,
        timestamp,
        provenance.file_index,
    )
}

fn insert_dedupe_record(
    seen: &mut CodexDedupeMap,
    key: CodexEventKey,
    event: &CodexTokenUsageEvent,
    model: &str,
    kind: AgentReportKind,
    timestamp: crate::TimestampMs,
    file_index: usize,
) -> bool {
    if let Some(record) = seen.get_mut(&key) {
        record.service_tier = merge_codex_service_tiers(record.service_tier, event.service_tier);
        if let Some(compaction) = record.compaction.as_mut()
            && file_index < compaction.file_index
        {
            compaction.file_index = file_index;
            compaction.timestamp = timestamp;
            compaction.event = event.clone();
            record.model = CompactString::new(model);
            record.session_id =
                (kind == AgentReportKind::Session).then(|| CompactString::new(&event.session_id));
        }
        return false;
    }
    seen.insert(
        key,
        CodexDedupeRecord {
            service_tier: event.service_tier,
            model: CompactString::new(model),
            session_id: (kind == AgentReportKind::Session)
                .then(|| CompactString::new(&event.session_id)),
            compaction: event.response_id.as_ref().map(|_| {
                Box::new(CodexCompactionEvent {
                    file_index,
                    timestamp,
                    event: event.clone(),
                })
            }),
        },
    );
    event.response_id.is_none()
}

fn codex_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
) -> CodexEventKey {
    if let Some(response_id) = &event.response_id {
        return CodexEventKey::Response(CompactString::new(response_id));
    }
    let (session_hash, session_len) = if kind == AgentReportKind::Session {
        (hash_text(&event.session_id), event.session_id.len())
    } else {
        (0, 0)
    };
    CodexEventKey::Usage(CodexUsageKey {
        session_hash,
        session_len,
        timestamp,
        model_hash: hash_text(model),
        model_len: model.len(),
        input_tokens: event.input_tokens,
        cached_input_tokens: event.cached_input_tokens,
        cache_creation_tokens: event.cache_creation_tokens,
        output_tokens: event.output_tokens,
        reasoning_output_tokens: event.reasoning_output_tokens,
        total_tokens: event.total_tokens,
        occurrence: 0,
    })
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

fn merge_groups(target: &mut BTreeMap<String, CodexGroup>, source: BTreeMap<String, CodexGroup>) {
    for (period, group) in source {
        let target_group = target.entry(period).or_default();
        target_group.input_tokens += group.input_tokens;
        target_group.cached_input_tokens += group.cached_input_tokens;
        target_group.cache_creation_tokens += group.cache_creation_tokens;
        target_group.output_tokens += group.output_tokens;
        target_group.reasoning_output_tokens += group.reasoning_output_tokens;
        target_group.total_tokens += group.total_tokens;
        if target_group.last_activity.as_deref().is_none_or(|current| {
            group
                .last_activity
                .as_deref()
                .is_some_and(|next| next > current)
        }) {
            target_group.last_activity = group.last_activity;
        }
        for (model, usage) in group.models {
            let target_usage = target_group.models.entry(model).or_default();
            merge_codex_model_usage(target_usage, usage);
        }
    }
}

fn merge_codex_model_usage(target: &mut crate::CodexModelUsage, source: crate::CodexModelUsage) {
    target.input_tokens += source.input_tokens;
    target.cached_input_tokens += source.cached_input_tokens;
    target.cache_creation_tokens += source.cache_creation_tokens;
    target.output_tokens += source.output_tokens;
    target.reasoning_output_tokens += source.reasoning_output_tokens;
    target.total_tokens += source.total_tokens;
    target.long_context_input_tokens += source.long_context_input_tokens;
    target.long_context_cached_input_tokens += source.long_context_cached_input_tokens;
    target.long_context_cache_creation_tokens += source.long_context_cache_creation_tokens;
    target.long_context_output_tokens += source.long_context_output_tokens;
    merge_codex_usage_bucket(
        &mut target.recorded_standard_usage,
        source.recorded_standard_usage,
    );
    merge_codex_usage_bucket(&mut target.recorded_fast_usage, source.recorded_fast_usage);
    merge_codex_usage_bucket(&mut target.recorded_flex_usage, source.recorded_flex_usage);
    merge_codex_usage_bucket(
        &mut target.recorded_ultrafast_usage,
        source.recorded_ultrafast_usage,
    );
    for (timestamp, usage) in source.timestamped_usage {
        let target_usage = target.timestamped_usage.entry(timestamp).or_default();
        merge_codex_usage_bucket(&mut target_usage.usage, usage.usage);
        merge_codex_usage_bucket(
            &mut target_usage.recorded_standard_usage,
            usage.recorded_standard_usage,
        );
        merge_codex_usage_bucket(
            &mut target_usage.recorded_fast_usage,
            usage.recorded_fast_usage,
        );
        merge_codex_usage_bucket(
            &mut target_usage.recorded_flex_usage,
            usage.recorded_flex_usage,
        );
        merge_codex_usage_bucket(
            &mut target_usage.recorded_ultrafast_usage,
            usage.recorded_ultrafast_usage,
        );
    }
    target.is_fallback |= source.is_fallback;
}

pub fn aggregate_events(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
) -> Result<BTreeMap<String, CodexGroup>> {
    let mut groups = BTreeMap::new();
    let mut seen_compaction_responses = FxHashSet::default();
    let mut compaction_tiers = FxHashMap::default();
    // Match loader and streaming aggregation: a copied record can supply a
    // tier missing from the first copy, without changing its usage or owner.
    for event in events {
        if let Some(response_id) = event.response_id.as_deref()
            && event
                .model
                .as_deref()
                .is_some_and(|model| !model.is_empty())
        {
            let tier = compaction_tiers.entry(response_id).or_insert(None);
            *tier = merge_codex_service_tiers(*tier, event.service_tier);
        }
    }
    let timezone = resolve_tz(timezone);
    for event in events {
        let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
            continue;
        };
        if let Some(response_id) = event.response_id.as_deref()
            && !seen_compaction_responses.insert(response_id)
        {
            continue;
        }
        let merged_event = event.response_id.as_deref().map(|response_id| {
            let mut merged = event.clone();
            merged.service_tier = compaction_tiers[response_id];
            merged
        });
        let event = merged_event.as_ref().unwrap_or(event);
        let timestamp = parse_ts_timestamp(&event.timestamp).ok_or_else(|| {
            crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp))
        })?;
        let date = format_date_tz(timestamp, &timezone);
        let period = match kind {
            AgentReportKind::Daily => date,
            AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
            AgentReportKind::Monthly => date[..7].to_string(),
            AgentReportKind::Session => event.session_id.clone(),
        };
        let group = groups.entry(period).or_insert_with(CodexGroup::default);
        let model = crate::model_aliases::resolve_model_name(model);
        accumulate_codex_event_into_group(group, event, model.as_ref(), timestamp, true);
    }
    Ok(groups)
}

pub fn filter_events_by_date(
    events: &mut Vec<CodexTokenUsageEvent>,
    shared: &SharedArgs,
) -> Result<()> {
    if shared.since.is_none() && shared.until.is_none() {
        return Ok(());
    }
    let timezone = resolve_tz(shared.timezone.as_deref());
    let mut kept = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let timestamp = parse_ts_timestamp(&event.timestamp).ok_or_else(|| {
            crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp))
        })?;
        let date = format_date_tz(timestamp, &timezone).replace('-', "");
        if shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
        {
            kept.push(event);
        }
    }
    *events = kept;
    Ok(())
}

#[cfg(test)]
pub(super) fn load_groups_from_directory(
    sessions_dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    load_groups_from_directory_with_diagnostics(
        sessions_dir,
        shared,
        kind,
        &Diagnostics::new("codex"),
    )
    .map(|(groups, _)| groups)
}

#[cfg(test)]
fn load_groups_from_sources(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    load_groups_from_sources_with_diagnostics(sources, shared, kind, &Diagnostics::new("codex"))
        .map(|(groups, _)| groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use crate::{
        CodexModelUsage, PricingMap, cli::CodexSpeed, model_aliases::set_model_aliases_for_tests,
        paths::CodexUsageSource, replay,
    };

    #[test]
    fn compaction_attribution_uses_file_order_when_workers_finish_in_reverse() {
        let first = CodexTokenUsageEvent {
            session_id: "first-session".into(),
            response_id: Some("response-1".into()),
            timestamp: "2026-09-01T00:00:01Z".into(),
            model: Some("gpt-5".into()),
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
        copied.timestamp = "2026-09-02T00:00:01Z".into();
        copied.model = Some("gpt-5-mini".into());
        copied.input_tokens = 500;
        copied.total_tokens = 520;
        copied.service_tier = Some(CodexServiceTier::Fast);
        let shared = SharedArgs {
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let seen = create_dedupe_shards();
        let mut groups = BTreeMap::new();
        for (event, file_index) in [(&copied, 1), (&first, 0)] {
            add_event_to_groups(
                event,
                AgentReportKind::Session,
                &jiff::tz::TimeZone::system(),
                &shared,
                &seen,
                &mut groups,
                &mut CodexEventProvenance::new(file_index),
            )
            .unwrap();
        }
        apply_recorded_usage_from_shards(&mut groups, &seen, &shared, AgentReportKind::Session);

        assert_eq!(groups.len(), 1);
        let group = &groups["first-session"];
        assert_eq!(group.total_tokens, 320);
        assert_eq!(group.last_activity.as_deref(), Some("2026-09-01T00:00:01Z"));
        assert_eq!(
            group.models["gpt-5"].recorded_standard_usage.input_tokens,
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
        let event = |model: &str| CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            response_id: None,
            timestamp: "2026-08-17T01:00:00.000Z".to_string(),
            model: Some(model.to_string()),
            input_tokens: 1_000_000,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1_000_000,
            is_fallback_model: false,
            service_tier: None,
        };
        let groups = aggregate_events(
            &[event("gpt-5"), event("deepseek-v4-flash")],
            AgentReportKind::Daily,
            Some("UTC"),
        )
        .unwrap();
        let models = &groups["2026-08-17"].models;

        assert!(models["gpt-5"].timestamped_usage.is_empty());
        assert_eq!(models["deepseek-v4-flash"].timestamped_usage.len(), 1);
    }

    #[test]
    fn records_flex_usage_in_model_buckets() {
        let event = CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            timestamp: "2026-08-17T01:00:00.000Z".to_string(),
            model: Some("gpt-test".to_string()),
            input_tokens: 1_000,
            cached_input_tokens: 100,
            cache_creation_tokens: 50,
            output_tokens: 25,
            reasoning_output_tokens: 5,
            total_tokens: 1_025,
            is_fallback_model: false,
            service_tier: Some(CodexServiceTier::Flex),
            response_id: None,
        };

        let groups = aggregate_events(&[event], AgentReportKind::Daily, Some("UTC")).unwrap();
        let usage = &groups["2026-08-17"].models["gpt-test"];

        assert_eq!(usage.input_tokens, 1_000);
        assert_eq!(usage.recorded_flex_usage.input_tokens, 1_000);
        assert_eq!(usage.recorded_flex_usage.cached_input_tokens, 100);
        assert_eq!(usage.recorded_flex_usage.cache_creation_tokens, 50);
        assert_eq!(usage.recorded_flex_usage.output_tokens, 25);
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
        crate::paths::set_file_modified(
            &historical_path,
            parse_ts_timestamp("2025-01-01T08:01:00.000Z").unwrap(),
        );
        crate::paths::set_file_modified(
            &long_running_path,
            parse_ts_timestamp("2026-03-15T08:01:00.000Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for single_thread in [true, false] {
            let _ = replay::take_observed_file_read_events();
            let bounded = load_groups_from_directory(
                &fixture.path("sessions"),
                &SharedArgs {
                    single_thread,
                    ..shared.clone()
                },
                AgentReportKind::Daily,
            )
            .unwrap();
            let group = &bounded["2026-03-15"];
            assert_eq!(group.input_tokens, 100);
            assert_eq!(group.output_tokens, 1);
            assert_eq!(group.total_tokens, 101);
            if single_thread {
                let reads = replay::take_observed_file_read_events();
                assert!(reads.iter().any(|read| matches!(
                    read,
                    replay::ObservedFileRead::MetadataProbe(path) if path == &long_running_path
                )));
                assert!(reads.iter().any(|read| matches!(
                    read,
                    replay::ObservedFileRead::MetadataProbe(path) if path == &historical_path
                )));
                assert!(!reads.iter().any(|read| matches!(
                    read,
                    replay::ObservedFileRead::ParentUsage(path) if path == &historical_path
                )));
            }

            let unbounded = load_groups_from_directory(
                &fixture.path("sessions"),
                &SharedArgs {
                    single_thread,
                    ..SharedArgs::default()
                },
                AgentReportKind::Daily,
            )
            .unwrap();
            assert_eq!(unbounded["2026-03-15"].input_tokens, 1_099);
        }
    }

    #[test]
    fn retains_next_utc_path_day_for_local_until_boundary() {
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
        let late_utc_path = fixture.path("sessions/2026/03/16/late-utc.jsonl");
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };

        let _ = replay::take_observed_file_reads();
        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        assert_eq!(groups["2026-03-15"].input_tokens, 100);
        assert_eq!(groups["2026-03-15"].total_tokens, 101);
        assert!(replay::take_observed_file_reads().contains(&late_utc_path));
    }

    #[test]
    fn dedupes_copied_token_usage_across_session_files() {
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
                    group.models["gpt-5.2"].recorded_fast_usage.input_tokens,
                    1_000
                );
                costs.push(crate::calculate_group_cost(
                    group,
                    &pricing,
                    CodexSpeed::Auto,
                ));
            }
        }
        assert!(costs.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn resolves_conflicting_duplicate_tiers_as_standard() {
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

                assert_eq!(usage.input_tokens, 1_000);
                assert_eq!(usage.recorded_standard_usage.input_tokens, 1_000);
                assert_eq!(usage.recorded_fast_usage.input_tokens, 0);
            }
        }
    }

    #[test]
    fn tracks_long_context_token_split_per_request() {
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
        // One request above the 272K input threshold and one below it.
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
        assert_eq!(usage.input_tokens, 380_000);
        assert_eq!(usage.cached_input_tokens, 70_000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.long_context_input_tokens, 280_000);
        assert_eq!(usage.long_context_cached_input_tokens, 20_000);
        assert_eq!(usage.long_context_output_tokens, 500);
        assert_eq!(usage.recorded_fast_usage.input_tokens, 280_000);
        assert_eq!(usage.recorded_fast_usage.cached_input_tokens, 20_000);
        assert_eq!(usage.recorded_fast_usage.output_tokens, 500);
        assert_eq!(usage.recorded_fast_usage.long_context_input_tokens, 280_000);
        assert_eq!(
            usage.recorded_fast_usage.long_context_cached_input_tokens,
            20_000
        );
        assert_eq!(usage.recorded_fast_usage.long_context_output_tokens, 500);
        assert_eq!(usage.recorded_standard_usage.input_tokens, 100_000);
        assert_eq!(usage.recorded_standard_usage.cached_input_tokens, 50_000);
        assert_eq!(usage.recorded_standard_usage.output_tokens, 300);
    }

    #[test]
    fn parallel_merge_preserves_speed_and_long_context_buckets() {
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

            assert_eq!(usage.input_tokens, 380_000);
            assert_eq!(usage.cache_creation_tokens, 40_000);
            assert_eq!(usage.long_context_input_tokens, 280_000);
            assert_eq!(usage.long_context_cache_creation_tokens, 30_000);
            assert_eq!(usage.recorded_fast_usage.input_tokens, 280_000);
            assert_eq!(usage.recorded_fast_usage.cache_creation_tokens, 30_000);
            assert_eq!(usage.recorded_fast_usage.long_context_input_tokens, 280_000);
            assert_eq!(
                usage.recorded_fast_usage.long_context_cache_creation_tokens,
                30_000
            );
            assert_eq!(usage.recorded_standard_usage.input_tokens, 100_000);
            assert_eq!(usage.recorded_standard_usage.cache_creation_tokens, 10_000);
            assert_eq!(usage.recorded_standard_usage.long_context_input_tokens, 0);
            observed.push((usage.recorded_fast_usage, usage.recorded_standard_usage));
        }

        assert_eq!(observed[0], observed[1]);
    }

    #[test]
    fn merges_cache_creation_usage_into_groups_and_recorded_buckets() {
        let usage = CodexModelUsage {
            cache_creation_tokens: 30,
            long_context_cache_creation_tokens: 20,
            recorded_standard_usage: CodexUsageBucket {
                cache_creation_tokens: 30,
                long_context_cache_creation_tokens: 20,
                ..CodexUsageBucket::default()
            },
            ..CodexModelUsage::default()
        };
        let mut source_group = CodexGroup {
            cache_creation_tokens: 30,
            ..CodexGroup::default()
        };
        source_group.models.insert("gpt-test".to_string(), usage);
        let source = BTreeMap::from([("2026-07-09".to_string(), source_group)]);
        let mut target = BTreeMap::new();

        merge_groups(&mut target, source);

        let merged = &target["2026-07-09"];
        let merged_usage = &merged.models["gpt-test"];
        assert_eq!(merged.cache_creation_tokens, 30);
        assert_eq!(merged_usage.cache_creation_tokens, 30);
        assert_eq!(
            merged_usage
                .recorded_standard_usage
                .long_context_cache_creation_tokens,
            20
        );
    }

    #[test]
    fn dedupes_copied_token_usage_after_model_alias_resolution() {
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
            assert_eq!(group.models["gpt-5.2"].input_tokens, 1_000);
        }
    }

    #[test]
    fn keeps_matching_token_usage_in_distinct_session_groups() {
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
                CodexUsageSource::new_for_test(
                    fixture.path("codex/sessions"),
                    fixture.path("codex"),
                ),
                CodexUsageSource::new_for_test(
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

    fn token_count_line(timestamp: &str, last: u64, total: u64) -> String {
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
        let parent = [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "parent-xyz"},
            })
            .to_string(),
            token_count_line("2026-05-12T08:01:00.000Z", 1_000, 1_000),
            token_count_line("2026-05-12T08:02:00.000Z", 500, 1_500),
        ]
        .join("\n");
        let fork = [
            json!({
                "timestamp": "2026-05-12T08:03:00.000Z",
                "type": "session_meta",
                "payload": {"id": "fork-abc", "forked_from_id": "parent-xyz"},
            })
            .to_string(),
            token_count_line("2026-05-12T08:03:00.000Z", 1_000, 1_000),
            token_count_line("2026-05-12T08:03:00.000Z", 500, 1_500),
            token_count_line("2026-05-12T08:04:00.000Z", 100, 1_600),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "codex/sessions/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent.clone(),
            "codex/archived_sessions/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork.clone(),
            "single/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent,
            "single/2026/05/12/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork,
        });
        let sources = [
            CodexUsageSource::new_for_test(fixture.path("codex/sessions"), fixture.path("codex")),
            CodexUsageSource::new_for_test(
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

    /// The rollout from #2046: a spawned subagent whose every line carries the
    /// fork instant, with two requests of identical usage.
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
        let fixture = fs_fixture!({
            "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
                IDENTICAL_REQUESTS_ROLLOUT,
        });
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            let events = crate::load_codex_events_from_directory(&sessions, single_thread)
                .unwrap()
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
                let events =
                    crate::load_codex_events_from_directory(&sessions, single_thread).unwrap();
                assert_eq!(events.len(), 2, "{name}, single_thread={single_thread}");

                assert_eq!(
                    daily_input_tokens(&sessions, single_thread),
                    [("2026-11-01".to_string(), 2_000)],
                    "{name}, single_thread={single_thread}"
                );
            }
        }
    }
}
