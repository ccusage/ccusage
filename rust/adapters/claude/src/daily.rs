use ccusage_core::Diagnostics;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    IteratedTokenUsage, ModelBreakdown, PricingMap, Result, Speed, TimestampMs, TokenCounts,
    TokenUsageRaw, UsageSummary, calculate_cost_for_usage_at,
    cli::{CostMode, SharedArgs},
    fast::{FxHashMap, LinePrefilter, suffix_string},
    format_date_tz, missing_pricing_model_for_usage, parse_ts_timestamp, stored_cost_for_mode,
    utc_now,
};

use super::{
    DailySummaries, DedupeIndexVec, Rejection, SessionDays, UsageLoadContext,
    advisor_usages_from_line, carries_more_usage_detail, deserialize_usage_line,
    is_per_response_message_id, is_semver_prefix, main_thread_session_days,
    paths::{
        claude_paths, extract_project, split_files_before_since, usage_file_order, usage_files,
        visit_usage_files,
    },
    push_deduped_index, push_deduped_session_alias, push_new_deduped_index, read_discovered_files,
    sidechain_entry_replay_dedupe_hash, sidechain_replay_dedupe_hash, usage_dedupe_hash,
};

pub(super) fn load_daily_summaries_inner(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    group_by_project: bool,
    collect_session_days: bool,
    diagnostics: &Diagnostics,
) -> Result<DailySummaries> {
    let paths = claude_paths()?;
    let context = UsageLoadContext::new(shared);
    let mut pruned = Vec::new();
    let deduped = if shared.since.is_some() {
        let since_files = split_files_before_since(
            usage_files(&paths, project_filter, diagnostics),
            shared,
            utc_now(),
        );
        pruned = since_files.pruned;
        let kept = since_files.kept;
        load_deduped_daily_entries(
            |visit| kept.into_iter().for_each(visit),
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
    let deduped = match deduped {
        Some(deduped) => deduped,
        None if pruned.is_empty() => {
            return Ok(DailySummaries {
                summaries: Vec::new(),
                detected: false,
                session_days: SessionDays::default(),
            });
        }
        None => DedupedDailyEntries {
            entries: Vec::new(),
            copies: Vec::new(),
        },
    };
    let session_days = if collect_session_days {
        session_days(&deduped)
    } else {
        SessionDays::default()
    };
    let summaries = summarize_daily_entries(&deduped.entries, group_by_project);
    let detected = !summaries.is_empty() || pruned.iter().any(|file| has_usage(file));
    Ok(DailySummaries {
        summaries,
        detected,
        session_days,
    })
}

fn has_usage(path: &Path) -> bool {
    use std::{
        fs::File,
        io::{BufRead, BufReader},
    };
    let Ok(file) = File::open(path) else {
        return false;
    };
    let marker = LinePrefilter::all(&[br#""usage":{"#]);
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {
                if marker.matches(&line)
                    && let Ok((data, _)) = parse_usage_line(&line)
                    && is_valid_usage_record(&data)
                    && parse_ts_timestamp(&data.timestamp).is_some()
                {
                    return true;
                }
            }
        }
    }
}

/// Deduplicated daily entries plus the sessions of main-thread copies dedup
/// merged into them from other sessions, such as a resumed or forked
/// transcript repeating a turn, as `(survivor index, copy session)` pairs.
pub(super) struct DedupedDailyEntries {
    entries: Vec<ClaudeEntry>,
    copies: Vec<(usize, Arc<str>)>,
}

/// Counts a copy's session on the day its survivor is counted: a tool wrapping
/// Claude Code can link its row to the session it ran in even when dedup kept
/// another session's copy.
pub(super) fn session_days(deduped: &DedupedDailyEntries) -> SessionDays {
    main_thread_session_days(
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
            .map(|(session_id, survivor)| {
                (
                    session_id.as_ref(),
                    survivor.date.as_str(),
                    survivor.key.is_sidechain,
                )
            }),
    )
}

/// Loads and deduplicates the files `discover` yields, or `None` when it
/// yields none.
fn load_deduped_daily_entries(
    discover: impl FnOnce(&mut dyn FnMut(PathBuf)),
    context: &UsageLoadContext<'_>,
    diagnostics: &Diagnostics,
) -> Option<DedupedDailyEntries> {
    let mode = context.shared.mode;
    let mut loaded_files = read_discovered_files(context.shared.single_thread, discover, |file| {
        read_daily_usage_file_with_diagnostics(
            file,
            context.timezone(),
            mode,
            context.pricing(),
            diagnostics,
        )
    });
    if loaded_files.is_empty() {
        return None;
    }
    // Dedup keeps the first copy it meets, so files are folded in listing order.
    loaded_files.sort_by_cached_key(|(path, _)| usage_file_order(path));

    let entry_count = loaded_files
        .iter()
        .map(|(_, file)| file.entries.len())
        .sum();
    let mut deduped_indexes: FxHashMap<u64, DedupeIndexVec> =
        FxHashMap::with_capacity_and_hasher(entry_count, Default::default());
    let mut deduped = Vec::with_capacity(entry_count);
    let mut copies = Vec::new();
    for (_, loaded_file) in loaded_files {
        for entry in loaded_file.entries {
            if let Some((index, discarded)) =
                push_deduped_entry(entry, &mut deduped_indexes, &mut deduped)
                && discarded.key.session_id != deduped[index].key.session_id
                && !is_sidechain_entry(&discarded)
            {
                copies.push((index, discarded.key.session_id));
            }
        }
    }
    Some(DedupedDailyEntries {
        entries: deduped,
        copies,
    })
}

fn summarize_daily_entries(deduped: &[ClaudeEntry], group_by_project: bool) -> Vec<UsageSummary> {
    if group_by_project {
        let mut groups = BTreeMap::<(String, Arc<str>), DailyAccumulator>::new();
        for entry in deduped {
            groups
                .entry((entry.date.clone(), Arc::clone(&entry.project)))
                .or_default()
                .add_entry(entry);
        }
        return groups
            .into_iter()
            .map(|((date, project), group)| {
                let mut summary = group.into_summary();
                summary.date = Some(date);
                summary.project = Some(project.to_string());
                summary
            })
            .collect();
    }

    let mut groups = BTreeMap::<String, DailyAccumulator>::new();
    for entry in deduped {
        if let Some(group) = groups.get_mut(entry.date.as_str()) {
            group.add_entry(entry);
        } else {
            groups
                .entry(entry.date.clone())
                .or_default()
                .add_entry(entry);
        }
    }
    groups
        .into_iter()
        .map(|(key, group)| {
            let mut summary = group.into_summary();
            summary.date = Some(key);
            summary
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct UsageFile<M = ()> {
    pub(super) entries: Vec<ClaudeEntry<M>>,
}

#[derive(Debug)]
pub(super) struct DedupKey {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: Arc<str>,
    pub(super) message_id: Option<String>,
    pub(super) request_id: Option<String>,
    pub(super) is_sidechain: Option<bool>,
}

#[derive(Debug)]
pub(super) struct ClaudeEntry<M = ()> {
    pub(super) key: DedupKey,
    pub(super) date: String,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) model: Option<String>,
    pub(super) missing_pricing_model: Option<String>,
    pub(super) stored_cost: Option<f64>,
    pub(super) metadata: M,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClaudeUsageRecord {
    pub(super) timestamp: String,
    pub(super) message: ClaudeUsageMessage,
    pub(super) version: Option<String>,
    pub(super) session_id: Option<String>,
    #[serde(rename = "costUSD")]
    pub(super) cost_usd: Option<f64>,
    pub(super) request_id: Option<String>,
    pub(super) is_sidechain: Option<bool>,
    pub(super) is_api_error_message: Option<bool>,
}

/// Parses a direct assistant line, falling back to the AgentProgress wrapper.
///
/// Each shape is tried against the raw bytes in the order `#[serde(untagged)]` would
/// use; the derive would first buffer every line, message content included, into an
/// intermediate tree.
fn parse_usage_line(
    line: &[u8],
) -> std::result::Result<(ClaudeUsageRecord, AdvisorLookup), Rejection> {
    match deserialize_usage_line::<ClaudeUsageRecord>(line) {
        Ok(entry) => {
            let lookup = if entry.message.usage.has_advisor_iteration {
                AdvisorLookup::AskLine
            } else {
                AdvisorLookup::RuledOut
            };
            Ok((entry, lookup))
        }
        Err(Rejection::Malformed) => deserialize_usage_line::<AgentProgressEntry>(line)
            .map(|entry| (entry.into_entry(), AdvisorLookup::AskLine)),
        Err(rejection) => Err(rejection),
    }
}

#[derive(Debug, Clone, Copy)]
enum AdvisorLookup {
    RuledOut,
    AskLine,
}

#[derive(Debug, Deserialize)]
struct AgentProgressEntry {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    data: AgentProgressData,
}

impl AgentProgressEntry {
    fn into_entry(self) -> ClaudeUsageRecord {
        ClaudeUsageRecord {
            timestamp: self.data.message.timestamp,
            message: self.data.message.message,
            version: None,
            is_api_error_message: self.data.message.is_api_error_message,
            session_id: self.session_id,
            cost_usd: self.data.message.cost_usd,
            request_id: self.data.message.request_id,
            is_sidechain: self.data.message.is_sidechain,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgentProgressData {
    message: AgentProgressMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProgressMessage {
    is_api_error_message: Option<bool>,
    timestamp: String,
    message: ClaudeUsageMessage,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ClaudeUsageMessage {
    pub(super) usage: IteratedTokenUsage,
    pub(super) model: Option<String>,
    pub(super) id: Option<String>,
}

fn read_daily_usage_file_with_diagnostics(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> UsageFile {
    read_usage_file_with_metadata(path, tz, mode, pricing, diagnostics, |_, _, _| ())
}

pub(super) fn read_usage_file_with_metadata<M>(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
    metadata: impl Fn(&ClaudeUsageRecord, &[u8], Option<&super::AdvisorUsage>) -> M,
) -> UsageFile<M> {
    let project: Arc<str> = Arc::from(extract_project(path));
    let (path_session_id, _) = super::paths::extract_session_parts(path);
    let path_session_id: Arc<str> = Arc::from(path_session_id);
    let mut recorded_session_id = Arc::clone(&path_session_id);
    let mut loaded_file = UsageFile {
        entries: Vec::new(),
    };
    let usage_marker = LinePrefilter::all(&[br#""usage":{"#]);
    super::diagnostics::for_each_line(path, diagnostics, |current| {
        let (index, line) = (current.index, current.bytes);
        if !usage_marker.matches(line) {
            return;
        }
        let (mut data, advisor_lookup) = match parse_usage_line(line) {
            Ok(parsed) => parsed,
            Err(Rejection::NullContract) => return,
            Err(Rejection::Malformed) => {
                super::diagnostics::rejected(path, current, diagnostics);
                return;
            }
        };
        let Some(timestamp) = parse_ts_timestamp(&data.timestamp) else {
            super::diagnostics::invalid_timestamp(path, index, diagnostics);
            return;
        };
        if !is_valid_usage_record(&data) {
            super::diagnostics::rejected(path, current, diagnostics);
            return;
        }
        let parent_metadata = metadata(&data, line, None);
        let IteratedTokenUsage { usage, .. } = data.message.usage;
        let cost = calculate_cost_for_usage_at(
            data.message.model.as_deref(),
            usage,
            data.cost_usd,
            Some(timestamp),
            mode,
            pricing,
        );
        let missing_pricing_model = missing_pricing_model_for_usage(
            data.message.model.as_deref(),
            usage,
            data.cost_usd,
            mode,
            pricing,
        );
        let model = data.message.model.take().and_then(|model| {
            if model == "<synthetic>" {
                None
            } else if matches!(usage.speed, Some(Speed::Fast)) {
                Some(suffix_string(&model, "-fast"))
            } else {
                Some(model)
            }
        });
        let session_id = if let Some(session_id) = data.session_id.as_deref() {
            if recorded_session_id.as_ref() != session_id {
                recorded_session_id = Arc::from(session_id);
            }
            Arc::clone(&recorded_session_id)
        } else {
            Arc::clone(&path_session_id)
        };
        let parent_index = loaded_file.entries.len();
        loaded_file.entries.push(ClaudeEntry {
            key: DedupKey {
                timestamp,
                session_id,
                message_id: data.message.id.take(),
                request_id: data.request_id.take(),
                is_sidechain: data.is_sidechain,
            },
            date: format_date_tz(timestamp, tz),
            project: Arc::clone(&project),
            usage,
            cost,
            model,
            missing_pricing_model,
            stored_cost: stored_cost_for_mode(data.cost_usd, mode),
            metadata: parent_metadata,
        });
        if matches!(advisor_lookup, AdvisorLookup::RuledOut) {
            return;
        }
        for (index, advisor) in advisor_usages_from_line(line).into_iter().enumerate() {
            let metadata = metadata(&data, line, Some(&advisor));
            let parent = &loaded_file.entries[parent_index];
            let missing_pricing_model = missing_pricing_model_for_usage(
                Some(&advisor.model),
                advisor.usage,
                None,
                mode,
                pricing,
            );
            loaded_file.entries.push(ClaudeEntry {
                key: DedupKey {
                    timestamp,
                    session_id: Arc::clone(&parent.key.session_id),
                    message_id: parent
                        .key
                        .message_id
                        .as_ref()
                        .map(|id| format!("{id}:advisor:{index}")),
                    request_id: parent.key.request_id.clone(),
                    is_sidechain: data.is_sidechain,
                },
                date: parent.date.clone(),
                project: Arc::clone(&project),
                usage: advisor.usage,
                cost: calculate_cost_for_usage_at(
                    Some(&advisor.model),
                    advisor.usage,
                    None,
                    Some(timestamp),
                    mode,
                    pricing,
                ),
                model: Some(advisor.model),
                missing_pricing_model,
                stored_cost: None,
                metadata,
            });
        }
    });
    loaded_file
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
fn usage_token_total<M>(entry: &ClaudeEntry<M>) -> u64 {
    entry.usage.input_tokens
        + entry.usage.output_tokens
        + entry.usage.cache_creation_token_count()
        + entry.usage.cache_read_input_tokens
}

pub(super) fn push_deduped_entry<M>(
    entry: ClaudeEntry<M>,
    deduped_indexes: &mut FxHashMap<u64, DedupeIndexVec>,
    deduped: &mut Vec<ClaudeEntry<M>>,
) -> Option<(usize, ClaudeEntry<M>)> {
    let dedupe_lookup = entry.key.message_id.as_deref().map(|message_id| {
        let request_id = entry.key.request_id.as_deref();
        let exact_hash = usage_dedupe_hash(
            message_id,
            request_id,
            entry.key.session_id.as_ref(),
            entry.key.timestamp,
        );
        let existing_index = deduped_indexes
            .get(&exact_hash)
            .and_then(|indexes| {
                indexes.iter().find_map(|dedupe_index| {
                    let existing = &deduped[dedupe_index.index];
                    (existing.key.message_id.as_deref() == Some(message_id)
                        && existing.key.request_id.as_deref() == request_id
                        && (request_id.is_some()
                            || (existing.key.session_id == entry.key.session_id
                                && (is_per_response_message_id(message_id)
                                    || existing.key.timestamp == entry.key.timestamp))))
                        .then_some(dedupe_index.index)
                })
            })
            .or_else(|| {
                // /btw sidechain logs can replay parent messages with new request IDs. A parent
                // candidate only has to look through sidechain entries.
                let candidate_is_sidechain = is_sidechain_entry(&entry);
                let route_hash = if candidate_is_sidechain {
                    sidechain_replay_dedupe_hash(message_id, entry.key.session_id.as_ref())
                } else {
                    sidechain_entry_replay_dedupe_hash(message_id, entry.key.session_id.as_ref())
                };
                deduped_indexes.get(&route_hash).and_then(|indexes| {
                    indexes.iter().find_map(|dedupe_index| {
                        let existing = &deduped[dedupe_index.index];
                        let indexed_session_id = dedupe_index
                            .session_alias
                            .as_deref()
                            .unwrap_or(existing.key.session_id.as_ref());
                        (existing.key.message_id.as_deref() == Some(message_id)
                            && indexed_session_id == entry.key.session_id.as_ref()
                            && (candidate_is_sidechain || is_sidechain_entry(existing)))
                        .then_some(dedupe_index.index)
                    })
                })
            });
        (exact_hash, existing_index)
    });

    if let Some((hash, Some(index))) = dedupe_lookup {
        if deduped[index].key.session_id != entry.key.session_id
            && let Some(message_id) = entry.key.message_id.as_deref()
        {
            // Cross-session copies can become the survivor, so keep every session route used by
            // later sidechain replays.
            let existing_session_id = deduped[index].key.session_id.as_ref();
            for session_id in [entry.key.session_id.as_ref(), existing_session_id] {
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
            // The survivor is already indexed under its previous keys. Only register the keys
            // that changed, so repeated same-timestamp rewrites do not rescan large buckets.
            if entry_exact_hash(&previous) != Some(hash) {
                push_deduped_index(deduped_indexes, hash, index);
            }
            let replay_route_changed = previous.key.session_id != deduped[index].key.session_id
                || is_sidechain_entry(&previous) != is_sidechain_entry(&deduped[index])
                || previous.key.message_id != deduped[index].key.message_id;
            if replay_route_changed
                && let Some(message_id) = deduped[index].key.message_id.as_deref()
            {
                let session_id = deduped[index].key.session_id.as_ref();
                push_deduped_index(
                    deduped_indexes,
                    sidechain_replay_dedupe_hash(message_id, session_id),
                    index,
                );
                if is_sidechain_entry(&deduped[index]) {
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
        // `index` is new, so none of these buckets can already hold it.
        push_new_deduped_index(deduped_indexes, hash, index);
        if let Some(message_id) = deduped[index].key.message_id.as_deref() {
            let session_id = deduped[index].key.session_id.as_ref();
            push_new_deduped_index(
                deduped_indexes,
                sidechain_replay_dedupe_hash(message_id, session_id),
                index,
            );
            if is_sidechain_entry(&deduped[index]) {
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

fn entry_exact_hash<M>(entry: &ClaudeEntry<M>) -> Option<u64> {
    entry.key.message_id.as_deref().map(|message_id| {
        usage_dedupe_hash(
            message_id,
            entry.key.request_id.as_deref(),
            entry.key.session_id.as_ref(),
            entry.key.timestamp,
        )
    })
}

fn should_replace_deduped_entry<M>(candidate: &ClaudeEntry<M>, existing: &ClaudeEntry<M>) -> bool {
    let candidate_is_sidechain = is_sidechain_entry(candidate);
    let existing_is_sidechain = is_sidechain_entry(existing);
    if candidate_is_sidechain != existing_is_sidechain {
        return existing_is_sidechain;
    }

    let candidate_total = usage_token_total(candidate);
    let existing_total = usage_token_total(existing);
    if candidate_total != existing_total {
        return candidate_total > existing_total;
    }
    carries_more_usage_detail(&candidate.usage, &existing.usage)
}

fn is_sidechain_entry<M>(entry: &ClaudeEntry<M>) -> bool {
    entry.key.is_sidechain == Some(true)
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
            breakdown.add_stored_cost(entry.stored_cost);
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
mod tests {
    use std::{path::Path, sync::Arc};

    use super::{
        ClaudeEntry, UsageFile, load_daily_summaries_inner, push_deduped_entry,
        read_daily_usage_file_with_diagnostics,
    };
    use crate::{
        Diagnostics, PricingMap, TimestampMs, TokenUsageRaw,
        cli::{CostMode, SharedArgs},
    };
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
    use jiff::tz::TimeZone as JiffTimeZone;

    fn read_daily_usage_file(
        path: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: Option<&PricingMap>,
    ) -> UsageFile {
        read_daily_usage_file_with_diagnostics(path, tz, mode, pricing, &Diagnostics::new("claude"))
    }

    #[test]
    fn null_contract_rejections_claude_code_writes_stay_quiet() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": [
                r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                crate::tests::SYNTHETIC_NULL_SPEED_LINE,
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
            debug: true,
            ..SharedArgs::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("claude");

        let daily = load_daily_summaries_inner(&shared, None, false, true, &diagnostics).unwrap();

        assert_eq!(daily.summaries[0].input_tokens, 10);
        assert!(diagnostics.warnings(&shared).is_empty());
    }

    #[test]
    fn collects_session_days_of_counted_main_thread_usage_only() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-empty.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-empty","type":"user","message":{"role":"user","content":"hi"}}"#,
            "projects/project-a/session-sub/subagents/agent-1.jsonl": r#"{"timestamp":"2026-01-02T10:00:00.000Z","sessionId":"session-sub","isSidechain":true,"requestId":"req-sub","message":{"id":"msg-sub","model":"claude-sonnet-4-20250514","usage":{"input_tokens":30,"output_tokens":1}}}"#,
        });
        let _env = EnvVarsGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };

        let daily = load_daily_summaries_inner(
            &shared,
            None,
            false,
            true,
            &ccusage_core::Diagnostics::new("claude"),
        )
        .unwrap();

        assert_eq!(
            daily.session_days.into_iter().collect::<Vec<_>>(),
            [(Arc::from("session-a"), "2026-01-02".to_string())]
        );
        assert_eq!(daily.summaries[0].input_tokens, 130);
    }

    #[test]
    fn detecting_usage_in_files_outside_the_window_records_no_skips() {
        let fixture = fs_fixture!({
            "projects/project-a/old.jsonl": [
                r#"{"timestamp":"2001-01-01T00:00:00.000Z","message":{"usage":{"input_tokens":"bad"}}}"#,
                r#"{"timestamp":"2001-01-01T00:00:00.000Z","sessionId":"old","requestId":"req-old","message":{"id":"msg-old","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ]
            .join("\n"),
        });
        std::fs::File::options()
            .write(true)
            .open(fixture.path("projects/project-a/old.jsonl"))
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200))
            .unwrap();
        let _env = EnvVarsGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            since: Some("20260101".to_string()),
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("claude");

        let daily = load_daily_summaries_inner(&shared, None, false, true, &diagnostics).unwrap();

        assert!(daily.summaries.is_empty());
        assert!(daily.detected);
        assert!(diagnostics.warnings(&shared).is_empty());
    }

    #[test]
    fn keeps_parent_daily_usage_when_sidechain_replays_message_with_new_request_id() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry(DailyEntryFixture {
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
            daily_loaded_entry(DailyEntryFixture {
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
            daily_loaded_entry(DailyEntryFixture {
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
        assert_eq!(deduped[0].key.message_id.as_deref(), Some("msg-parent"));
        assert_eq!(deduped[0].key.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
        assert_eq!(
            deduped[1].key.message_id.as_deref(),
            Some("msg-sidechain-answer")
        );
        assert_eq!(deduped[1].usage.cache_read_input_tokens, 700);
    }

    #[test]
    fn dedupes_timestamp_different_daily_sidechain_replay_with_parent() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 20,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-sidechain-replay",
                    is_sidechain: true,
                    cache_read_tokens: 50_000,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_001_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
    }

    #[test]
    fn refreshes_daily_dedupe_indexes_when_parent_replaces_sidechain_replay() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry(DailyEntryFixture {
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
            daily_loaded_entry(DailyEntryFixture {
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
            daily_loaded_entry(DailyEntryFixture {
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
        assert_eq!(deduped[0].key.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
    }

    #[test]
    fn propagates_sidechain_metadata_from_agent_progress_lines() {
        let (data, _) = super::parse_usage_line(
            br#"{"sessionId":"session-a","data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","requestId":"req-sidechain","isSidechain":true,"message":{"usage":{"input_tokens":0,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":20},"model":"claude-sonnet-4-20250514","id":"msg-sidechain"}}}}"#,
        )
        .unwrap();

        assert_eq!(data.session_id.as_deref(), Some("session-a"));
        assert_eq!(data.is_sidechain, Some(true));
    }

    #[test]
    fn prefers_the_direct_entry_over_an_agent_progress_wrapper() {
        let (data, _) = super::parse_usage_line(
            br#"{"timestamp":"2026-03-29T08:00:00.000Z","requestId":"req-direct","message":{"usage":{"input_tokens":1,"output_tokens":2},"model":"direct-model","id":"msg-direct"},"data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","requestId":"req-wrapped","message":{"usage":{"input_tokens":3,"output_tokens":4},"model":"wrapped-model","id":"msg-wrapped"}}}}"#,
        )
        .unwrap();

        assert_eq!(data.request_id.as_deref(), Some("req-direct"));
        assert_eq!(data.message.model.as_deref(), Some("direct-model"));
    }

    #[test]
    fn keeps_gateway_daily_usage_from_distinct_sessions_with_reused_message_id() {
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
            let loaded = read_daily_usage_file(
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
                .map(|entry| entry.usage.input_tokens)
                .sum::<u64>(),
            400
        );
    }

    #[test]
    fn keeps_timestamp_different_requestless_daily_entries_without_sidechain() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let mut first = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "ocgo",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 10,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        );
        first.key.request_id = None;
        push_deduped_entry(first, &mut deduped_indexes, &mut deduped);

        let mut second = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "ocgo",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 20,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_001_000),
        );
        second.key.request_id = None;
        push_deduped_entry(second, &mut deduped_indexes, &mut deduped);

        assert_eq!(deduped.len(), 2);
        assert_eq!(
            deduped
                .iter()
                .map(|entry| entry.usage.output_tokens)
                .sum::<u64>(),
            30
        );
    }

    #[test]
    fn keeps_the_cache_ttl_split_from_an_otherwise_equal_daily_duplicate() {
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
                timezone: Some("UTC".to_string()),
                single_thread: true,
                ..SharedArgs::default()
            };

            let daily = load_daily_summaries_inner(
                &shared,
                None,
                false,
                true,
                &ccusage_core::Diagnostics::new("claude"),
            )
            .unwrap();

            let breakdown = &daily.summaries[0].model_breakdowns[0];
            assert_eq!(breakdown.cache_creation_tokens, 100);
            assert_eq!(breakdown.cache_creation_1h_tokens, Some(60));
        }
    }

    #[test]
    fn dedupes_anthropic_message_id_from_same_session_at_distinct_timestamps() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let mut first = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg_01AbC2DeF3",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 25,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        );
        first.key.request_id = None;
        push_deduped_entry(first, &mut deduped_indexes, &mut deduped);

        let mut second = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "msg_01AbC2DeF3",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 25,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_500),
        );
        second.key.request_id = None;
        push_deduped_entry(second, &mut deduped_indexes, &mut deduped);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].usage.output_tokens, 25);
    }

    #[test]
    fn dedupes_response_message_id_from_same_session_at_distinct_timestamps() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let mut first = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "resp_0123456789abcdef",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 25,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_000),
        );
        first.key.request_id = None;
        push_deduped_entry(first, &mut deduped_indexes, &mut deduped);

        let mut second = daily_loaded_entry_at(
            DailyEntryFixture {
                message_id: "resp_0123456789abcdef",
                request_id: "unused",
                is_sidechain: false,
                cache_read_tokens: 0,
                output_tokens: 25,
            },
            "session-a",
            TimestampMs::from_millis(1_774_000_000_500),
        );
        second.key.request_id = None;
        push_deduped_entry(second, &mut deduped_indexes, &mut deduped);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].usage.output_tokens, 25);
    }

    #[test]
    fn keeps_daily_sidechain_replay_from_distinct_session() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 20,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-sidechain-replay",
                    is_sidechain: true,
                    cache_read_tokens: 50_000,
                    output_tokens: 10,
                },
                "session-b",
                TimestampMs::from_millis(1_774_000_001_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 2);
        assert_eq!(
            deduped
                .iter()
                .map(|entry| entry.usage.cache_read_input_tokens)
                .sum::<u64>(),
            50_020
        );
    }

    #[test]
    fn dedupes_copied_daily_transcripts_with_the_same_serialized_session_id() {
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
            let loaded = read_daily_usage_file(
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
        assert_eq!(deduped[0].usage.input_tokens, 200);
    }

    #[test]
    fn reads_daily_usage_and_advisor_iterations_under_escaped_keys() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-a","message":{"id":"msg-a","model":"main-model","us\u0061ge":{"input_tokens":100,"output_tokens":1,"iterations":[{"type":"advisor\u005fmessage","model":"advisor-model","input_tokens":10,"output_tokens":2}]}}}"#,
        });

        let loaded = read_daily_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].usage.input_tokens, 100);
        assert_eq!(loaded.entries[1].usage.input_tokens, 10);
    }

    #[test]
    fn daily_advisor_gate_preserves_the_previous_extraction() {
        let iterations = [
            r#"{"type":"advisor_message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
            r#"{"type":"advisor\u005fmessage","model":"advisor","input_tokens":10,"output_tokens":2}"#,
            r#"{"type":"advisor\\u005fmessage","model":"advisor","input_tokens":10,"output_tokens":2}"#,
            r#"{"type":"message","cache_creation":{"ephemeral_5m_input_tokens":7}}"#,
            r#"{"type":"advisor_message","type":"message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
            r#"{"type":"message","type":"advisor_message","model":"advisor","input_tokens":10,"output_tokens":2}"#,
            r#"{"type":null}"#,
            r#"{"type":["advisor_message"]}"#,
            r#"{"kind":"advisor_message"}"#,
            r#"null"#,
            r#"[]"#,
        ];
        let mut state = 0x3c6e_f372_fe94_f82b_u64;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % bound as u64).unwrap_or_default()
        };
        let mut skipped = 0;
        let mut retained = 0;
        for _ in 0..5_000 {
            let entries = (0..next(6))
                .map(|_| iterations[next(iterations.len())])
                .collect::<Vec<_>>()
                .join(",");
            let usage =
                format!(r#"{{"input_tokens":1,"output_tokens":2,"iterations":[{entries}]}}"#);
            let message = format!(r#"{{"model":"parent","usage":{usage}}}"#);
            for line in [
                format!(r#"{{"timestamp":"2026-05-22T02:34:40Z","message":{message}}}"#),
                format!(
                    r#"{{"message":{message},"data":{{"message":{{"timestamp":"2026-05-22T02:34:40Z","message":{message}}}}}}}"#
                ),
            ] {
                let (_, lookup) = super::parse_usage_line(line.as_bytes()).unwrap();
                let previous = super::advisor_usages_from_line(line.as_bytes());
                if matches!(lookup, super::AdvisorLookup::AskLine) {
                    retained += 1;
                } else {
                    skipped += 1;
                    assert!(previous.is_empty(), "{line}");
                }
            }
        }
        assert!(skipped > 0);
        assert!(retained > 0);
    }

    #[test]
    fn reads_top_level_advisor_iterations_beside_an_agent_progress_record() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"sessionId":"session-a","message":{"usage":{"input_tokens":5,"output_tokens":1,"iterations":[{"type":"advisor_message","model":"advisor-model","input_tokens":10,"output_tokens":2}]}},"data":{"message":{"timestamp":"2026-05-22T02:34:40.000Z","requestId":"req-a","message":{"id":"msg-a","model":"main-model","usage":{"input_tokens":100,"output_tokens":1}}}}}"#,
        });

        let loaded = read_daily_usage_file(
            &fixture.path("projects/project-a/session-a/chat.jsonl"),
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            None,
        );

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].usage.input_tokens, 100);
        assert_eq!(loaded.entries[1].usage.input_tokens, 10);
    }

    #[test]
    fn dedupes_copied_daily_transcripts_with_the_same_request_id_across_sessions() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:41.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        for path in [
            fixture.path("projects/project-a/session-a/chat.jsonl"),
            fixture.path("projects/project-a/session-b/chat.jsonl"),
        ] {
            let loaded = read_daily_usage_file(
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
        assert_eq!(deduped[0].usage.input_tokens, 200);
    }

    #[test]
    fn returns_the_discarded_daily_copy_from_another_session() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-a","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}}}"#,
            "projects/project-a/session-b/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-b","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":200,"output_tokens":1}}}"#,
            "projects/project-a/session-c/chat.jsonl": r#"{"timestamp":"2026-05-22T02:34:40.000Z","sessionId":"session-c","requestId":"req-shared","message":{"id":"msg-shared","model":"claude-sonnet-4-20250514","usage":{"input_tokens":50,"output_tokens":1}}}"#,
        });
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        let discarded = ["a", "b", "c"].map(|session| {
            let path = fixture.path(format!("projects/project-a/session-{session}/chat.jsonl"));
            let entry = read_daily_usage_file(
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
                .map(|(index, discarded)| (index, discarded.key.session_id.to_string()))
        });

        assert_eq!(
            discarded,
            [
                None,
                Some((0, "session-a".to_string())),
                Some((0, "session-c".to_string())),
            ]
        );
        assert_eq!(deduped[0].key.session_id.as_ref(), "session-b");
    }

    #[test]
    fn dedupes_daily_sidechain_replay_after_an_equal_copy_from_another_session() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 20,
                    output_tokens: 10,
                },
                "session-b",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 20,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-sidechain-replay",
                    is_sidechain: true,
                    cache_read_tokens: 50_000,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].usage.cache_read_input_tokens, 20);
    }

    #[test]
    fn dedupes_daily_sidechain_replay_after_cross_session_survivor_replacement() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 20,
                    output_tokens: 10,
                },
                "session-b",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-parent",
                    is_sidechain: false,
                    cache_read_tokens: 30,
                    output_tokens: 10,
                },
                "session-a",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            daily_loaded_entry_at(
                DailyEntryFixture {
                    message_id: "msg-parent",
                    request_id: "req-sidechain-replay",
                    is_sidechain: true,
                    cache_read_tokens: 50_000,
                    output_tokens: 10,
                },
                "session-b",
                TimestampMs::from_millis(1_774_000_000_000),
            ),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].key.session_id.as_ref(), "session-a");
        assert_eq!(deduped[0].usage.cache_read_input_tokens, 30);
    }

    struct DailyEntryFixture {
        message_id: &'static str,
        request_id: &'static str,
        is_sidechain: bool,
        cache_read_tokens: u64,
        output_tokens: u64,
    }

    fn daily_loaded_entry(fixture: DailyEntryFixture) -> ClaudeEntry {
        ClaudeEntry {
            key: super::DedupKey {
                timestamp: TimestampMs::from_millis(1_774_000_000_000),
                session_id: Arc::from("session-a"),
                message_id: Some(fixture.message_id.to_string()),
                request_id: Some(fixture.request_id.to_string()),
                is_sidechain: Some(fixture.is_sidechain),
            },
            metadata: (),
            date: "2026-03-29".to_string(),
            project: Arc::from("project-a"),
            usage: TokenUsageRaw {
                input_tokens: 0,
                output_tokens: fixture.output_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: fixture.cache_read_tokens,
                speed: None,
                cache_creation: None,
            },
            cost: 0.0,
            model: Some("claude-sonnet-4-20250514".to_string()),
            missing_pricing_model: None,
            stored_cost: None,
        }
    }

    fn daily_loaded_entry_at(
        fixture: DailyEntryFixture,
        session_id: &str,
        timestamp: TimestampMs,
    ) -> ClaudeEntry {
        let mut entry = daily_loaded_entry(fixture);
        entry.key.session_id = Arc::from(session_id);
        entry.key.timestamp = timestamp;
        entry
    }
}
