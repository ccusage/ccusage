use ccusage_adapter_common::{read_discovered_files, source::ProgressOwner};
use ccusage_core::*;

mod blocks;
mod commands;
mod daily;
mod diagnostics;
#[cfg(test)]
mod load_tests;
mod paths;
mod source;

pub use commands::{run_blocks, run_bucket, run_daily, run_session, run_statusline, run_weekly};
pub use source::SOURCE;

use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, LazyLock, OnceLock},
};

use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem;
use rustc_hash::FxHasher;
use serde::{Deserialize, de::DeserializeOwned};
use smallvec::SmallVec;

use crate::{
    LoadedEntry, PricingMap, Result, Speed, TimestampMs, TokenUsageRaw, UsageEntry, UsageSummary,
    cli::{CostMode, SharedArgs},
    debug_log,
    fast::{FxHashMap, LinePrefilter},
    log_level, progress, resolve_tz,
};

pub(crate) use paths::{
    claude_paths, extract_session_parts, split_files_before_since, usage_file_order, usage_files,
    visit_usage_files,
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

/// Loads every deduplicated Claude usage entry like [`load_entries`], keeping
/// the sessions of the copies dedup merged into them.
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
    diagnostics.emit(shared);
    Ok(loaded)
}

/// Loads deduplicated entries with progress owned by the caller or source.
pub(crate) fn load_deduped_entries_with_progress(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<DedupedEntries> {
    let load = || load_entries_inner(shared, project_filter, false, diagnostics);
    match owner {
        ProgressOwner::Source => {
            progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, load)
        }
        ProgressOwner::Caller => load(),
    }
}

/// Deduplicated Claude entries plus the main-thread copies dedup merged into
/// them from other sessions, such as a resumed or forked transcript repeating
/// a turn.
#[derive(Default)]
pub(crate) struct DedupedEntries {
    pub(crate) entries: Vec<LoadedEntry>,
    /// `(survivor index, recorded session of the copy)` pairs.
    copies: Vec<(usize, Arc<str>)>,
}

impl DedupedEntries {
    /// Main-thread days of the entries `is_counted` keeps, by recorded session ID. As in
    /// the daily loader, a merged copy's session counts on the day its survivor does.
    pub(crate) fn main_thread_session_days(
        &self,
        is_counted: impl Fn(&LoadedEntry) -> bool,
    ) -> SessionDays {
        main_thread_session_days(
            self.entries
                .iter()
                .map(|entry| (loaded_entry_session_id(entry), entry))
                .chain(
                    self.copies
                        .iter()
                        .map(|(index, session)| (session.as_ref(), &self.entries[*index])),
                )
                .filter(|(_, entry)| is_counted(entry))
                .map(|(session, entry)| (session, entry.date.as_str(), entry.data.is_sidechain)),
        )
    }
}

/// Loads Claude usage entries for a report that keeps only entries dated
/// inside the `--since` window.
///
/// Sessions last written before the window are skipped without being read, so
/// entries dated before `since` may be missing. Callers must discard those
/// entries, which leaves the in-window result identical to [`load_entries`].
///
/// @param shared Report arguments; `since` and `timezone` bound the files read.
/// @param project_filter Optional project directory to restrict the scan to.
/// @returns Deduplicated entries, complete for every date on or after `since`.
pub(crate) fn load_entries_since(
    shared: &SharedArgs,
    project_filter: Option<&str>,
) -> Result<Vec<LoadedEntry>> {
    let diagnostics = Diagnostics::new("claude");
    let entries = load_entries_since_with_diagnostics(shared, project_filter, &diagnostics)?;
    diagnostics.emit(shared);
    Ok(entries)
}

/// Loads bounded entries into caller-owned diagnostics without emitting warnings.
pub(crate) fn load_entries_since_with_diagnostics(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<Vec<LoadedEntry>> {
    progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, || {
        load_entries_inner(shared, project_filter, true, diagnostics)
    })
    .map(|deduped| deduped.entries)
}

/// Loads daily Claude summaries. Summaries dated before `since` may be
/// missing, so callers must filter them by date.
pub(crate) fn load_daily_summaries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    group_by_project: bool,
) -> Result<Vec<UsageSummary>> {
    let diagnostics = Diagnostics::new("claude");
    let daily =
        progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, || {
            daily::load_daily_summaries_inner(
                shared,
                project_filter,
                group_by_project,
                false,
                &diagnostics,
            )
        })?;
    diagnostics.emit(shared);
    Ok(daily.summaries)
}

