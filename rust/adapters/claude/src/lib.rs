use ccusage_adapter_common::{
    read_discovered_paths,
    source::{ProgressOwner, UsageLoadContext},
};
use ccusage_core::{
    Diagnostics, LoadedEntry, MILLIS_PER_DAY, ModelBreakdown, ReportWindow, Result, Speed,
    TimestampMs, TokenCounts, TokenUsageRaw, UsageSummary, cli, cli_error, cost, date_at,
    date_range_bounds_ms, debug_log, fast, model_aliases, parse_ts_timestamp, pricing, progress,
    resolve_tz,
};

mod blocks;
mod commands;
mod daily;
mod diagnostics;
mod paths;
mod source;
mod usage_wire;
use usage_wire::IteratedTokenUsage;

pub use commands::{run_blocks, run_bucket, run_daily, run_session, run_statusline};
pub use source::SOURCE;

use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem;
use rustc_hash::FxHasher;
use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeOwned, DeserializeSeed, MapAccess, Visitor,
        value::{BorrowedStrDeserializer, MapAccessDeserializer, StringDeserializer},
    },
};
use smallvec::SmallVec;

use ccusage_core::{
    cli::{CostMode, SharedArgs},
    fast::{FxHashMap, LinePrefilter},
};

use crate::daily::DedupEntry as _;

pub(crate) use paths::{
    claude_paths, extract_session_parts, usage_files_before_since, visit_usage_files,
};

struct DedupeIndex {
    index: usize,
    session_alias: Option<Arc<str>>,
}

type DedupeIndexVec = SmallVec<[DedupeIndex; 1]>;

/// Loads every deduplicated Claude usage entry, regardless of `since`.
///
/// Use this when a report reads history before the `--since` window, such as
/// billing blocks anchored on earlier entries or whole-session totals.
pub fn load_entries(shared: &SharedArgs, project_filter: Option<&str>) -> Result<Vec<LoadedEntry>> {
    load_deduped_entries(shared, project_filter).map(|deduped| {
        deduped
            .entries
            .into_iter()
            .map(|entry| entry.entry)
            .collect()
    })
}

pub(crate) fn load_deduped_entries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
) -> Result<DedupedEntries> {
    let diagnostics = Diagnostics::new("claude");
    let loaded = load_deduped_entries_with_progress(
        shared,
        project_filter,
        ProgressOwner::Source,
        &diagnostics,
    )?;
    diagnostics.emit(shared.debug);
    Ok(loaded)
}

fn load_deduped_entries_with_progress(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<DedupedEntries> {
    let load = || load_entries_inner(shared, project_filter, false, diagnostics);
    owner.run(progress::UsageLoadAgent("Claude"), shared.json, load)
}

#[derive(Default)]
pub(crate) struct DedupedEntries {
    entries: Vec<ClaudeEntry>,
    detected: bool,
    copies: Vec<(usize, Arc<str>)>,
}

#[derive(Debug, Clone)]
struct ClaudeEntry {
    source_timestamp: String,
    recorded_session_id: Option<String>,
    original_model: Option<String>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    entry: LoadedEntry,
}

impl std::ops::Deref for ClaudeEntry {
    type Target = LoadedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

impl DedupedEntries {
    fn main_thread_sessions(&self) -> MainThreadSessions {
        main_thread_sessions(
            self.entries
                .iter()
                .map(|entry| (loaded_entry_session_id(entry), entry))
                .chain(
                    self.copies
                        .iter()
                        .map(|(index, session)| (session.as_ref(), &self.entries[*index])),
                )
                .map(|(session, entry)| (session, entry.is_sidechain())),
        )
    }
}

pub(crate) fn load_entries_since(
    shared: &SharedArgs,
    project_filter: Option<&str>,
) -> Result<Vec<ClaudeEntry>> {
    let diagnostics = Diagnostics::new("claude");
    let entries = load_entries_since_with_diagnostics(shared, project_filter, &diagnostics)?;
    diagnostics.emit(shared.debug);
    Ok(entries)
}

pub(crate) fn load_entries_since_with_diagnostics(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<Vec<ClaudeEntry>> {
    progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, || {
        load_windowed_entries(shared, project_filter, diagnostics, shared.mode)
    })
    .map(|deduped| deduped.entries)
}

pub(crate) fn load_daily_summaries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    group_by_project: bool,
) -> Result<Vec<UsageSummary>> {
    let diagnostics = Diagnostics::new("claude");
    let daily =
        progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, || {
            daily::load_daily_entries(shared, project_filter, &diagnostics)
        })?;
    diagnostics.emit(shared.debug);
    Ok(daily.summaries(group_by_project, None))
}

