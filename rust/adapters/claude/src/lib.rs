use ccusage_adapter_common::{
    read_discovered_files,
    source::{ProgressOwner, UsageLoadContext},
};
use ccusage_core::*;

mod blocks;
mod commands;
mod daily;
mod diagnostics;
#[cfg(test)]
mod load_tests;
mod paths;
mod source;

pub use commands::{run_blocks, run_bucket, run_daily, run_session, run_statusline};
pub use source::SOURCE;

use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    path::Path,
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

use crate::{
    LoadedEntry, Result, Speed, TimestampMs, TokenUsageRaw, UsageEntry, UsageSummary,
    cli::{CostMode, SharedArgs},
    daily::DedupEntry as _,
    debug_log,
    fast::{FxHashMap, LinePrefilter},
    progress,
};

pub(crate) use paths::{
    claude_paths, extract_session_parts, split_files_before_since, usage_files, visit_usage_files,
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
    load_deduped_entries(shared, project_filter).map(|deduped| deduped.entries)
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
    entries: Vec<LoadedEntry>,
    detected: bool,
    copies: Vec<(usize, Arc<str>)>,
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
) -> Result<Vec<LoadedEntry>> {
    let diagnostics = Diagnostics::new("claude");
    let entries = load_entries_since_with_diagnostics(shared, project_filter, &diagnostics)?;
    diagnostics.emit(shared.debug);
    Ok(entries)
}