/// Main-thread Claude usage as `(session ID, date)` pairs, so a source that
/// runs Claude Code, such as OpenCode's `claude-agent` provider, can skip its
/// copy of the turns this source counts.
pub type SessionDays = HashSet<(Arc<str>, String)>;

/// Takes `(session ID, date, is_sidechain)` per entry and leaves out sidechain
/// (subagent) usage, which a wrapping tool does not record. Dedups the borrowed
/// pairs before owning them, so the date is cloned once per session day.
fn main_thread_session_days<'a>(
    entries: impl Iterator<Item = (&'a str, &'a str, Option<bool>)>,
) -> SessionDays {
    entries
        .filter(|(_, _, is_sidechain)| *is_sidechain != Some(true))
        .map(|(session_id, date, _)| (session_id, date))
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|(session_id, date)| (Arc::from(session_id), date.to_owned()))
        .collect()
}

/// Daily Claude summaries plus whether any Claude usage exists at all.
pub struct DailySummaries {
    /// Summaries, possibly missing dates before `since`.
    pub summaries: Vec<UsageSummary>,
    /// Whether any usage file holds an entry, including files skipped as
    /// outside the `--since` window.
    pub detected: bool,
    /// Session days of the summarized main-thread entries, possibly missing
    /// dates before `since`.
    pub session_days: SessionDays,
}

/// Loads daily summaries with progress owned by the caller or source.
pub(crate) fn load_daily_summaries_with_detection_progress(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    group_by_project: bool,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<DailySummaries> {
    let load = || {
        daily::load_daily_summaries_inner(
            shared,
            project_filter,
            group_by_project,
            true,
            diagnostics,
        )
    };
    match owner {
        ProgressOwner::Source => {
            progress::track_usage_load(progress::UsageLoadAgent("Claude"), shared.json, load)
        }
        ProgressOwner::Caller => load(),
    }
}

fn load_entries_inner(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    skip_files_before_since: bool,
    diagnostics: &Diagnostics,
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
    let mode = shared.mode;
    let read = |file: &Path| {
        read_usage_file_with_diagnostics(
            file,
            context.timezone(),
            mode,
            context.pricing(),
            diagnostics,
        )
    };
    let mut loaded_files = if skip_files_before_since && shared.since.is_some() {
        let files = usage_files(&paths, project_filter, diagnostics);
        debug_log(shared, format!("Found {} JSONL usage files", files.len()));
        let files = split_files_before_since(files, shared, utc_now()).kept;
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
        return Ok(DedupedEntries::default());
    }
    // Dedup keeps the first copy it meets, so files are folded in listing order.
    loaded_files.sort_by_cached_key(|(path, _)| usage_file_order(path));
    let loaded_files = loaded_files
        .into_iter()
        .map(|(_, file)| file)
        .collect::<Vec<_>>();
    let loaded_entry_count = loaded_files
        .iter()
        .map(|file| file.entries.len())
        .sum::<usize>();
    debug_log(
        shared,
        format!(
            "Loaded {loaded_entry_count} usage entries from {} JSONL files",
            loaded_files.len()
        ),
    );

    let mut deduped_indexes: FxHashMap<u64, DedupeIndexVec> =
        FxHashMap::with_capacity_and_hasher(loaded_entry_count, Default::default());
    let mut deduped = Vec::with_capacity(loaded_entry_count);
    let mut copies = Vec::new();
    for loaded_file in loaded_files {
        for entry in loaded_file.entries {
            if let Some((index, discarded)) =
                daily::push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
                && discarded.key.session_id != deduped[index].key.session_id
                && discarded.key.is_sidechain != Some(true)
            {
                copies.push((index, discarded.key.session_id));
            }
        }
    }
    debug_log(
        shared,
        format!("Kept {} usage entries after deduplication", deduped.len()),
    );
    Ok(DedupedEntries {
        entries: deduped
            .into_iter()
            .map(daily::ClaudeEntry::into_loaded)
            .collect(),
        copies,
    })
}

/// The timezone and pricing a load prices entries with, resolved by the
/// first file that needs them, so a load that finds no files skips both.
pub(crate) struct UsageLoadContext<'a> {
    pub(crate) shared: &'a SharedArgs,
    timezone: OnceLock<JiffTimeZone>,
    pricing: OnceLock<Option<PricingMap>>,
}

