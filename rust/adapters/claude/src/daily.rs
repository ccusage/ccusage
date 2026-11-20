use ccusage_adapter_common::jsonl::{self, Interruption, ReadPolicy};
use ccusage_core::{Diagnostics, SkipKind};
use std::{
    collections::BTreeMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem::Finder;
use serde::Deserialize;

use crate::{
    IteratedTokenUsage, ModelBreakdown, Result, Speed, TimestampMs, TokenCounts, TokenUsageRaw,
    UsageSummary,
    cli::{DayLabel, IsoDate, SharedArgs},
    cost::{PriceTable, price_resolved_usage},
    date_at,
    fast::{FxHashMap, LinePrefilter, suffix_string},
    parse_ts_timestamp,
    pricing::PricingHandle,
};

use super::{
    DedupeIndexVec, GuardedUsage, MainThreadSessions, Nullable, Rejection, UsageLoadContext,
    advisor_usages_from_line, deserialize_usage_line, is_per_response_message_id,
    main_thread_sessions, non_null,
    paths::{
        claude_paths, extract_project, usage_file_order, usage_files_before_since,
        visit_usage_files,
    },
    push_deduped_index, push_deduped_session_alias, push_new_deduped_index, read_discovered_paths,
    reports_more_of_the_request, sidechain_entry_replay_dedupe_hash, sidechain_replay_dedupe_hash,
    usage_dedupe_hash,
};

pub(super) struct DailyEntries {
    deduped: DedupedDailyEntries,
    pub(super) detected: bool,
}

impl DailyEntries {
    pub(super) fn summaries(
        &self,
        group_by_project: bool,
        window: Option<&ccusage_core::ReportWindow>,
    ) -> Vec<UsageSummary> {
        summarize_daily_entries(&self.deduped.entries, group_by_project, window)
    }

    pub(super) fn sessions(&self) -> MainThreadSessions {
        sessions(&self.deduped)
    }
}

pub(super) fn load_daily_entries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<DailyEntries> {
    let paths = claude_paths()?;
    let context = UsageLoadContext::new(shared);
    let mut pruned = Vec::new();
    let deduped = if shared.since.is_some() {
        let since_files = usage_files_before_since(
            &paths,
            project_filter,
            diagnostics,
            shared,
            shared.clock.now().into(),
        );
        pruned = since_files.pruned;
        let mut kept = since_files.kept;
        load_deduped_daily_entries(
            |visit| std::mem::take(&mut kept).into_iter().for_each(visit),
            &context,
            diagnostics,
        )
    } else {
        load_deduped_daily_entries(
            |visit| visit_usage_files(&paths, project_filter, visit, diagnostics),
            &context,
            diagnostics,
        )
    };
    let mut deduped = deduped.unwrap_or(DedupedDailyEntries {
        entries: Vec::new(),
        copies: Vec::new(),
    });
    let detected = !deduped.entries.is_empty() || pruned.iter().any(|file| has_usage(file));
    deduped.retain_entries_in(&ccusage_core::ReportWindow::from_shared(shared));
    Ok(DailyEntries { deduped, detected })
}

const READ_POLICY: ReadPolicy = ReadPolicy {
    capacity: 256 * 1024,
    keep_interrupted_tail: |_| true,
    open_failure: Some("file open failed"),
    interrupted: |_| Interruption::KeepRecords(SkipKind::ReadInterrupted, "file read failed"),
    ..ReadPolicy::PLAIN
};

pub(super) fn has_usage(path: &Path) -> bool {
    jsonl::read(path, None, READ_POLICY, |line, _, _| {
        if admit(line.bytes()).is_ok() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_ok_and(|found| found.is_some())
}

enum Refusal {
    NotUsage,
    Rejected(Rejection),
    InvalidTimestamp,
}

struct Admitted {
    record: ClaudeUsageRecord,
    timestamp: TimestampMs,
    advisors: super::AdvisorUsages,
}

fn has_usage_marker(line: &[u8]) -> bool {
    const MARKER: &[u8] = br#""usage":{"#;
    static VERBATIM: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(MARKER));
    // NOTE(2026-11-19, #2222): RFC 8259 lets a JSON string escape an ASCII letter only as `\u00XX`. A line without `\u00` cannot spell the usage key with escapes, so the escape-aware matcher can be skipped.
    static ASCII_UNICODE_ESCAPE: LazyLock<Finder<'static>> =
        LazyLock::new(|| Finder::new(br"\u00"));
    static FALLBACK: LazyLock<LinePrefilter> = LazyLock::new(|| LinePrefilter::all(&[MARKER]));

    VERBATIM.find(line).is_some()
        || (ASCII_UNICODE_ESCAPE.find(line).is_some() && FALLBACK.matches(line))
}

fn admit(line: &[u8]) -> std::result::Result<Admitted, Refusal> {
    if !has_usage_marker(line) {
        return Err(Refusal::NotUsage);
    }
    let (record, advisor_lookup) = parse_usage_line(line).map_err(Refusal::Rejected)?;
    let timestamp = parse_ts_timestamp(&record.timestamp).ok_or(Refusal::InvalidTimestamp)?;
    if !is_valid_usage_record(&record) {
        return Err(Refusal::Rejected(Rejection::Malformed));
    }
    let advisors = match advisor_lookup.usages(line) {
        Ok(advisors) => advisors,
        Err(Rejection::Malformed) => super::AdvisorUsages {
            malformed: 1,
            ..super::AdvisorUsages::default()
        },
        Err(Rejection::NullContract) => super::AdvisorUsages::default(),
    };
    Ok(Admitted {
        record,
        timestamp,
        advisors,
    })
}

pub(super) struct DedupedDailyEntries {
    entries: Vec<ClaudeEntry>,
    copies: Vec<(usize, Arc<str>)>,
}

impl DedupedDailyEntries {
    fn retain_entries_in(&mut self, window: &ccusage_core::ReportWindow) {
        super::retain_copied_entries(&mut self.entries, &mut self.copies, window, |entry| {
            entry.date
        });
    }
}

fn sessions(deduped: &DedupedDailyEntries) -> MainThreadSessions {
    main_thread_sessions(
        deduped
            .entries
            .iter()
            .map(|entry| (&entry.key.session_id, entry))
            .chain(
                deduped
                    .copies
                    .iter()
                    .map(|(index, session_id)| (session_id, &deduped.entries[*index])),
            )
            .map(|(session_id, survivor)| (session_id.as_ref(), survivor.is_sidechain())),
    )
}

fn load_deduped_daily_entries(
    discover: impl FnMut(&mut dyn FnMut(PathBuf)),
    context: &UsageLoadContext<'_>,
    diagnostics: &Diagnostics,
) -> Option<DedupedDailyEntries> {
    let loaded_files = read_discovered_paths(context.shared.single_thread, discover, |file| {
        read_daily_usage_file_with_diagnostics(
            file,
            context.window().timezone(),
            context.table(),
            diagnostics,
        )
    });
    if loaded_files.is_empty() {
        return None;
    }
    let (entries, copies) = dedupe_files(loaded_files);
    Some(DedupedDailyEntries { entries, copies })
}

pub(super) fn dedupe_files<E: DedupEntry>(
    mut files: Vec<(PathBuf, Vec<E>)>,
) -> (Vec<E>, Vec<(usize, Arc<str>)>) {
    files.sort_by_cached_key(|(path, _)| usage_file_order(path));
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

fn summarize_daily_entries(
    deduped: &[ClaudeEntry],
    group_by_project: bool,
    window: Option<&ccusage_core::ReportWindow>,
) -> Vec<UsageSummary> {
    let includes = |day| window.is_none_or(|window| window.contains_entry_date(day));
    if group_by_project {
        let mut groups = BTreeMap::<(DayLabel, Arc<str>), DailyAccumulator>::new();
        for entry in deduped {
            groups
                .entry((entry.date.into(), Arc::clone(&entry.project)))
                .or_default()
                .add_entry(entry);
        }
        return groups
            .into_iter()
            .filter(|((date, _), _)| includes(date.day()))
            .map(|((date, project), group)| {
                let mut summary = group.into_summary();
                summary.date = Some(date.day().to_string());
                summary.project = Some(project.to_string());
                summary
            })
            .collect();
    }

    let mut groups = BTreeMap::<DayLabel, DailyAccumulator>::new();
    for entry in deduped {
        groups
            .entry(entry.date.into())
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_iter()
        .filter(|(date, _)| includes(date.day()))
        .map(|(key, group)| {
            let mut summary = group.into_summary();
            summary.date = Some(key.day().to_string());
            summary
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct DedupKey {
    pub(super) timestamp: TimestampMs,
    session_id: Arc<str>,
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

#[derive(Debug)]
pub(super) struct ClaudeEntry {
    key: DedupKey,
    date: IsoDate,
    project: Arc<str>,
    usage: TokenUsageRaw,
    cost: f64,
    model: Option<String>,
    missing_pricing_model: Option<String>,
    recorded_cost: RecordedCost,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RecordedCost {
    Absent,
    Unbilled,
    Billed(f64),
}

impl RecordedCost {
    fn new(cost_usd: Option<f64>, billed: Option<f64>) -> Self {
        match (cost_usd, billed) {
            (None, _) => Self::Absent,
            (Some(_), None) => Self::Unbilled,
            (Some(_), Some(cost)) => Self::Billed(cost),
        }
    }

    fn billed(self) -> Option<f64> {
        match self {
            Self::Billed(cost) => Some(cost),
            Self::Absent | Self::Unbilled => None,
        }
    }
}

impl DedupEntry for ClaudeEntry {
    fn timestamp(&self) -> TimestampMs {
        self.key.timestamp
    }

    fn session_id(&self) -> &str {
        &self.key.session_id
    }

    fn owned_session_id(&self) -> Arc<str> {
        Arc::clone(&self.key.session_id)
    }

    fn message_id(&self) -> Option<&str> {
        self.key.message_id.as_deref()
    }

    fn request_id(&self) -> Option<&str> {
        self.key.request_id.as_deref()
    }

    fn is_sidechain(&self) -> bool {
        self.key.is_sidechain == Some(true)
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        !matches!(self.recorded_cost, RecordedCost::Absent)
    }
}

pub(super) struct ParsedUsage {
    pub(super) key: DedupKey,
    pub(super) date: IsoDate,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) missing_pricing_model: Option<String>,
    pub(super) stored_cost: Option<f64>,
    pub(super) raw_model: Option<String>,
    pub(super) timestamp_text: String,
    pub(super) version: Option<String>,
    pub(super) recorded_session_id: Option<String>,
    pub(super) cost_usd: Option<f64>,
    pub(super) is_api_error_message: Option<bool>,
}

impl ParsedUsage {
    pub(super) fn display_model(
        raw_model: Option<String>,
        usage: &TokenUsageRaw,
    ) -> Option<String> {
        raw_model.and_then(|model| {
            if model == "<synthetic>" {
                None
            } else if matches!(usage.speed, Some(Speed::Fast)) {
                Some(suffix_string(&model, "-fast"))
            } else {
                Some(model)
            }
        })
    }

    fn into_daily(self) -> ClaudeEntry {
        ClaudeEntry {
            model: Self::display_model(self.raw_model, &self.usage),
            key: self.key,
            date: self.date,
            project: self.project,
            usage: self.usage,
            cost: self.cost,
            missing_pricing_model: self.missing_pricing_model,
            recorded_cost: RecordedCost::new(self.cost_usd, self.stored_cost),
        }
    }
}

#[derive(Debug)]
pub(super) struct ClaudeUsageRecord {
    timestamp: String,
    pub(super) message: ClaudeUsageMessage,
    version: Option<String>,
    session_id: Option<String>,
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    is_api_error_message: Option<bool>,
}

#[derive(Debug)]
pub(super) struct ClaudeUsageMessage {
    pub(super) usage: IteratedTokenUsage,
    model: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUsageRecord {
    timestamp: String,
    message: RawUsageMessage,
    #[serde(default, deserialize_with = "super::nullable")]
    version: Nullable<String>,
    #[serde(default, deserialize_with = "super::nullable")]
    session_id: Nullable<String>,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "super::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "super::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
    #[serde(default, deserialize_with = "super::nullable")]
    is_api_error_message: Nullable<bool>,
}

impl RawUsageRecord {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord, Rejection> {
        Ok(ClaudeUsageRecord {
            timestamp: self.timestamp,
            message: self.message.decode()?,
            version: non_null(self.version)?,
            session_id: non_null(self.session_id)?,
            cost_usd: non_null(self.cost_usd)?,
            request_id: non_null(self.request_id)?,
            is_sidechain: self.is_sidechain,
            is_api_error_message: non_null(self.is_api_error_message)?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RawUsageMessage {
    usage: GuardedUsage,
    #[serde(default, deserialize_with = "super::nullable")]
    model: Nullable<String>,
    #[serde(default, deserialize_with = "super::nullable")]
    id: Nullable<String>,
}

impl RawUsageMessage {
    fn decode(self) -> std::result::Result<ClaudeUsageMessage, Rejection> {
        if self.usage.has_null {
            return Err(Rejection::NullContract);
        }
        Ok(ClaudeUsageMessage {
            usage: self.usage.usage,
            model: non_null(self.model)?,
            id: non_null(self.id)?,
        })
    }
}

pub(super) fn parse_usage_line(
    line: &[u8],
) -> std::result::Result<(ClaudeUsageRecord, AdvisorLookup), Rejection> {
    match deserialize_usage_line::<RawUsageRecord>(line) {
        Ok(raw) => {
            let entry = raw.decode()?;
            let lookup = if entry.message.usage.has_advisor_iteration {
                AdvisorLookup::AskLine
            } else {
                AdvisorLookup::RuledOut
            };
            Ok((entry, lookup))
        }
        Err(_) => deserialize_usage_line::<AgentProgressEntry>(line)
            .and_then(AgentProgressEntry::decode)
            .map(|entry| {
                let lookup = if entry.message.usage.has_advisor_iteration {
                    AdvisorLookup::AskProgress
                } else {
                    AdvisorLookup::RuledOut
                };
                (entry, lookup)
            }),
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum AdvisorLookup {
    RuledOut,
    AskLine,
    AskProgress,
}

impl AdvisorLookup {
    fn usages(self, line: &[u8]) -> std::result::Result<super::AdvisorUsages, Rejection> {
        match self {
            Self::RuledOut => Ok(super::AdvisorUsages::default()),
            Self::AskLine => advisor_usages_from_line(line),
            Self::AskProgress => super::advisor_usages_from_progress_line(line),
        }
    }
}

// NOTE(2026-05-19, #977): Claude Code also writes assistant usage under `data.message.message` in AgentProgress records. Read that envelope for subagent usage.
#[derive(Debug, Deserialize)]
struct AgentProgressEntry {
    #[serde(rename = "sessionId")]
    #[serde(default, deserialize_with = "super::nullable")]
    session_id: Nullable<String>,
    data: AgentProgressData,
}

impl AgentProgressEntry {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord, Rejection> {
        let message = self.data.message;
        Ok(ClaudeUsageRecord {
            timestamp: message.timestamp,
            message: message.message.decode()?,
            version: None,
            is_api_error_message: non_null(message.is_api_error_message)?,
            session_id: non_null(self.session_id)?,
            cost_usd: non_null(message.cost_usd)?,
            request_id: non_null(message.request_id)?,
            is_sidechain: message.is_sidechain,
        })
    }
}

#[derive(Debug, Deserialize)]
struct AgentProgressData {
    message: AgentProgressMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProgressMessage {
    #[serde(default, deserialize_with = "super::nullable")]
    is_api_error_message: Nullable<bool>,
    timestamp: String,
    message: RawUsageMessage,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "super::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "super::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
}

fn read_daily_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<ClaudeEntry> {
    read_usage_file(path, tz, pricing, diagnostics, |parsed, _| {
        parsed.into_daily()
    })
}

struct FilePricing<'a> {
    table: PriceTable<'a>,
    models: smallvec::SmallVec<[(Box<str>, Option<PricingHandle>); 1]>,
}

impl FilePricing<'_> {
    fn price(
        &mut self,
        model: Option<&str>,
        usage: TokenUsageRaw,
        stored_cost: Option<f64>,
        timestamp: TimestampMs,
    ) -> (f64, Option<String>) {
        let table = match self.table {
            PriceTable::Display => return (stored_cost.unwrap_or_default(), None),
            PriceTable::Auto(_) if stored_cost.is_some() => {
                return (stored_cost.unwrap_or_default(), None);
            }
            PriceTable::Auto(table) | PriceTable::Calculate(table) => table,
        };
        let Some(model) = model else {
            return (0.0, None);
        };
        if let Some((_, handle)) = self.models.iter().find(|(name, _)| name.as_ref() == model) {
            return price_resolved_usage(model, usage, timestamp, handle.as_ref());
        }
        let handle = table.find_handle(model);
        let price = price_resolved_usage(model, usage, timestamp, handle.as_ref());
        self.models.push((model.into(), handle));
        price
    }
}

pub(super) fn read_usage_file<E>(
    path: &Path,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
    build: impl Fn(ParsedUsage, &[u8]) -> E,
) -> Vec<E> {
    let project: Arc<str> = Arc::from(extract_project(path));
    let (path_session_id, _) = super::paths::extract_session_parts(path);
    let path_session_id: Arc<str> = Arc::from(path_session_id);
    let mut recorded_session_id = Arc::clone(&path_session_id);
    let mut entries = Vec::new();
    let mut file_pricing = FilePricing {
        table: pricing,
        models: smallvec::SmallVec::new(),
    };
    let _ = jsonl::read(path, Some(diagnostics), READ_POLICY, |current, _, _| {
        let line = current.bytes();
        let Admitted {
            record: data,
            timestamp,
            advisors,
        } = match admit(line) {
            Ok(admitted) => admitted,
            Err(Refusal::NotUsage | Refusal::Rejected(Rejection::NullContract)) => {
                return ControlFlow::Continue(());
            }
            Err(Refusal::Rejected(Rejection::Malformed)) => {
                super::diagnostics::rejected(path, current, diagnostics);
                return ControlFlow::Continue(());
            }
            Err(Refusal::InvalidTimestamp) => {
                super::diagnostics::invalid_timestamp(path, current.index, diagnostics);
                return ControlFlow::Continue(());
            }
        };
        for _ in 0..advisors.malformed {
            diagnostics.record(ccusage_core::Skip::record(
                path,
                ccusage_core::Skip::line_number(current.index),
                ccusage_core::SkipKind::InvalidData,
                "invalid advisor usage",
            ));
        }
        let IteratedTokenUsage { usage, .. } = data.message.usage;
        let raw_model = data.message.model;
        let (cost, missing_pricing_model) =
            file_pricing.price(raw_model.as_deref(), usage, data.cost_usd, timestamp);
        let session_id = if let Some(session_id) = data.session_id.as_deref() {
            if recorded_session_id.as_ref() != session_id {
                recorded_session_id = Arc::from(session_id);
            }
            Arc::clone(&recorded_session_id)
        } else {
            Arc::clone(&path_session_id)
        };
        let parent = ParsedUsage {
            key: DedupKey {
                timestamp,
                session_id,
                message_id: data.message.id,
                request_id: data.request_id,
                is_sidechain: data.is_sidechain,
            },
            date: date_at(timestamp, tz),
            project: Arc::clone(&project),
            usage,
            cost,
            missing_pricing_model,
            stored_cost: data
                .cost_usd
                .filter(|_| matches!(pricing, PriceTable::Auto(_))),
            raw_model,
            timestamp_text: data.timestamp,
            version: data.version,
            recorded_session_id: data.session_id,
            cost_usd: data.cost_usd,
            is_api_error_message: data.is_api_error_message,
        };
        let advisors = advisors
            .usages
            .into_iter()
            .enumerate()
            .map(|(index, advisor)| {
                let (cost, missing_pricing_model) =
                    file_pricing.price(Some(&advisor.model), advisor.usage, None, timestamp);
                ParsedUsage {
                    key: DedupKey {
                        timestamp,
                        session_id: Arc::clone(&parent.key.session_id),
                        message_id: parent
                            .key
                            .message_id
                            .as_ref()
                            .map(|id| format!("{id}:advisor:{index}")),
                        request_id: parent.key.request_id.clone(),
                        is_sidechain: parent.key.is_sidechain,
                    },
                    date: parent.date,
                    project: Arc::clone(&project),
                    usage: advisor.usage,
                    cost,
                    missing_pricing_model,
                    stored_cost: None,
                    raw_model: Some(advisor.model),
                    timestamp_text: parent.timestamp_text.clone(),
                    version: parent.version.clone(),
                    recorded_session_id: parent.recorded_session_id.clone(),
                    cost_usd: None,
                    is_api_error_message: parent.is_api_error_message,
                }
            })
            .collect::<Vec<_>>();
        entries.push(build(parent, line));
        entries.extend(advisors.into_iter().map(|advisor| build(advisor, line)));
        ControlFlow::<()>::Continue(())
    });
    entries
}

fn is_valid_usage_record(data: &ClaudeUsageRecord) -> bool {
    if data
        .version
        .as_deref()
        .is_some_and(|version| !is_semver_prefix(version))
    {
        return false;
    }
    if data
        .session_id
        .as_deref()
        .is_some_and(|session_id| session_id.is_empty())
    {
        return false;
    }
    if data
        .request_id
        .as_deref()
        .is_some_and(|request_id| request_id.is_empty())
    {
        return false;
    }
    if data
        .message
        .id
        .as_deref()
        .is_some_and(|message_id| message_id.is_empty())
    {
        return false;
    }
    if data
        .message
        .model
        .as_deref()
        .is_some_and(|model| model.is_empty())
    {
        return false;
    }
    true
}

fn is_semver_prefix(value: &str) -> bool {
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

#[derive(Default)]
struct DailyAccumulator {
    counts: TokenCounts,
    cost: f64,
    models: Vec<String>,
    breakdowns: Vec<ModelBreakdown>,
    breakdown_indexes: FxHashMap<String, usize>,
}

impl DailyAccumulator {
    fn add_entry(&mut self, entry: &ClaudeEntry) {
        self.counts.add_usage(entry.usage);
        self.cost += entry.cost;
        if let Some(model) = &entry.model {
            let model = crate::model_aliases::resolve_model_name(model);
            let index = if let Some(index) = self.breakdown_indexes.get(model.as_ref()) {
                *index
            } else {
                let model = model.into_owned();
                let index = self.breakdowns.len();
                self.breakdown_indexes.insert(model.clone(), index);
                self.models.push(model.clone());
                self.breakdowns.push(ModelBreakdown {
                    model_name: model,
                    ..ModelBreakdown::default()
                });
                index
            };
            let breakdown = &mut self.breakdowns[index];
            breakdown.add_usage(&entry.usage);
            breakdown.cost += entry.cost;
            if entry.missing_pricing_model.is_some() {
                breakdown.missing_pricing = true;
            }
            breakdown.add_stored_cost(entry.recorded_cost.billed());
        }
    }

    fn into_summary(mut self) -> UsageSummary {
        self.breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: self.counts.input_tokens,
            output_tokens: self.counts.output_tokens,
            cache_creation_tokens: self.counts.cache_creation_tokens,
            cache_read_tokens: self.counts.cache_read_tokens,
            extra_total_tokens: 0,
            total_cost: self.cost,
            credits: None,
            message_count: None,
            models_used: self.models,
            model_breakdowns: self.breakdowns,
            project: None,
            versions: None,
        }
    }
}

#[cfg(test)]
mod tests;