pub(crate) type MainThreadSessions = HashSet<Arc<str>>;

fn main_thread_sessions<'a>(entries: impl Iterator<Item = (&'a str, bool)>) -> MainThreadSessions {
    entries
        .filter(|(_, is_sidechain)| !is_sidechain)
        .map(|(session_id, _)| session_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(Arc::from)
        .collect()
}

pub(crate) struct DailySummaries {
    summaries: Vec<UsageSummary>,
    detected: bool,
    sessions: MainThreadSessions,
}

pub(crate) fn load_daily_summaries_with_detection_progress(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    report_window: Option<&crate::ReportWindow>,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<DailySummaries> {
    let load = || daily::load_daily_entries(shared, project_filter, diagnostics);
    let daily = owner.run(progress::UsageLoadAgent("Claude"), shared.json, load)?;
    Ok(DailySummaries {
        summaries: daily.summaries(false, report_window),
        detected: daily.detected,
        sessions: daily.sessions(),
    })
}

fn load_windowed_entries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
    mode: CostMode,
) -> Result<DedupedEntries> {
    let mut loaded = load_entries_with_mode(shared, project_filter, true, diagnostics, mode)?;
    retain_copied_entries(
        &mut loaded.entries,
        &mut loaded.copies,
        &crate::ReportWindow::from_shared(shared),
        |entry| entry.date,
    );
    Ok(loaded)
}

fn retain_copied_entries<T>(
    entries: &mut Vec<T>,
    copies: &mut Vec<(usize, Arc<str>)>,
    window: &crate::ReportWindow,
    date: impl Fn(&T) -> crate::cli::IsoDate,
) {
    if window.is_unbounded() {
        return;
    }
    if copies.is_empty() {
        entries.retain(|entry| window.contains_entry_date(date(entry)));
        return;
    }
    let mut kept_indexes = Vec::with_capacity(entries.len());
    let mut next_index = 0;
    entries.retain(|entry| {
        let keep = window.contains_entry_date(date(entry));
        kept_indexes.push(keep.then_some(next_index));
        if keep {
            next_index += 1;
        }
        keep
    });
    copies.retain_mut(|(index, _)| {
        if let Some(kept_index) = kept_indexes[*index] {
            *index = kept_index;
            true
        } else {
            false
        }
    });
}

fn load_entries_inner(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    skip_files_before_since: bool,
    diagnostics: &Diagnostics,
) -> Result<DedupedEntries> {
    load_entries_with_mode(
        shared,
        project_filter,
        skip_files_before_since,
        diagnostics,
        shared.mode,
    )
}

fn load_entries_with_mode(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    skip_files_before_since: bool,
    diagnostics: &Diagnostics,
    mode: CostMode,
) -> Result<DedupedEntries> {
    let paths = claude_paths()?;
    debug_log(
        shared,
        format!(
            "Scanning Claude data directories: {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    let context = UsageLoadContext::new(shared);
    let read = |file: &PathBuf| {
        read_usage_file_with_diagnostics(
            file,
            context.window().timezone(),
            context.table_for_mode(mode),
            diagnostics,
        )
    };
    let mut pruned = Vec::new();
    let loaded_files = if skip_files_before_since && shared.since.is_some() {
        let split = usage_files_before_since(
            &paths,
            project_filter,
            diagnostics,
            shared,
            shared.clock.now().into(),
        );
        debug_log(
            shared,
            format!(
                "Found {} JSONL usage files",
                split.kept.len() + split.pruned.len()
            ),
        );
        pruned = split.pruned;
        let mut files = split.kept;
        debug_log(
            shared,
            format!(
                "Kept {} JSONL usage files inside the --since window",
                files.len()
            ),
        );
        read_discovered_paths(
            shared.single_thread,
            |visit| std::mem::take(&mut files).into_iter().for_each(visit),
            read,
        )
    } else {
        let loaded_files = read_discovered_paths(
            shared.single_thread,
            |visit| visit_usage_files(&paths, project_filter, visit, diagnostics),
            read,
        );
        debug_log(
            shared,
            format!("Found {} JSONL usage files", loaded_files.len()),
        );
        loaded_files
    };
    if loaded_files.is_empty() {
        return Ok(DedupedEntries {
            detected: pruned.iter().any(|file| daily::has_usage(file)),
            ..DedupedEntries::default()
        });
    }
    let loaded_entry_count = loaded_files
        .iter()
        .map(|(_, entries)| entries.len())
        .sum::<usize>();
    debug_log(
        shared,
        format!(
            "Loaded {loaded_entry_count} usage entries from {} JSONL files",
            loaded_files.len()
        ),
    );
    let (entries, copies) = daily::dedupe_files(loaded_files);
    debug_log(
        shared,
        format!("Kept {} usage entries after deduplication", entries.len()),
    );
    let detected = !entries.is_empty() || pruned.iter().any(|file| daily::has_usage(file));
    Ok(DedupedEntries {
        entries,
        copies,
        detected,
    })
}

// NOTE(2026-03-09, #886): Claude Code bills fast-mode usage at a higher rate. Prefer the fast record when otherwise equal copies disagree on speed.
fn reports_more_of_the_request<E: daily::DedupEntry>(candidate: &E, existing: &E) -> bool {
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
pub(crate) fn is_per_response_message_id(message_id: &str) -> bool {
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

fn loaded_entry_session_id(entry: &ClaudeEntry) -> &str {
    entry
        .recorded_session_id
        .as_deref()
        .unwrap_or(entry.session_id.as_ref())
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

impl daily::DedupEntry for ClaudeEntry {
    fn timestamp(&self) -> TimestampMs {
        self.timestamp
    }

    fn session_id(&self) -> &str {
        loaded_entry_session_id(self)
    }

    fn owned_session_id(&self) -> Arc<str> {
        match self.recorded_session_id.as_deref() {
            Some(session_id) => Arc::from(session_id),
            None => Arc::clone(&self.session_id),
        }
    }

    fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    fn is_sidechain(&self) -> bool {
        self.is_sidechain == Some(true)
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        self.recorded_cost.is_some()
    }
}

fn read_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    pricing: ccusage_core::cost::PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<ClaudeEntry> {
    let (session_id, project_path) = extract_session_parts(path);
    let session_id: Arc<str> = Arc::from(session_id);
    let project_path: Arc<str> = Arc::from(project_path);
    daily::read_usage_file(path, tz, pricing, diagnostics, |parsed, line| {
        let model = daily::ParsedUsage::display_model(parsed.raw_model.clone(), &parsed.usage);
        ClaudeEntry {
            source_timestamp: parsed.timestamp_text,
            recorded_session_id: parsed.recorded_session_id,
            original_model: parsed.raw_model,
            request_id: parsed.key.request_id,
            is_sidechain: parsed.key.is_sidechain,
            entry: LoadedEntry {
                model,
                usage_limit_reset_time: usage_limit_reset_time_from_line_bytes(
                    line,
                    parsed.is_api_error_message,
                ),
                usage: parsed.usage,
                message_id: parsed.key.message_id,
                version: parsed.version,
                recorded_cost: parsed.cost_usd,
                timestamp: parsed.key.timestamp,
                date: parsed.date,
                project: parsed.project,
                session_id: Arc::clone(&session_id),
                project_path: Arc::clone(&project_path),
                cost: parsed.cost,
                extra_total_tokens: 0,
                credits: None,
                message_count: None,
                missing_pricing_model: parsed.missing_pricing_model,
                stored_cost: parsed.stored_cost,
            },
        }
    })
}

#[derive(Debug, Deserialize)]
struct UsageIterationsEnvelope {
    #[serde(default)]
    message: Option<UsageIterationsMessage>,
}

#[derive(Debug, Deserialize)]
struct ProgressIterationsEnvelope {
    data: ProgressIterationsData,
}

#[derive(Debug, Deserialize)]
struct ProgressIterationsData {
    message: UsageIterationsEnvelope,
}

#[derive(Debug, Deserialize)]
struct UsageIterationsMessage {
    #[serde(default)]
    usage: Option<UsageIterations>,
}

#[derive(Debug, Deserialize)]
struct UsageIterations {
    #[serde(default)]
    iterations: Vec<serde_json::Value>,
}

pub(crate) struct AdvisorUsage {
    model: String,
    usage: TokenUsageRaw,
}

#[derive(Default)]
pub(crate) struct AdvisorUsages {
    usages: Vec<AdvisorUsage>,
    malformed: usize,
}

pub(crate) fn advisor_usages_from_line(
    line: &[u8],
) -> std::result::Result<AdvisorUsages, Rejection> {
    static ADVISOR_MARKER: LazyLock<LinePrefilter> =
        LazyLock::new(|| LinePrefilter::all(&[br#""advisor_message""#]));
    if !ADVISOR_MARKER.matches(line) {
        return Ok(AdvisorUsages::default());
    }
    let envelope = deserialize_usage_line::<UsageIterationsEnvelope>(line)?;
    Ok(advisor_usages_from_envelope(envelope))
}

#[cold]
fn advisor_usages_from_progress_line(line: &[u8]) -> std::result::Result<AdvisorUsages, Rejection> {
    let envelope = deserialize_usage_line::<ProgressIterationsEnvelope>(line)?;
    Ok(advisor_usages_from_envelope(envelope.data.message))
}

fn advisor_usages_from_envelope(envelope: UsageIterationsEnvelope) -> AdvisorUsages {
    let iterations = envelope
        .message
        .and_then(|message| message.usage)
        .map(|usage| usage.iterations)
        .unwrap_or_default();
    let mut advisors = AdvisorUsages::default();
    for iteration in &iterations {
        let text = |key: &str| iteration.get(key).and_then(serde_json::Value::as_str);
        // NOTE(2026-09-16, Claude Code 2.1.268, #1719): Claude Code 2.1.266 onward can write a null model on Fable 5.1 main-model iterations. Require a model only for advisor usage.
        let Some(model) = text("model").filter(|model| !model.is_empty()) else {
            continue;
        };
        // NOTE(2026-07-10, #1423): Claude Code reports advisor usage separately from the top-level main-model totals. Count only advisor iterations in this additional pass.
        if text("type") != Some("advisor_message") {
            continue;
        }
        let Ok(usage) = GuardedUsage::deserialize(iteration) else {
            advisors.malformed += 1;
            continue;
        };
        if usage.has_null {
            continue;
        }
        advisors.usages.push(AdvisorUsage {
            model: model.to_owned(),
            usage: usage.usage.usage,
        });
    }
    advisors
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rejection {
    // NOTE(2026-11-16): Claude Code writes synthetic error, refusal and interrupt records with null speed and zero tokens. Skip these quietly to avoid invalid-data warnings during normal operation.
    NullContract,
    Malformed,
}

pub(crate) fn deserialize_usage_line<T: DeserializeOwned>(
    line: &[u8],
) -> std::result::Result<T, Rejection> {
    ccusage_adapter_common::jsonl::parse(line).map_err(|_| Rejection::Malformed)
}

pub(crate) type Nullable<T> = Option<Option<T>>;

fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Nullable<T>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(crate) fn non_null<T>(field: Nullable<T>) -> std::result::Result<Option<T>, Rejection> {
    match field {
        Some(None) => Err(Rejection::NullContract),
        Some(value) => Ok(value),
        None => Ok(None),
    }
}

#[derive(Debug)]
pub(crate) struct GuardedUsage {
    usage: IteratedTokenUsage,
    has_null: bool,
}

impl<'de> Deserialize<'de> for GuardedUsage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct UsageVisitor;
        impl<'de> Visitor<'de> for UsageVisitor {
            type Value = GuardedUsage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a usage object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut fields = UsageFields {
                    map,
                    pending: None,
                    has_null: false,
                };
                let usage =
                    IteratedTokenUsage::deserialize(MapAccessDeserializer::new(&mut fields))?;
                Ok(GuardedUsage {
                    usage,
                    has_null: fields.has_null,
                })
            }
        }
        deserializer.deserialize_map(UsageVisitor)
    }
}

fn is_guarded_usage_member(key: &str) -> bool {
    matches!(
        key,
        "speed" | "cache_creation_input_tokens" | "cache_read_input_tokens"
    )
}

struct UsageFields<A> {
    map: A,
    pending: Option<serde_json::Value>,
    has_null: bool,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for UsageFields<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> std::result::Result<Option<K::Value>, Self::Error> {
        loop {
            let Some(key) = self.map.next_key::<UsageKey<'de>>()? else {
                return Ok(None);
            };
            if !is_guarded_usage_member(key.as_str()) {
                return key.into_seed(seed).map(Some);
            }
            let value = self.map.next_value::<serde_json::Value>()?;
            if value.is_null() {
                self.has_null = true;
            } else {
                self.pending = Some(value);
                return key.into_seed(seed).map(Some);
            }
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(value) => seed.deserialize(value).map_err(serde::de::Error::custom),
            None => self.map.next_value_seed(seed),
        }
    }
}

enum UsageKey<'de> {
    Borrowed(&'de str),
    Owned(String),
}

impl<'de> UsageKey<'de> {
    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(key) => key,
            Self::Owned(key) => key,
        }
    }

    fn into_seed<K: DeserializeSeed<'de>, E: serde::de::Error>(
        self,
        seed: K,
    ) -> std::result::Result<K::Value, E> {
        match self {
            Self::Borrowed(key) => seed.deserialize(BorrowedStrDeserializer::new(key)),
            Self::Owned(key) => seed.deserialize(StringDeserializer::new(key)),
        }
    }
}

impl<'de> Deserialize<'de> for UsageKey<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = UsageKey<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a usage member name")
            }

            fn visit_borrowed_str<E>(self, key: &'de str) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Borrowed(key))
            }

            fn visit_str<E>(self, key: &str) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Owned(key.to_owned()))
            }

            fn visit_string<E>(self, key: String) -> std::result::Result<Self::Value, E> {
                Ok(UsageKey::Owned(key))
            }
        }
        deserializer.deserialize_identifier(KeyVisitor)
    }
}