impl<'a> UsageLoadContext<'a> {
    pub(crate) fn new(shared: &'a SharedArgs) -> Self {
        Self {
            shared,
            timezone: OnceLock::new(),
            pricing: OnceLock::new(),
        }
    }

    pub(crate) fn timezone(&self) -> &JiffTimeZone {
        self.timezone
            .get_or_init(|| resolve_tz(self.shared.timezone.as_deref()))
    }

    pub(crate) fn pricing(&self) -> Option<&PricingMap> {
        self.pricing
            .get_or_init(|| {
                (self.shared.mode != CostMode::Display).then(|| {
                    PricingMap::load_with_overrides(
                        self.shared.offline,
                        log_level() != Some(0),
                        self.shared.pricing_overrides.iter(),
                    )
                })
            })
            .as_ref()
    }
}

/// Breaks a tie between duplicates with equal token totals by keeping the copy
/// that reports more of the request: the speed tier, with `Fast` above
/// `Standard` because only `Fast` carries the fast-mode price multiplier, then
/// the cache-write TTL split.
fn carries_more_usage_detail(candidate: &TokenUsageRaw, existing: &TokenUsageRaw) -> bool {
    let detail = |usage: &TokenUsageRaw| {
        let speed_rank = match usage.speed {
            None => 0,
            Some(Speed::Standard) => 1,
            Some(Speed::Fast) => 2,
        };
        (speed_rank, usage.cache_creation.is_some())
    };
    detail(candidate) > detail(existing)
}

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
        if !is_per_response_message_id(message_id) {
            timestamp.hash(&mut hasher);
        }
    }
    hasher.finish()
}

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

/// Replay route that only indexes sidechain entries, for parent candidates.
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

#[derive(Clone, Debug)]
struct EntryMetadata {
    timestamp: String,
    version: Option<String>,
    recorded_session_id: Option<String>,
    raw_model: Option<String>,
    cost_usd: Option<f64>,
    is_api_error_message: Option<bool>,
    usage_limit_reset_time: Option<TimestampMs>,
    session_id: Arc<str>,
    project_path: Arc<str>,
}

impl daily::ClaudeEntry<EntryMetadata> {
    fn into_loaded(self) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                timestamp: self.metadata.timestamp,
                version: self.metadata.version,
                session_id: self.metadata.recorded_session_id,
                message: UsageMessage {
                    id: self.key.message_id,
                    model: self.metadata.raw_model,
                    usage: self.usage,
                },
                request_id: self.key.request_id,
                is_sidechain: self.key.is_sidechain,
                cost_usd: self.metadata.cost_usd,
                is_api_error_message: self.metadata.is_api_error_message,
            },
            timestamp: self.key.timestamp,
            date: self.date,
            project: self.project,
            session_id: self.metadata.session_id,
            project_path: self.metadata.project_path,
            cost: self.cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: self.model,
            usage_limit_reset_time: self.metadata.usage_limit_reset_time,
            missing_pricing_model: self.missing_pricing_model,
            stored_cost: self.stored_cost,
        }
    }
}

fn read_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> daily::UsageFile<EntryMetadata> {
    let (session_id, project_path) = extract_session_parts(path);
    let session_id: Arc<str> = Arc::from(session_id);
    let project_path: Arc<str> = Arc::from(project_path);
    daily::read_usage_file_with_metadata(
        path,
        tz,
        mode,
        pricing,
        diagnostics,
        |data, line, advisor| EntryMetadata {
            timestamp: data.timestamp.clone(),
            version: data.version.clone(),
            recorded_session_id: data.session_id.clone(),
            raw_model: advisor
                .map(|advisor| advisor.model.clone())
                .or_else(|| data.message.model.clone()),
            cost_usd: if advisor.is_some() {
                None
            } else {
                data.cost_usd
            },
            is_api_error_message: data.is_api_error_message,
            usage_limit_reset_time: usage_limit_reset_time_from_line_bytes(
                line,
                data.is_api_error_message,
            ),
            session_id: Arc::clone(&session_id),
            project_path: Arc::clone(&project_path),
        },
    )
}

