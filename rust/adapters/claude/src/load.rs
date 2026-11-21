use std::{
    borrow::Cow,
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use ccusage_adapter_common::{
    read_discovered_paths,
    source::{ProgressOwner, UsageLoadContext},
};
use ccusage_core::{
    Diagnostics, LoadedEntry, Result, TimestampMs, TokenUsageRaw, UsageSummary,
    cli::{CostMode, SharedArgs},
    debug_log, progress,
};
use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem;

use super::{
    claude_paths, containing_projects_root, daily, dedupe, dedupe::DedupEntry as _,
    extract_session_parts, projects_roots, read_usage_files_since, visit_usage_files,
};

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

pub(super) fn load_deduped_entries_with_progress(
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
    pub(super) entries: Vec<ClaudeEntry>,
    pub(super) detected: bool,
    pub(super) copies: Vec<(usize, Arc<str>)>,
}

#[derive(Debug, Clone)]
pub(super) struct ClaudeEntry {
    pub(super) source_timestamp: String,
    pub(super) recorded_session_id: Option<Arc<str>>,
    pub(super) original_model: Option<String>,
    pub(super) request: dedupe::Request,
    pub(super) is_sidechain: bool,
    pub(super) entry: LoadedEntry,
}

impl std::ops::Deref for ClaudeEntry {
    type Target = LoadedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

impl DedupedEntries {
    pub(super) fn main_thread_sessions(&self) -> MainThreadSessions {
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

pub(super) fn main_thread_sessions<'a>(
    entries: impl Iterator<Item = (&'a str, bool)>,
) -> MainThreadSessions {
    entries
        .filter(|(_, is_sidechain)| !is_sidechain)
        .map(|(session_id, _)| session_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(Arc::from)
        .collect()
}

pub(crate) struct DailySummaries {
    pub(super) summaries: Vec<UsageSummary>,
    pub(super) detected: bool,
    pub(super) sessions: MainThreadSessions,
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

pub(super) fn load_windowed_entries(
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

pub(super) fn retain_copied_entries<T>(
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
    let projects_roots = projects_roots(&paths);
    let read = |file: &PathBuf| {
        read_usage_file_with_diagnostics(
            file,
            containing_projects_root(file, &projects_roots),
            context.window().timezone(),
            context.table_for_mode(mode),
            diagnostics,
        )
    };
    let mut pruned = Vec::new();
    let loaded_files = if skip_files_before_since && shared.since.is_some() {
        let split = read_usage_files_since(
            &paths,
            project_filter,
            diagnostics,
            shared,
            shared.clock.now().into(),
            read,
        );
        debug_log(
            shared,
            format!(
                "Found {} JSONL usage files",
                split.kept.len() + split.pruned.len()
            ),
        );
        pruned = split.pruned;
        debug_log(
            shared,
            format!(
                "Kept {} JSONL usage files inside the --since window",
                split.kept.len()
            ),
        );
        split.kept
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
    let (entries, copies) = dedupe::dedupe_files(loaded_files);
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

fn loaded_entry_session_id(entry: &ClaudeEntry) -> &str {
    entry
        .recorded_session_id
        .as_deref()
        .unwrap_or(entry.session_id.as_ref())
}

impl dedupe::DedupEntry for ClaudeEntry {
    fn timestamp(&self) -> TimestampMs {
        self.timestamp
    }

    fn session_id(&self) -> &str {
        loaded_entry_session_id(self)
    }

    fn owned_session_id(&self) -> Arc<str> {
        Arc::clone(
            self.recorded_session_id
                .as_ref()
                .unwrap_or(&self.session_id),
        )
    }

    fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    fn request(&self) -> &dedupe::Request {
        &self.request
    }

    fn is_sidechain(&self) -> bool {
        self.is_sidechain
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        self.recorded_cost.is_some()
    }
}

pub(super) fn read_usage_file_with_diagnostics(
    path: &Path,
    projects_root: Option<&Path>,
    tz: &JiffTimeZone,
    pricing: ccusage_core::cost::PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<ClaudeEntry> {
    let (session_id, project_path) = extract_session_parts(path, projects_root);
    let session_id: Arc<str> = Arc::from(session_id);
    let project_path: Arc<str> = Arc::from(project_path);
    daily::read_usage_file(
        path,
        projects_root,
        tz,
        pricing,
        diagnostics,
        |parsed, line| {
            let model = daily::ParsedUsage::display_model(parsed.raw_model.clone(), &parsed.usage);
            ClaudeEntry {
                source_timestamp: parsed.timestamp_text.into_owned(),
                recorded_session_id: parsed.recorded_session_id,
                original_model: parsed.raw_model,
                request: parsed.key.request,
                is_sidechain: parsed.is_sidechain,
                entry: LoadedEntry {
                    model,
                    usage_limit_reset_time: usage_limit_reset_time_from_line_bytes(
                        line,
                        parsed.is_api_error_message,
                    ),
                    usage: parsed.usage,
                    message_id: parsed.key.message_id.map(String::from),
                    version: parsed.version.map(Cow::into_owned),
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
                    estimated_pricing: parsed.estimated_pricing,
                    stored_cost: parsed.stored_cost,
                },
            }
        },
    )
}

pub(super) fn usage_limit_reset_time_from_line_bytes(
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