pub(crate) fn is_semver_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    if !consume_ascii_digits(bytes, &mut index) || bytes.get(index) != Some(&b'.') {
        return false;
    }
    index += 1;
    if !consume_ascii_digits(bytes, &mut index) || bytes.get(index) != Some(&b'.') {
        return false;
    }
    index += 1;
    bytes.get(index).is_some_and(u8::is_ascii_digit)
}

fn consume_ascii_digits(bytes: &[u8], index: &mut usize) -> bool {
    let start = *index;
    while bytes.get(*index).is_some_and(u8::is_ascii_digit) {
        *index += 1;
    }
    *index > start
}

fn usage_limit_reset_time_from_line_bytes(
    line: &[u8],
    is_api_error_message: Option<bool>,
) -> Option<TimestampMs> {
    if is_api_error_message != Some(true) {
        return None;
    }
    let marker = b"Claude AI usage limit reached";
    let marker_start = memmem::find(line, marker)?;
    let timestamp_start = memchr::memchr(b'|', &line[marker_start..])? + marker_start + 1;
    let timestamp_end = line[timestamp_start..]
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .map_or(line.len(), |offset| timestamp_start + offset);
    if timestamp_start == timestamp_end {
        return None;
    }
    let timestamp = std::str::from_utf8(&line[timestamp_start..timestamp_end])
        .ok()?
        .parse::<i64>()
        .ok()?;
    if timestamp <= 0 {
        return None;
    }
    TimestampMs::from_unix_seconds(timestamp)
}

#[cfg(test)]
mod tests;