pub(crate) fn load_entries_since_with_diagnostics(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<Vec<LoadedEntry>> {
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
    Ok(daily.summaries(group_by_project))
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
    group_by_project: bool,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<DailySummaries> {
    let load = || daily::load_daily_entries(shared, project_filter, diagnostics);
    let daily = owner.run(progress::UsageLoadAgent("Claude"), shared.json, load)?;
    Ok(DailySummaries {
        summaries: daily.summaries(group_by_project),
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
        |entry| &entry.date,
    );
    Ok(loaded)
}

fn retain_copied_entries<T>(
    entries: &mut Vec<T>,
    copies: &mut Vec<(usize, Arc<str>)>,
    window: &crate::ReportWindow,
    date: impl Fn(&T) -> &str,
) {
    if window.is_unbounded() {
        return;
    }
    if copies.is_empty() {
        entries.retain(|entry| window.contains_date(date(entry)));
        return;
    }
    let mut kept_indexes = Vec::with_capacity(entries.len());
    let mut next_index = 0;
    entries.retain(|entry| {
        let keep = window.contains_date(date(entry));
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
    let read = |file: &Path| {
        read_usage_file_with_diagnostics(
            file,
            context.window().timezone(),
            context.table_for_mode(mode),
            diagnostics,
        )
    };
    let mut pruned = Vec::new();
    let loaded_files = if skip_files_before_since && shared.since.is_some() {
        let files = usage_files(&paths, project_filter, diagnostics);
        debug_log(shared, format!("Found {} JSONL usage files", files.len()));
        let split = split_files_before_since(files, shared, utc_now());
        pruned = split.pruned;
        let files = split.kept;
        debug_log(
            shared,
            format!(
                "Kept {} JSONL usage files inside the --since window",
                files.len()
            ),
        );
        read_discovered_files(
            shared.single_thread,
            |visit| files.into_iter().for_each(visit),
            read,
        )
    } else {
        let loaded_files = read_discovered_files(
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

fn loaded_entry_session_id(entry: &LoadedEntry) -> &str {
    entry
        .data
        .session_id
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

impl daily::DedupEntry for LoadedEntry {
    fn timestamp(&self) -> TimestampMs {
        self.timestamp
    }

    fn session_id(&self) -> &str {
        loaded_entry_session_id(self)
    }

    fn owned_session_id(&self) -> Arc<str> {
        match self.data.session_id.as_deref() {
            Some(session_id) => Arc::from(session_id),
            None => Arc::clone(&self.session_id),
        }
    }

    fn message_id(&self) -> Option<&str> {
        self.data.message.id.as_deref()
    }

    fn request_id(&self) -> Option<&str> {
        self.data.request_id.as_deref()
    }

    fn is_sidechain(&self) -> bool {
        self.data.is_sidechain == Some(true)
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.data.message.usage
    }

    fn records_cost(&self) -> bool {
        self.data.cost_usd.is_some()
    }
}

fn read_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    pricing: ccusage_core::cost::PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<LoadedEntry> {
    let (session_id, project_path) = extract_session_parts(path);
    let session_id: Arc<str> = Arc::from(session_id);
    let project_path: Arc<str> = Arc::from(project_path);
    daily::read_usage_file(path, tz, pricing, diagnostics, |parsed, line| LoadedEntry {
        model: daily::ParsedUsage::display_model(parsed.raw_model.clone(), &parsed.usage),
        usage_limit_reset_time: usage_limit_reset_time_from_line_bytes(
            line,
            parsed.is_api_error_message,
        ),
        data: UsageEntry {
            timestamp: parsed.timestamp_text,
            version: parsed.version,
            session_id: parsed.recorded_session_id,
            message: UsageMessage {
                id: parsed.key.message_id,
                model: parsed.raw_model,
                usage: parsed.usage,
            },
            request_id: parsed.key.request_id,
            is_sidechain: parsed.key.is_sidechain,
            cost_usd: parsed.cost_usd,
            is_api_error_message: parsed.is_api_error_message,
        },
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
            match self.map.next_value::<serde_json::Value>()? {
                serde_json::Value::Null => self.has_null = true,
                value => {
                    self.pending = Some(value);
                    return key.into_seed(seed).map(Some);
                }
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
mod tests {
    use std::assert_matches;
    use std::{path::Path, sync::Arc};

    use super::{
        extract_session_parts, load_deduped_entries, paths::is_project_path_segment, usage_files,
        usage_limit_reset_time_from_line_bytes,
    };
    use crate::{
        Diagnostics, LoadedEntry, ModelBreakdown, PricingMap, Speed, TimestampMs, TokenUsageRaw,
        UsageEntry, UsageMessage,
        cli::{CostMode, SharedArgs},
        format_rfc3339_millis,
    };
    use ccusage_test_support::{EnvGuard, fs_fixture};
    use jiff::tz::TimeZone as JiffTimeZone;

    struct LoadedFile {
        entries: Vec<LoadedEntry>,
    }

    fn read_usage_file_with_diagnostics(
        path: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: Option<&PricingMap>,
        diagnostics: &Diagnostics,
    ) -> LoadedFile {
        LoadedFile {
            entries: super::read_usage_file_with_diagnostics(
                path,
                tz,
                pricing.map_or(ccusage_core::cost::PriceTable::Display, |map| {
                    ccusage_core::cost::PriceTable::for_map(mode, map)
                }),
                diagnostics,
            ),
        }
    }

    use super::daily::push_deduped_entry;

    fn read_usage_file(
        path: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: Option<&PricingMap>,
    ) -> LoadedFile {
        read_usage_file_with_diagnostics(path, tz, mode, pricing, &Diagnostics::new("claude"))
    }

    #[test]
    fn windowed_calculate_mode_prices_entries_with_shared_display_mode() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/pricing.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"pricing","requestId":"known","message":{"id":"known","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
                r#"{"timestamp":"2026-01-02T10:01:00Z","sessionId":"pricing","requestId":"unknown","message":{"id":"unknown","model":"unlisted-fixture-model","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
            ].join("\n"),
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            offline: true,
            single_thread: true,
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };
        let loaded = super::load_windowed_entries(
            &shared,
            None,
            &Diagnostics::new("claude"),
            CostMode::Calculate,
        )
        .unwrap();
        assert_eq!(loaded.entries.len(), 2);
        let known = loaded
            .entries
            .iter()
            .find(|entry| entry.model.as_deref() == Some("claude-sonnet-4-20250514"))
            .unwrap();
        assert!((known.cost - 0.018).abs() < 1e-12, "{}", known.cost);
        assert_eq!(known.missing_pricing_model, None);
        let unknown = loaded
            .entries
            .iter()
            .find(|entry| entry.model.as_deref() == Some("unlisted-fixture-model"))
            .unwrap();
        assert_eq!(unknown.cost, 0.0);
        assert_eq!(
            unknown.missing_pricing_model.as_deref(),
            Some("unlisted-fixture-model")
        );
    }

    #[test]
    fn all_report_loaders_drop_timestamp_different_sidechain_replays() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"session-a","requestId":"parent","message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"timestamp":"2026-01-02T10:00:01Z","sessionId":"session-a","requestId":"replay","isSidechain":true,"message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            ].join("\n"),
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 10);
    }

    #[test]
    fn all_report_loaders_count_agent_progress_usage() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": r#"{"sessionId":"session-a","data":{"message":{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-progress","message":{"id":"msg-progress","model":"claude-sonnet-4-20250514","usage":{"input_tokens":7,"output_tokens":2}}}}}"#,
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 7);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 7);
    }

    #[test]
    fn all_report_loaders_prefer_usage_detail_over_stored_cost() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","costUSD":20,"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","costUSD":1,"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1,"speed":"fast"}}}"#,
            ].join("\n"),
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 1.0);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].total_cost, 1.0);
    }

    #[test]
    fn counts_usage_with_nulls_in_unrelated_nested_members() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00Z","requestId":"req-a","cwd":null,"toolUseResult":{"iterations":[{"model":null,"id":null,"cwd":null,"speed":null}]},"message":{"id":"msg-a","model":"claude-sonnet-4-20250514","content":[{"id":null,"model":null}],"usage":{"input_tokens":10,"output_tokens":1,"metadata":{"cache_read_input_tokens":null}}}}"#,
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let entries = super::load_entries(&shared, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        let daily = super::load_daily_summaries(&shared, None, false).unwrap();
        assert_eq!(daily[0].input_tokens, 10);
    }

    #[test]
    fn extracts_usage_limit_reset_time_from_raw_line() {
        let line = br#"{"timestamp":"2025-01-10T10:00:00.000Z","isApiErrorMessage":true,"message":{"content":[{"text":"Claude AI usage limit reached|1736503200 remaining"}],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let reset_time = usage_limit_reset_time_from_line_bytes(line, Some(true)).unwrap();

        assert_eq!(
            format_rfc3339_millis(reset_time),
            "2025-01-10T10:00:00.000Z"
        );
        assert!(usage_limit_reset_time_from_line_bytes(line, Some(false)).is_none());
        assert!(
            usage_limit_reset_time_from_line_bytes(
                br#"{"message":{"content":[{"text":"Claude AI usage limit reached|0"}]}}"#,
                Some(true)
            )
            .is_none()
        );
    }

    pub(crate) const SYNTHETIC_NULL_SPEED_LINE: &str = r#"{"timestamp":"2026-01-02T10:00:01.000Z","sessionId":"session-a","isApiErrorMessage":true,"message":{"id":"msg-synthetic","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"service_tier":null,"speed":null}}}"#;

    #[test]
    fn null_contract_rejections_claude_code_writes_stay_quiet() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                SYNTHETIC_NULL_SPEED_LINE,
            ]
            .join("\n"),
        });
        let diagnostics = ccusage_core::Diagnostics::new("claude");

        let loaded = read_usage_file_with_diagnostics(
            &fixture.path("projects/project-a/session-a.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
            &diagnostics,
        );

        assert_eq!(loaded.entries.len(), 1);
        assert!(
            diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                })
                .is_empty()
        );
    }

    #[test]
    fn limits_usage_file_discovery_to_requested_project() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/a.jsonl": "{}",
            "projects/project-b/session-b/b.jsonl": "{}",
        });

        let files = usage_files(
            &[fixture.root().to_path_buf()],
            Some("project-a"),
            &ccusage_core::Diagnostics::new("claude"),
        );

        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("project-a"));
    }

    #[test]
    fn filters_non_segment_projects_before_reading_files() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/a.jsonl": "{}",
            "projects/project-b/session-b/b.jsonl": "{}",
        });

        let files = usage_files(
            &[fixture.root().to_path_buf()],
            Some("project-a/session-a"),
            &ccusage_core::Diagnostics::new("claude"),
        );

        assert!(files.is_empty());
    }

    #[test]
    fn rejects_dot_segments_as_project_path_segments() {
        assert!(!is_project_path_segment(""));
        assert!(!is_project_path_segment("."));
        assert!(!is_project_path_segment(".."));
        assert!(!is_project_path_segment("project-a/session-a"));
        assert!(!is_project_path_segment("project-a\\session-a"));
        assert!(is_project_path_segment("project-a"));
    }

    #[test]
    fn extracts_file_session_from_modern_claude_project_path() {
        let (session_id, project_path) = extract_session_parts(Path::new(
            "/home/me/.claude/projects/project-a/session-a.jsonl",
        ));

        assert_eq!(session_id, "session-a");
        assert_eq!(project_path, "project-a");
    }

    #[test]
    fn extracts_parent_session_from_nested_claude_project_path() {
        let (session_id, project_path) = extract_session_parts(Path::new(
            "/home/me/.claude/projects/project-a/session-a/chat.jsonl",
        ));

        assert_eq!(session_id, "session-a");
        assert_eq!(project_path, "project-a");
    }

    #[test]
    fn extracts_parent_session_from_claude_subagent_path() {
        let (session_id, project_path) = extract_session_parts(Path::new(
            "/home/me/.claude/projects/project-a/session-a/subagents/worker.jsonl",
        ));

        assert_eq!(session_id, "session-a");
        assert_eq!(project_path, "project-a");
    }

    #[test]
    fn extracts_parent_session_from_claude_workflow_run_path() {
        let (session, project) = extract_session_parts(Path::new(
            "/home/user/.claude/projects/project-a/session-a/subagents/workflows/wf_1/agent-a.jsonl",
        ));
        assert_eq!(
            (session.as_str(), project.as_str()),
            ("session-a", "project-a")
        );
    }

    #[test]
    fn rejects_null_in_each_read_non_nullable_field() {
        for pointer in [
            "/sessionId",
            "/requestId",
            "/version",
            "/costUSD",
            "/isApiErrorMessage",
            "/message/id",
            "/message/model",
            "/message/usage/speed",
            "/message/usage/cache_read_input_tokens",
            "/message/usage/cache_creation_input_tokens",
        ] {
            let mut record = serde_json::json!({
                "timestamp": "2026-01-02T10:00:00Z", "sessionId": "session-a", "requestId": "request-a",
                "version": "1.2.3", "costUSD": 1, "isApiErrorMessage": false,
                "message": {"id": "msg-a", "model": "model-a", "usage": {"input_tokens": 10, "output_tokens": 1, "speed": "standard", "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}
            });
            *record.pointer_mut(pointer).unwrap() = serde_json::Value::Null;
            let bytes = serde_json::to_vec(&record).unwrap();
            assert_eq!(
                super::daily::parse_usage_line(&bytes).unwrap_err(),
                super::Rejection::NullContract,
                "{pointer}"
            );
            let wrapped =
                serde_json::json!({"sessionId":record["sessionId"], "data":{"message":record}});
            if pointer != "/version" {
                assert_eq!(
                    super::daily::parse_usage_line(&serde_json::to_vec(&wrapped).unwrap())
                        .unwrap_err(),
                    super::Rejection::NullContract,
                    "wrapped {pointer}"
                );
            }
        }
    }

    #[test]
    fn allows_missing_optional_fields_and_nullable_unread_members() {
        for extra in [
            r#","cwd":null,"content":[{"id":null,"model":null}],"toolUseResult":{"iterations":[{"model":null}]}"#,
            r#","isSidechain":null"#,
            "",
        ] {
            let line = format!(
                r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":1,"output_tokens":2,"iterations":[{{"type":"message","model":null}}]}}}}{extra}}}"#
            );
            assert!(
                super::daily::parse_usage_line(line.as_bytes()).is_ok(),
                "{line}"
            );
        }
    }

    #[test]
    fn rejects_null_under_escaped_read_field_names() {
        for line in [
            br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"m\u006fdel":null,"usage":{"input_tokens":1,"output_tokens":2}}}"#.as_slice(),
            br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"sp\u0065ed":null}}}"#,
        ] {
            assert_eq!(
                super::daily::parse_usage_line(line).unwrap_err(),
                super::Rejection::NullContract
            );
        }
        let (record, _) = super::daily::parse_usage_line(
            br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"cache_r\u0065ad_input_tokens":7,"sp\u0065ed":"fast"}}}"#,
        )
        .unwrap();
        assert_eq!(record.message.usage.usage.cache_read_input_tokens, 7);
        assert!(matches!(
            record.message.usage.usage.speed,
            Some(crate::Speed::Fast)
        ));
    }

    #[test]
    fn malformed_required_usage_is_diagnosed_separately_from_null_contract() {
        for usage in [
            r#"{"input_tokens":null,"output_tokens":2}"#,
            r#"{"input_tokens":1,"output_tokens":null}"#,
            r#"{"input_tokens":1}"#,
        ] {
            let line =
                format!(r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{usage}}}}}"#);
            assert_eq!(
                super::daily::parse_usage_line(line.as_bytes()).unwrap_err(),
                super::Rejection::Malformed
            );
        }
    }

    #[test]
    fn handles_duplicate_members_the_same_with_or_without_nulls() {
        for nulls in [
            "",
            r#","stop_sequence":null"#,
            r#","iterations":[{"type":"message","model":null}]"#,
        ] {
            let (message_nulls, usage_nulls) = if nulls.contains("iterations") {
                ("", nulls)
            } else {
                (nulls, "")
            };
            let duplicate_known = format!(
                r#"{{"timestamp":"2026-09-12T04:38:42.296Z","message":{{"id":"msg_1","model":"first","model":"second"{message_nulls},"usage":{{"input_tokens":2,"output_tokens":1{usage_nulls}}}}}}}"#
            );
            let duplicate_unknown = format!(
                r#"{{"timestamp":"2026-09-12T04:38:42.296Z","extra":1,"extra":2,"message":{{"id":"msg_1","model":"m"{message_nulls},"usage":{{"input_tokens":2,"output_tokens":1{usage_nulls}}}}}}}"#
            );

            assert!(
                super::daily::parse_usage_line(duplicate_known.as_bytes()).is_err(),
                "{duplicate_known}"
            );
            assert!(
                super::daily::parse_usage_line(duplicate_unknown.as_bytes()).is_ok(),
                "{duplicate_unknown}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_known_usage_fields_but_keeps_unknown_duplicates() {
        for (field, value) in [
            ("input_tokens", "1"),
            ("output_tokens", "2"),
            ("cache_read_input_tokens", "0"),
            ("cache_creation_input_tokens", "0"),
            ("speed", r#""fast""#),
            ("cache_creation", "{}"),
            ("iterations", "[]"),
        ] {
            let line = format!(
                r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":1,"output_tokens":2,"{field}":{value},"{field}":{value}}}}}}}"#
            );
            assert!(
                super::daily::parse_usage_line(line.as_bytes()).is_err(),
                "{field}"
            );
        }
        let line = br#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":1,"output_tokens":2,"metadata":null,"metadata":{}}}}"#;
        assert!(super::daily::parse_usage_line(line).is_ok());
    }

    #[test]
    fn rejects_duplicate_members_before_typed_deserialization() {
        assert!(super::daily::parse_usage_line(
            br#"{"type":"assistant","timestamp":"2026-09-12T04:38:42.296Z","sessionId":"session-a","message":{"id":"msg_1","model":"first","model":"second","usage":{"input_tokens":2,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289}]}}}"#
        )
        .is_err());
    }

    #[test]
    fn counts_entries_whose_iteration_model_is_null() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"type":"assistant","timestamp":"2026-09-12T04:38:42.296Z","version":"2.1.268","sessionId":"session-a","requestId":"req_1","message":{"id":"msg_1","model":"claude-fable-5-1","role":"assistant","usage":{"input_tokens":2,"cache_creation_input_tokens":55866,"cache_read_input_tokens":0,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289,"cache_creation_input_tokens":55866,"cache_read_input_tokens":0}]}}}"#,
        });

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Calculate,
            Some(&PricingMap::default()),
        );

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(loaded.entries[0].data.message.usage.output_tokens, 289);
    }

    #[test]
    fn calculates_advisor_cost_with_the_advisor_model() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg-parent","model":"main-model","usage":{"input_tokens":1,"output_tokens":2,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}]}},"requestId":"req-parent","costUSD":1.23}"#,
        });
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "main-model": {
                    "input_cost_per_token": 100,
                    "output_cost_per_token": 100
                },
                "advisor-model": {
                    "input_cost_per_token": 2,
                    "output_cost_per_token": 3
                }
            }"#,
        );

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            Some(&pricing),
        );

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].cost, 1.23);
        assert_eq!(loaded.entries[1].model.as_deref(), Some("advisor-model"));
        assert_eq!(loaded.entries[1].cost, 26.0);
        assert_eq!(
            loaded.entries[1].data.message.model.as_deref(),
            Some("advisor-model")
        );
        assert_eq!(loaded.entries[1].data.cost_usd, None);
    }

    #[test]
    fn reads_usage_and_advisor_iterations_under_escaped_keys() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","version":"1.2.3","sessionId":"session-a","message":{"id":"msg-parent","model":"main-model","us\u0061ge":{"input_tokens":1,"output_tokens":2,"iterations":[{"type":"advisor\u005fmessage","model":"advisor-model","input_tokens":10,"output_tokens":2}]}},"requestId":"req-parent"}"#,
        });

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].data.message.usage.output_tokens, 2);
        assert_eq!(loaded.entries[1].model.as_deref(), Some("advisor-model"));
    }

    #[test]
    fn keeps_gateway_usage_from_distinct_sessions_with_reused_message_id() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":300,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        for path in [
            fixture.path("projects/project-a/session-a/chat.jsonl"),
            fixture.path("projects/project-a/session-b/chat.jsonl"),
        ] {
            let loaded = read_usage_file(
                &path,
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                None,
            );
            for entry in loaded.entries {
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
            }
        }

        assert_eq!(deduped.len(), 2);
        assert_eq!(
            deduped
                .iter()
                .map(|entry| entry.data.message.usage.input_tokens)
                .sum::<u64>(),
            400
        );
    }

    #[test]
    fn keeps_requestless_usage_from_same_session_at_distinct_timestamps() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:41.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":250,"speed":"standard"}}}"#,
            ]
            .join("\n"),
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }

        assert_eq!(deduped.len(), 2);
        assert_eq!(
            deduped
                .iter()
                .map(|entry| entry.data.message.usage.output_tokens)
                .sum::<u64>(),
            275
        );
    }

    #[test]
    fn keeps_the_cache_ttl_split_from_an_otherwise_equal_duplicate() {
        for lines in [
            [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
            ],
            [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100}}}"#,
            ],
        ] {
            let fixture = fs_fixture!({
                "projects/project-a/session-a.jsonl": lines.join("\n"),
            });
            let _env = EnvGuard::set_many([(
                "CLAUDE_CONFIG_DIR",
                Some(fixture.root().as_os_str().to_os_string()),
            )]);
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread: true,
                ..SharedArgs::default()
            };

            let deduped = load_deduped_entries(&shared, None).unwrap();

            assert_eq!(deduped.entries.len(), 1);
            let mut breakdown = ModelBreakdown::default();
            breakdown.add_usage(&deduped.entries[0].data.message.usage);
            assert_eq!(breakdown.cache_creation_tokens, 100);
            assert_eq!(breakdown.cache_creation_1h_tokens, Some(60));
        }
    }

    #[test]
    fn keeps_the_fast_tier_from_an_otherwise_equal_duplicate() {
        for lines in [
            [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"standard","cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"fast"}}}"#,
            ],
            [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"fast"}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg_a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"speed":"standard","cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#,
            ],
        ] {
            let fixture = fs_fixture!({
                "projects/project-a/session-a.jsonl": lines.join("\n"),
            });
            let mut deduped_indexes = Default::default();
            let mut deduped = Vec::new();

            let loaded = read_usage_file(
                &fixture.path("projects/project-a/session-a.jsonl"),
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                None,
            );
            for entry in loaded.entries {
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
            }

            assert_eq!(deduped.len(), 1);
            assert_matches!(deduped[0].data.message.usage.speed, Some(Speed::Fast));
        }
    }

    #[test]
    fn dedupes_anthropic_message_id_from_same_session_at_distinct_timestamps() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.500Z","message":{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
            ]
            .join("\n"),
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.message.usage.output_tokens, 25);
    }

    #[test]
    fn dedupes_response_message_id_from_same_session_at_distinct_timestamps() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"resp_0123456789abcdef","model":"gpt-5.5","usage":{"input_tokens":100,"output_tokens":25}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.500Z","message":{"id":"resp_0123456789abcdef","model":"gpt-5.5","usage":{"input_tokens":100,"output_tokens":25}}}"#,
            ]
            .join("\n"),
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.message.usage.output_tokens, 25);
    }

    #[test]
    fn dedupes_repeated_requestless_writes_at_the_same_timestamp() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":25}}}"#,
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":250,"speed":"standard"}}}"#,
            ]
            .join("\n"),
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let loaded = read_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );
        for entry in loaded.entries {
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
        }

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.message.usage.output_tokens, 250);
    }

    #[test]
    fn dedupes_copied_transcripts_with_the_same_serialized_session_id() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        for path in [
            fixture.path("projects/project-a/session-a/chat.jsonl"),
            fixture.path("projects/project-a/session-b/chat.jsonl"),
        ] {
            let loaded = read_usage_file(
                &path,
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                None,
            );
            for entry in loaded.entries {
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
            }
        }

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.message.usage.input_tokens, 200);
    }

    #[test]
    fn dedupes_copied_transcripts_with_the_same_request_id_across_sessions() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        for path in [
            fixture.path("projects/project-a/session-a/chat.jsonl"),
            fixture.path("projects/project-a/session-b/chat.jsonl"),
        ] {
            let loaded = read_usage_file(
                &path,
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                None,
            );
            for entry in loaded.entries {
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
            }
        }

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.message.usage.input_tokens, 200);
    }

    #[test]
    fn returns_the_discarded_copy_from_another_session() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
            "projects/project-a/session-c/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-c","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":50,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let discarded = ["a", "b", "c"].map(|session| {
            let path = fixture.path(format!("projects/project-a/session-{session}/chat.jsonl"));
            let entry = read_usage_file(
                &path,
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                None,
            )
            .entries
            .into_iter()
            .next()
            .unwrap();
            push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
                .map(|(index, discarded)| (index, discarded.session_id.to_string()))
        });

        assert_eq!(
            discarded,
            [
                None,
                Some((0, "session-a".to_string())),
                Some((0, "session-c".to_string())),
            ]
        );
        assert_eq!(deduped[0].session_id.as_ref(), "session-b");
    }

    #[test]
    fn attributes_a_cross_session_copy_to_the_survivor_day() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
            "projects/project-a/session-b.jsonl": [
                r#"{"timestamp":"2026-01-01T10:00:00.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
                r#"{"timestamp":"2026-01-03T10:00:00.000Z","sessionId":"session-b","requestId":"req-own","message":{"id":"msg-own","model":"claude-sonnet-4-20250514","usage":{"input_tokens":7,"output_tokens":1}}}"#,
            ]
            .join("\n"),
        });
        let _env = EnvGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            single_thread: true,
            json: true,
            ..SharedArgs::default()
        };
        let deduped = load_deduped_entries(&shared, None).unwrap();
        let sorted_days = |deduped: &crate::DedupedEntries| {
            let mut days: Vec<_> = deduped
                .main_thread_sessions()
                .into_iter()
                .map(|session| session.to_string())
                .collect();
            days.sort();
            days
        };

        assert_eq!(deduped.entries.len(), 2);
        assert_eq!(sorted_days(&deduped), ["session-a", "session-b"]);
        let filtered = crate::DedupedEntries {
            entries: deduped
                .entries
                .into_iter()
                .filter(|entry| entry.date != "2026-01-02")
                .collect(),
            ..Default::default()
        };
        assert_eq!(sorted_days(&filtered), ["session-b"]);
    }

    #[test]
    fn dedupes_sidechain_replay_after_an_equal_copy_from_another_session() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let mut copied_parent = loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 20,
            output_tokens: 10,
        });
        copied_parent.data.session_id = Some("session-b".to_string());
        copied_parent.session_id = Arc::from("session-b");
        push_deduped_entry(copied_parent, &mut deduped_indexes, &mut deduped);

        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 20);
    }

    #[test]
    fn dedupes_sidechain_replay_after_cross_session_survivor_replacement() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let mut copied_parent = loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 20,
            output_tokens: 10,
        });
        copied_parent.data.session_id = Some("session-b".to_string());
        copied_parent.session_id = Arc::from("session-b");
        push_deduped_entry(copied_parent, &mut deduped_indexes, &mut deduped);

        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 30,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );

        let mut copied_session_replay = loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-sidechain-replay",
            is_sidechain: true,
            cache_read_tokens: 50_000,
            output_tokens: 10,
        });
        copied_session_replay.data.session_id = Some("session-b".to_string());
        copied_session_replay.session_id = Arc::from("session-b");
        push_deduped_entry(copied_session_replay, &mut deduped_indexes, &mut deduped);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].session_id.as_ref(), "session-a");
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 30);
    }

    #[test]
    fn keeps_parent_usage_when_sidechain_replays_message_with_new_request_id() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-sidechain-answer",
                request_id: "req-sidechain-answer",
                is_sidechain: true,
                cache_read_tokens: 700,
                output_tokens: 30,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].data.message.id.as_deref(), Some("msg-parent"));
        assert_eq!(deduped[0].data.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 20);
        assert_eq!(
            deduped[1].data.message.id.as_deref(),
            Some("msg-sidechain-answer")
        );
        assert_eq!(deduped[1].data.message.usage.cache_read_input_tokens, 700);
    }

    #[test]
    fn refreshes_dedupe_indexes_when_parent_replaces_sidechain_replay() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain-replay",
                is_sidechain: true,
                cache_read_tokens: 50_000,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 10,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 5,
                output_tokens: 5,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].data.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 20);
    }

    struct UsageEntryFixture {
        message_id: &'static str,
        request_id: &'static str,
        is_sidechain: bool,
        cache_read_tokens: u64,
        output_tokens: u64,
    }

    fn loaded_usage_entry(fixture: UsageEntryFixture) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-03-29T07:00:00.000Z".to_string(),
                version: Some("1.0.0".to_string()),
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 0,
                        output_tokens: fixture.output_tokens,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: fixture.cache_read_tokens,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some(fixture.message_id.to_string()),
                },
                cost_usd: None,
                request_id: Some(fixture.request_id.to_string()),
                is_api_error_message: None,
                is_sidechain: Some(fixture.is_sidechain),
            },
            timestamp: TimestampMs::from_millis(1_775_000_000_000),
            date: "2026-03-29".to_string(),
            project: Arc::from("project-a"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("project-a"),
            cost: 0.0,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            stored_cost: None,
        }
    }
}