#[derive(Debug, Deserialize)]
struct UsageIterationsEnvelope {
    message: UsageIterationsMessage,
}

#[derive(Debug, Deserialize)]
struct UsageIterationsMessage {
    usage: UsageIterations,
}

#[derive(Debug, Deserialize)]
struct UsageIterations {
    #[serde(default)]
    iterations: Vec<UsageIteration>,
}

#[derive(Debug, Deserialize)]
struct UsageIteration {
    #[serde(rename = "type")]
    kind: String,
    model: Option<String>,
    #[serde(flatten)]
    usage: TokenUsageRaw,
}

pub(crate) struct AdvisorUsage {
    model: String,
    usage: TokenUsageRaw,
}

pub(crate) fn advisor_usages_from_line(line: &[u8]) -> Vec<AdvisorUsage> {
    static ADVISOR_MARKER: LazyLock<LinePrefilter> =
        LazyLock::new(|| LinePrefilter::all(&[br#""advisor_message""#]));
    if !ADVISOR_MARKER.matches(line) {
        return Vec::new();
    }
    let Ok(envelope) = serde_json::from_slice::<UsageIterationsEnvelope>(line) else {
        return Vec::new();
    };
    envelope
        .message
        .usage
        .iterations
        .into_iter()
        .filter_map(|iteration| {
            (iteration.kind == "advisor_message")
                .then_some(iteration.model)
                .flatten()
                .filter(|model| !model.is_empty())
                .map(|model| AdvisorUsage {
                    model,
                    usage: iteration.usage,
                })
        })
        .collect()
}

/// Mirrors the TypeScript loader's schema: a line whose known non-nullable field is `null`
/// is skipped before deserialisation. The direct `model` member of an element in either
/// assistant or AgentProgress `usage.iterations` is the one exception, because
/// `UsageIteration.model` is optional and Claude Code writes it as `null`.
#[cfg(test)]
fn has_unsupported_null_field(line: &[u8]) -> bool {
    if memmem::find(line, b"null").is_none() {
        return false;
    }
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(line) else {
        return false;
    };
    let unsupported = has_unsupported_null_field_in_value(&root);
    assert!(
        !unsupported || may_have_unsupported_null_field(line),
        "the raw pre-check must not skip a line the value check rejects"
    );
    unsupported
}

/// Why [`deserialize_usage_line`] did not return a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rejection {
    /// A member the nullable-field contract guards is `null`. Claude Code
    /// writes such lines itself, such as `<synthetic>` records with
    /// `"speed":null`, so skipping one is by design rather than a sign of bad
    /// input.
    NullContract,
    /// The line did not decode as the record type.
    Malformed,
}

/// Deserializes a transcript line while enforcing Claude's nullable-field contract.
///
/// Every other rejection, such as a duplicate known field, comes from the typed parse
/// of the raw line, so a line is loaded or skipped the same way whether or not it
/// also needed the null check.
pub(crate) fn deserialize_usage_line<T: DeserializeOwned>(
    line: &[u8],
) -> std::result::Result<T, Rejection> {
    if may_have_unsupported_null_field(line) {
        let root =
            serde_json::from_slice::<serde_json::Value>(line).map_err(|_| Rejection::Malformed)?;
        if has_unsupported_null_field_in_value(&root) {
            return Err(Rejection::NullContract);
        }
    }
    serde_json::from_slice(line).map_err(|_| Rejection::Malformed)
}

/// Whether a `null` in `line` could be the value of a member that
/// [`is_unsupported_nullable_field`] guards.
///
/// Claude Code writes always-null members such as `stop_sequence` and `container` on
/// every assistant line, so the mere presence of `null` would send nearly every line
/// through the value-tree check. A key holding an escape sequence counts as guarded,
/// because its decoded name cannot be compared from the raw bytes; any other false
/// positive only costs the slower validated parse.
fn may_have_unsupported_null_field(line: &[u8]) -> bool {
    memmem::find_iter(line, b"null").any(|null_start| {
        let Some(before_value) = trim_json_whitespace_end(&line[..null_start]).strip_suffix(b":")
        else {
            return false;
        };
        let Some(before_closing_quote) = trim_json_whitespace_end(before_value).strip_suffix(b"\"")
        else {
            return false;
        };
        let key_start = memchr::memrchr(b'"', before_closing_quote).map_or(0, |index| index + 1);
        let key = &before_closing_quote[key_start..];
        key.contains(&b'\\') || is_unsupported_nullable_field(key)
    })
}

fn trim_json_whitespace_end(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

fn has_unsupported_null_field_in_value(root: &serde_json::Value) -> bool {
    let iteration_arrays = [
        root.pointer("/message/usage/iterations"),
        root.pointer("/data/message/message/usage/iterations"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    has_unsupported_null_value(root, false, &iteration_arrays)
}

fn has_unsupported_null_value(
    value: &serde_json::Value,
    allow_iteration_model: bool,
    iteration_arrays: &[&serde_json::Value],
) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(field, value)| {
            if value.is_null() && is_unsupported_nullable_field(field.as_bytes()) {
                return !(allow_iteration_model && field == "model");
            }
            has_unsupported_null_value(value, false, iteration_arrays)
        }),
        serde_json::Value::Array(values) => {
            let is_iteration_array = iteration_arrays
                .iter()
                .any(|array| std::ptr::eq(*array, value));
            values.iter().any(|value| {
                has_unsupported_null_value(value, is_iteration_array, iteration_arrays)
            })
        }
        _ => false,
    }
}

fn is_unsupported_nullable_field(field: &[u8]) -> bool {
    // Match the raw field bytes directly against the known set of fields that
    // are not allowed to be `null`. Comparing byte slices lets rustc dispatch on
    // length before comparing bytes, which avoids both UTF-8 validation and the
    // hash computation a `phf::Set` lookup would incur on every call.
    matches!(
        field,
        b"id"
            | b"cwd"
            | b"model"
            | b"speed"
            | b"costUSD"
            | b"version"
            | b"sessionId"
            | b"requestId"
            | b"isApiErrorMessage"
            | b"cache_read_input_tokens"
            | b"cache_creation_input_tokens"
    )
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
        deserialize_usage_line, extract_session_parts, has_unsupported_null_field,
        load_deduped_entries, may_have_unsupported_null_field, paths::is_project_path_segment,
        usage_files, usage_limit_reset_time_from_line_bytes,
    };
    use crate::{
        Diagnostics, LoadedEntry, ModelBreakdown, PricingMap, Speed, TimestampMs, TokenUsageRaw,
        UsageEntry, UsageMessage,
        cli::{CostMode, SharedArgs},
        format_rfc3339_millis,
    };
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
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
            entries: super::read_usage_file_with_diagnostics(path, tz, mode, pricing, diagnostics)
                .entries
                .into_iter()
                .map(super::daily::ClaudeEntry::into_loaded)
                .collect(),
        }
    }

    fn push_deduped_entry(
        entry: LoadedEntry,
        indexes: &mut super::FxHashMap<u64, super::DedupeIndexVec>,
        entries: &mut Vec<LoadedEntry>,
    ) -> Option<(usize, LoadedEntry)> {
        fn composed(entry: LoadedEntry) -> super::daily::ClaudeEntry<super::EntryMetadata> {
            super::daily::ClaudeEntry {
                key: super::daily::DedupKey {
                    timestamp: entry.timestamp,
                    session_id: Arc::from(super::loaded_entry_session_id(&entry)),
                    message_id: entry.data.message.id,
                    request_id: entry.data.request_id,
                    is_sidechain: entry.data.is_sidechain,
                },
                usage: entry.data.message.usage,
                date: entry.date,
                project: entry.project,
                cost: entry.cost,
                model: entry.model,
                missing_pricing_model: entry.missing_pricing_model,
                stored_cost: entry.stored_cost,
                metadata: super::EntryMetadata {
                    timestamp: entry.data.timestamp,
                    version: entry.data.version,
                    recorded_session_id: entry.data.session_id,
                    raw_model: entry.data.message.model,
                    cost_usd: entry.data.cost_usd,
                    is_api_error_message: entry.data.is_api_error_message,
                    usage_limit_reset_time: entry.usage_limit_reset_time,
                    session_id: entry.session_id,
                    project_path: entry.project_path,
                },
            }
        }
        let mut compact = std::mem::take(entries).into_iter().map(composed).collect();
        let discarded = super::daily::push_deduped_entry(composed(entry), indexes, &mut compact);
        *entries = compact
            .into_iter()
            .map(super::daily::ClaudeEntry::into_loaded)
            .collect();
        discarded.map(|(index, entry)| (index, entry.into_loaded()))
    }

    fn read_usage_file(
        path: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: Option<&PricingMap>,
    ) -> LoadedFile {
        read_usage_file_with_diagnostics(path, tz, mode, pricing, &Diagnostics::new("claude"))
    }

    #[test]
    fn all_report_loaders_drop_timestamp_different_sidechain_replays() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00Z","sessionId":"session-a","requestId":"parent","message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"timestamp":"2026-01-02T10:00:01Z","sessionId":"session-a","requestId":"replay","isSidechain":true,"message":{"id":"msg-parent","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            ].join("\n"),
        });
        let _env = EnvVarsGuard::set_many([(
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
        let _env = EnvVarsGuard::set_many([(
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
        let _env = EnvVarsGuard::set_many([(
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
    fn rejects_null_schema_fields_like_typescript_loader() {
        assert!(has_unsupported_null_field(
            br#"{"message":{"usage":{"speed":null}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":null,"usage":{"input_tokens":0}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"message":{"model": null,"usage":{"input_tokens":0}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"sessionId":null,"message":{"usage":{"input_tokens":0}}}"#
        ));
    }

    #[test]
    fn allows_null_content_like_typescript_loader() {
        assert!(!has_unsupported_null_field(
            br#"{"message":{"content":null,"usage":{"input_tokens":0}}}"#
        ));
    }

    /// Claude Code 2.1.266+ writes `"model":null` for the main-model iteration of Fable 5.1
    /// entries; `UsageIteration.model` is optional, so those lines must be kept while every
    /// other unsupported null keeps being rejected.
    #[test]
    fn allows_null_iteration_model_but_still_rejects_other_nulls() {
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"claude-fable-5-1","usage":{"input_tokens":2,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289}]}}}"#
        ));
        // A missing iteration model and an empty iterations array are unchanged.
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"input_tokens":2,"iterations":[{"type":"message","input_tokens":2}]}}}"#
        ));
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"input_tokens":2,"iterations":[]}}}"#
        ));
        // Strings inside the array may contain brackets or escaped quotes without ending the span.
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"iterations":[{"type":"ad]vi\"sor}","model":null,"input_tokens":1}]}}}"#
        ));
        // `message.model` null is still rejected, before or after the iterations array.
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":null,"usage":{"iterations":[{"type":"message","model":"m"}]}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"message":{"usage":{"iterations":[{"type":"message","model":"m"}]},"model":null}}"#
        ));
        // Other unsupported nulls inside the array are still rejected.
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"iterations":[{"type":"message","model":null,"cache_read_input_tokens":null}]}}}"#
        ));
    }

    /// The exemption is scoped to the `iterations` member of a `usage` object: whitespace
    /// around the member is fine, but a same-named array elsewhere in the line, an `iterations`
    /// member nested deeper inside `usage`, or the key as text inside a string never qualifies.
    #[test]
    fn scopes_the_iteration_model_exemption_to_the_usage_object() {
        // JSON whitespace between the key, the colon and the array is tolerated.
        assert!(!has_unsupported_null_field(
            br#"{"message": {"model": "m", "usage" : { "input_tokens": 2, "iterations" : [ {"type": "message", "model": null} ] }}}"#
        ));
        // Escaped property names are decoded before the structural path check.
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"iter\u0061tions" : [{"type":"message","model" : null}]}}}"#
        ));
        // An earlier `iterations` array in another object does not stand in for the real one.
        assert!(!has_unsupported_null_field(
            br#"{"toolUseResult":{"iterations":[{"model":"x"}]},"message":{"model":"m","usage":{"iterations":[{"type":"message","model":null}]}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"toolUseResult":{"iterations":[{"model":null}]},"message":{"model":"m","usage":{"input_tokens":1}}}"#
        ));
        // A `usage` member that is not an object is skipped in favor of the real one.
        assert!(!has_unsupported_null_field(
            br#"{"usage":"n/a","message":{"model":"m","usage":{"iterations":[{"type":"message","model":null}]}}}"#
        ));
        // Only a direct member of `usage` counts, not one nested deeper inside it.
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"server_tool_use":{"iterations":[{"model":null}]},"input_tokens":1}}}"#
        ));
        // A nested object's model is not the optional model of the iteration itself.
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"iterations":[{"type":"message","metadata":{"model":null}}]}}}"#
        ));
        // The key as text inside a string value is not a member.
        assert!(has_unsupported_null_field(
            br#"{"message":{"content":"\"usage\":{\"iterations\":[","model":null,"usage":{"input_tokens":1}}}"#
        ));
        // Members before `iterations` may hold nested containers and strings with brackets.
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"cache_creation":{"ephemeral_5m_input_tokens":0},"server_tool_use":{"web_search_requests":0},"inference_geo":"[not]{available}","iterations":[{"type":"message","model":null}]}}}"#
        ));
        // Only the direct `message.usage` object may provide the exempted array.
        assert!(!has_unsupported_null_field(
            br#"{"usage":{"iterations":[{"model":"decoy"}]},"message":{"model":"m","usage":{"iterations":[{"type":"message","model":null}]}}}"#
        ));
        // AgentProgress records wrap the assistant message under `data.message.message`.
        assert!(!has_unsupported_null_field(
            br#"{"type":"progress","data":{"message":{"message":{"model":"m","usage":{"iterations":[{"type":"message","model":null}]}}}}}"#
        ));
    }

    #[test]
    fn malformed_usage_members_do_not_panic_the_null_precheck() {
        assert!(!has_unsupported_null_field(
            br#"{"message":{"model":"m","usage":{"input_tokens":,"iterations":[{"type":"message","model":null}]}}}"#
        ));
    }

    #[test]
    fn null_precheck_skips_only_unguarded_members() {
        assert!(!may_have_unsupported_null_field(
            br#"{"message":{"model":"m","stop_sequence":null,"container":null,"content":"null","usage":{"input_tokens":1,"iterations":null}}}"#
        ));
        assert!(!may_have_unsupported_null_field(
            br#"{"message":{"content":[null],"usage":{"input_tokens":1}}}"#
        ));
        assert!(may_have_unsupported_null_field(
            br#"{"message":{"model" :
null,"usage":{"input_tokens":1}}}"#
        ));
        assert!(may_have_unsupported_null_field(
            br#"{"message":{"model":null,"usage":{"input_tokens":1}}}"#
        ));
        assert!(has_unsupported_null_field(
            br#"{"message":{"model":null,"usage":{"input_tokens":1}}}"#
        ));
        assert!(may_have_unsupported_null_field(
            br#"{"message":{"content":"\"id\":null","usage":{"input_tokens":1}}}"#
        ));
    }

    /// Duplicate handling is the typed parse's alone, so it must not depend on whether
    /// the line also carries an allowed or exempted `null`.
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
                deserialize_usage_line::<UsageEntry>(duplicate_known.as_bytes()).is_err(),
                "{duplicate_known}"
            );
            assert!(
                deserialize_usage_line::<UsageEntry>(duplicate_unknown.as_bytes()).is_ok(),
                "{duplicate_unknown}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_members_before_typed_deserialization() {
        assert!(deserialize_usage_line::<UsageEntry>(
            br#"{"type":"assistant","timestamp":"2026-09-12T04:38:42.296Z","sessionId":"session-a","message":{"id":"msg_1","model":"first","model":"second","usage":{"input_tokens":2,"output_tokens":289,"iterations":[{"type":"message","model":null,"input_tokens":2,"output_tokens":289}]}}}"#
        )
        .is_err());
    }

    /// The repro line from #1710 loads as one Fable 5.1 entry with its own token counts.
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
            let _env = EnvVarsGuard::set_many([(
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
        let _env = EnvVarsGuard::set_many([(
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
        let sorted_days = |is_counted: &dyn Fn(&LoadedEntry) -> bool| {
            let mut days: Vec<_> = deduped
                .main_thread_session_days(is_counted)
                .into_iter()
                .map(|(session, date)| (session.to_string(), date))
                .collect();
            days.sort();
            days
        };
        let day = |session: &str, date: &str| (session.to_string(), date.to_string());

        assert_eq!(deduped.entries.len(), 2);
        assert_eq!(
            sorted_days(&|_| true),
            [
                day("session-a", "2026-01-02"),
                day("session-b", "2026-01-02"),
                day("session-b", "2026-01-03"),
            ]
        );
        assert_eq!(
            sorted_days(&|entry| entry.date != "2026-01-02"),
            [day("session-b", "2026-01-03")]
        );
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
