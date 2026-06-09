mod paths;

use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use memchr::memmem;
use rustc_hash::FxHasher;

use crate::{
    LoadedEntry, LoadedFile, PricingMap, Result, Speed, TimestampMs, UsageEntry, UsageMessage,
    UsageSummary, calculate_cost,
    cli::{CostMode, SharedArgs},
    debug_log,
    fast::{FxHashMap, SmallIndexVec, byte_lines, suffix_string},
    format_date_tz, log_level, missing_pricing_model_for_usage, parse_ts_timestamp, parse_tz,
    progress, summarize_by_key,
};

#[cfg(test)]
pub(crate) use paths::timestamp_from_line;
pub(crate) use paths::{
    claude_paths, collect_files_with_extension, collect_usage_files, extract_project,
    extract_session_parts, usage_files,
};

pub(crate) fn load_entries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
) -> Result<Vec<LoadedEntry>> {
    progress::track_usage_load(progress::UsageLoadAgent::Claude, shared.json, || {
        load_entries_inner(shared, project_filter)
    })
}

pub(crate) fn load_daily_summaries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    group_by_project: bool,
) -> Result<Vec<UsageSummary>> {
    // Daily/monthly/weekly summaries share the cached, deduped entry path with
    // session and blocks so every Claude report mode warms (and is served by)
    // the on-disk cache and ledger. `summarize_by_key` reproduces the former
    // bespoke daily accumulator exactly (insertion-ordered models, cost-desc
    // breakdowns); the parity tests in `main.rs` pin this equivalence.
    let entries = load_entries(shared, project_filter)?;
    if group_by_project {
        summarize_by_key(
            &entries,
            |entry| format!("{}\0{}", entry.date, entry.project),
            |key| {
                let mut parts = key.split('\0');
                (
                    parts.next().unwrap_or_default().to_string(),
                    parts.next().map(str::to_string),
                )
            },
        )
    } else {
        summarize_by_key(
            &entries,
            |entry| entry.date.clone(),
            |key| (key.to_string(), None),
        )
    }
}

fn load_entries_inner(
    shared: &SharedArgs,
    project_filter: Option<&str>,
) -> Result<Vec<LoadedEntry>> {
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
    let files = usage_files(&paths, project_filter);
    debug_log(shared, format!("Found {} JSONL usage files", files.len()));
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let pricing = if shared.mode == CostMode::Display {
        None
    } else {
        Some(PricingMap::load_with_overrides(
            shared.offline,
            log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    let tz = parse_tz(shared.timezone.as_deref());
    let mode = shared.mode;
    let tz_ref = tz.as_ref();
    let pricing_ref = pricing.as_ref();

    // Archived entries (from deleted source files) are filtered by project below
    // alongside live entries, so no archive_filter is needed here.
    let loaded_entries = crate::cache::load_with_cache(
        "claude",
        &files,
        shared.single_thread,
        crate::cache::cost_fingerprint(pricing.as_ref().map(|p| p.fingerprint()), mode),
        shared.live_only,
        |file| read_usage_file(file, tz_ref, mode, pricing_ref).map(|f| f.entries),
    )?;
    debug_log(
        shared,
        format!("Loaded {} usage entries", loaded_entries.len()),
    );

    let mut deduped_indexes: FxHashMap<u64, SmallIndexVec> = FxHashMap::default();
    let mut deduped: Vec<LoadedEntry> = Vec::with_capacity(loaded_entries.len());
    for entry in loaded_entries {
        if let Some(filter) = project_filter {
            if entry.project.as_ref() != filter {
                continue;
            }
        }
        push_deduped_entry(entry, &mut deduped_indexes, &mut deduped);
    }
    debug_log(
        shared,
        format!("Kept {} usage entries after deduplication", deduped.len()),
    );
    Ok(deduped)
}

pub(crate) fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    entries.retain(|entry| {
        let date = entry.date.replace('-', "");
        shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
    });
}

pub(crate) fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
    let mut weighted_indexes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let size = fs::metadata(file).map_or(0, |metadata| metadata.len());
        weighted_indexes.push((index, size));
    }
    weighted_indexes.sort_unstable_by(|a, b| match b.1.cmp(&a.1) {
        std::cmp::Ordering::Equal => a.0.cmp(&b.0),
        order => order,
    });

    let mut chunks = vec![Vec::new(); chunk_count];
    let mut chunk_sizes = vec![0_u64; chunk_count];
    for (index, size) in weighted_indexes {
        let mut target = 0;
        for candidate in 1..chunk_sizes.len() {
            if chunk_sizes[candidate] < chunk_sizes[target] {
                target = candidate;
            }
        }
        chunks[target].push(index);
        chunk_sizes[target] = chunk_sizes[target].saturating_add(size);
    }

    chunks
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

fn usage_token_total(data: &UsageEntry) -> u64 {
    let usage = data.message.usage;
    usage.input_tokens
        + usage.output_tokens
        + usage.cache_creation_token_count()
        + usage.cache_read_input_tokens
}

fn should_replace_deduped_entry(candidate: &LoadedEntry, existing: &LoadedEntry) -> bool {
    let candidate_is_sidechain = is_sidechain_usage_entry(&candidate.data);
    let existing_is_sidechain = is_sidechain_usage_entry(&existing.data);
    if candidate_is_sidechain != existing_is_sidechain {
        return existing_is_sidechain;
    }

    let candidate_total = usage_token_total(&candidate.data);
    let existing_total = usage_token_total(&existing.data);
    if candidate_total != existing_total {
        return candidate_total > existing_total;
    }

    // A duplicated agent-progress line and its direct subagent line carry equal
    // tokens but the direct line has the computed cost; prefer the higher cost
    // so the cached path matches the former daily loader's tiebreak.
    if candidate.cost != existing.cost {
        return candidate.cost > existing.cost;
    }

    candidate.data.message.usage.speed.is_some() && existing.data.message.usage.speed.is_none()
}

fn push_deduped_entry(
    entry: LoadedEntry,
    deduped_indexes: &mut FxHashMap<u64, SmallIndexVec>,
    deduped: &mut Vec<LoadedEntry>,
) {
    let dedupe_lookup = entry.data.message.id.as_deref().map(|message_id| {
        let request_id = entry.data.request_id.as_deref();
        let exact_hash = usage_dedupe_hash(message_id, request_id);
        let existing_index = deduped_indexes
            .get(&exact_hash)
            .and_then(|indexes| {
                indexes.iter().copied().find(|&index| {
                    loaded_entry_matches_dedupe_key(&deduped[index], message_id, request_id)
                })
            })
            .or_else(|| {
                // /btw sidechain logs can replay parent messages with new request IDs.
                let message_hash = usage_dedupe_hash(message_id, None);
                let candidate_is_sidechain = is_sidechain_usage_entry(&entry.data);
                deduped_indexes.get(&message_hash).and_then(|indexes| {
                    indexes.iter().copied().find(|&index| {
                        loaded_entry_matches_sidechain_dedupe_key(
                            &deduped[index],
                            message_id,
                            candidate_is_sidechain,
                        )
                    })
                })
            });
        (exact_hash, existing_index)
    });

    if let Some((hash, Some(index))) = dedupe_lookup {
        if should_replace_deduped_entry(&entry, &deduped[index]) {
            deduped[index] = entry;
            push_deduped_index(deduped_indexes, hash, index);
            if let Some(message_id) = deduped[index].data.message.id.as_deref() {
                push_deduped_index(deduped_indexes, usage_dedupe_hash(message_id, None), index);
            }
        }
        return;
    }

    let index = deduped.len();
    deduped.push(entry);
    if let Some((hash, None)) = dedupe_lookup {
        push_deduped_index(deduped_indexes, hash, index);
        if let Some(message_id) = deduped[index].data.message.id.as_deref() {
            push_deduped_index(deduped_indexes, usage_dedupe_hash(message_id, None), index);
        }
    }
}

fn usage_dedupe_hash(message_id: &str, request_id: Option<&str>) -> u64 {
    let mut hasher = FxHasher::default();
    message_id.hash(&mut hasher);
    request_id.hash(&mut hasher);
    hasher.finish()
}

fn loaded_entry_matches_dedupe_key(
    entry: &LoadedEntry,
    message_id: &str,
    request_id: Option<&str>,
) -> bool {
    entry.data.message.id.as_deref() == Some(message_id)
        && entry.data.request_id.as_deref() == request_id
}

fn loaded_entry_matches_sidechain_dedupe_key(
    entry: &LoadedEntry,
    message_id: &str,
    candidate_is_sidechain: bool,
) -> bool {
    entry.data.message.id.as_deref() == Some(message_id)
        && (candidate_is_sidechain || is_sidechain_usage_entry(&entry.data))
}

fn is_sidechain_usage_entry(entry: &UsageEntry) -> bool {
    entry.is_sidechain == Some(true)
}

fn push_deduped_index(
    deduped_indexes: &mut FxHashMap<u64, SmallIndexVec>,
    hash: u64,
    index: usize,
) {
    let indexes = deduped_indexes.entry(hash).or_default();
    if !indexes.contains(&index) {
        indexes.push(index);
    }
}

/// A nested agent-progress log line: `{"type":"progress","data":{"message":{..}}}`.
/// The billable usage lives under `data.message.message`, with the request id,
/// cost, and sidechain flag on the wrapping `data.message`.
#[derive(serde::Deserialize)]
struct AgentProgressEntry {
    data: AgentProgressData,
}

#[derive(serde::Deserialize)]
struct AgentProgressData {
    message: AgentProgressMessage,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProgressMessage {
    timestamp: String,
    message: UsageMessage,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
}

impl AgentProgressEntry {
    /// Flatten the progress wrapper into the same `UsageEntry` shape a direct
    /// line produces. `version`/`session_id` are absent on progress lines (left
    /// `None`, which `is_valid_usage_entry` treats as valid), matching the
    /// former daily loader's `DailyUsageLine::AgentProgress` mapping.
    fn into_usage_entry(self) -> UsageEntry {
        let message = self.data.message;
        UsageEntry {
            session_id: None,
            timestamp: message.timestamp,
            version: None,
            message: message.message,
            cost_usd: message.cost_usd,
            request_id: message.request_id,
            is_api_error_message: None,
            is_sidechain: message.is_sidechain,
        }
    }
}

fn read_usage_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> crate::Result<LoadedFile> {
    let project: Arc<str> = Arc::from(extract_project(path));
    let (session_id, project_path) = extract_session_parts(path);
    let session_id: Arc<str> = Arc::from(session_id);
    let project_path: Arc<str> = Arc::from(project_path);
    let mut loaded_file = LoadedFile {
        timestamp: None,
        entries: Vec::new(),
    };
    let Ok(content) = fs::read(path) else {
        return Ok(loaded_file);
    };

    let usage_marker = memmem::Finder::new(br#""usage":{"#);
    for line in byte_lines(&content) {
        if usage_marker.find(line).is_none() {
            continue;
        }
        if has_unsupported_null_field(line) {
            continue;
        }
        // Direct usage lines deserialize straight into `UsageEntry`; nested
        // agent-progress lines (`"type":"progress"`) carry their usage under
        // `data.message.message`, so fall back to the progress shape. Trying the
        // cheap direct parse first avoids untagged-enum buffering on the hot path
        // while preserving the agent-progress entries the daily loader captured.
        let data = match serde_json::from_slice::<UsageEntry>(line) {
            Ok(entry) => entry,
            Err(_) => match serde_json::from_slice::<AgentProgressEntry>(line) {
                Ok(progress) => progress.into_usage_entry(),
                Err(_) => continue,
            },
        };
        let Some(timestamp) = parse_ts_timestamp(&data.timestamp) else {
            continue;
        };
        update_loaded_file_timestamp(&mut loaded_file, timestamp);
        if !is_valid_usage_entry(&data) {
            continue;
        }
        let date = format_date_tz(timestamp, tz);
        let cost = calculate_cost(&data, mode, pricing);
        let missing_pricing_model = missing_pricing_model_for_usage(
            data.message.model.as_deref(),
            data.message.usage,
            data.cost_usd,
            mode,
            pricing,
        );
        let usage_limit_reset_time =
            usage_limit_reset_time_from_line_bytes(line, data.is_api_error_message);
        let model = data.message.model.as_ref().and_then(|model| {
            if model == "<synthetic>" {
                None
            } else if matches!(data.message.usage.speed, Some(Speed::Fast)) {
                Some(suffix_string(model, "-fast"))
            } else {
                Some(model.clone())
            }
        });
        loaded_file.entries.push(LoadedEntry {
            data,
            timestamp,
            date,
            project: Arc::clone(&project),
            session_id: Arc::clone(&session_id),
            project_path: Arc::clone(&project_path),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model,
            usage_limit_reset_time,
            missing_pricing_model,
        });
    }
    Ok(loaded_file)
}

fn update_loaded_file_timestamp(loaded_file: &mut LoadedFile, timestamp: TimestampMs) {
    loaded_file.timestamp = Some(
        loaded_file
            .timestamp
            .map_or(timestamp, |current| current.min(timestamp)),
    );
}

pub(super) fn is_valid_usage_entry(data: &UsageEntry) -> bool {
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

pub(super) fn has_unsupported_null_field(line: &[u8]) -> bool {
    let mut offset = 0;
    while let Some(relative_index) = memmem::find(&line[offset..], b":null") {
        let null_index = offset + relative_index;
        let mut field_end = null_index.saturating_sub(1);
        if line.get(field_end) != Some(&b'"') {
            while field_end > 0 && line[field_end] != b'"' {
                field_end -= 1;
            }
        }
        if line.get(field_end) == Some(&b'"') {
            let mut field_start = field_end.saturating_sub(1);
            while field_start > 0 && line[field_start] != b'"' {
                field_start -= 1;
            }
            if line.get(field_start) == Some(&b'"')
                && is_unsupported_nullable_field(&line[field_start + 1..field_end])
            {
                return true;
            }
        }
        offset = null_index + b":null".len();
    }
    false
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

pub(super) fn is_semver_prefix(value: &str) -> bool {
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

#[cfg(test)]
pub(crate) fn usage_limit_reset_time_from_line(
    line: &str,
    is_api_error_message: Option<bool>,
) -> Option<TimestampMs> {
    usage_limit_reset_time_from_line_bytes(line.as_bytes(), is_api_error_message)
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
    use std::{path::Path, sync::Arc};

    use super::{
        extract_session_parts, has_unsupported_null_field, paths::is_project_path_segment,
        push_deduped_entry, usage_files,
    };
    use crate::cache::CachedEntry;
    use crate::fast::{FxHashMap, SmallIndexVec};
    use crate::{LoadedEntry, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage};
    use ccusage_test_support::fs_fixture;

    #[test]
    fn limits_usage_file_discovery_to_requested_project() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/a.jsonl": "{}",
            "projects/project-b/session-b/b.jsonl": "{}",
        });

        let files = usage_files(&[fixture.root().to_path_buf()], Some("project-a"));

        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("project-a"));
    }

    #[test]
    fn falls_back_to_full_discovery_for_non_segment_project_filter() {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/a.jsonl": "{}",
            "projects/project-b/session-b/b.jsonl": "{}",
        });

        let files = usage_files(&[fixture.root().to_path_buf()], Some("project-a/session-a"));

        assert_eq!(files.len(), 2);
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
            br#"{"sessionId":null,"message":{"usage":{"input_tokens":0}}}"#
        ));
    }

    #[test]
    fn allows_null_content_like_typescript_loader() {
        assert!(!has_unsupported_null_field(
            br#"{"message":{"content":null,"usage":{"input_tokens":0}}}"#
        ));
    }

    #[test]
    fn propagates_sidechain_metadata_from_agent_progress_lines() {
        let data = serde_json::from_str::<super::AgentProgressEntry>(
            r#"{"data":{"message":{"timestamp":"2026-03-29T07:00:00.000Z","requestId":"req-sidechain","isSidechain":true,"message":{"usage":{"input_tokens":0,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":20},"model":"claude-sonnet-4-20250514","id":"msg-sidechain"}}}}"#,
        )
        .unwrap()
        .into_usage_entry();

        assert_eq!(data.is_sidechain, Some(true));
        assert_eq!(data.request_id.as_deref(), Some("req-sidechain"));
        assert_eq!(data.message.id.as_deref(), Some("msg-sidechain"));
        assert_eq!(
            data.message.model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn sidechain_replay_collapsed_after_cache_round_trip() {
        // A sidechain replay reuses the parent message_id with a new request_id.
        // The collapse depends on is_sidechain, so it only survives a cache hit if
        // CachedEntry persists that flag — this guards against warm-run drift.
        let parent = loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-parent",
            is_sidechain: false,
            cache_read_tokens: 20,
            output_tokens: 10,
        });
        let replay = loaded_usage_entry(UsageEntryFixture {
            message_id: "msg-parent",
            request_id: "req-sidechain-replay",
            is_sidechain: true,
            cache_read_tokens: 50_000,
            output_tokens: 10,
        });

        // Round-trip through CachedEntry to simulate a cache hit.
        let parent_cached = LoadedEntry::from(CachedEntry::from(&parent));
        let replay_cached = LoadedEntry::from(CachedEntry::from(&replay));

        let mut deduped_indexes: FxHashMap<u64, SmallIndexVec> = FxHashMap::default();
        let mut deduped: Vec<LoadedEntry> = Vec::new();
        push_deduped_entry(parent_cached, &mut deduped_indexes, &mut deduped);
        push_deduped_entry(replay_cached, &mut deduped_indexes, &mut deduped);

        // Same result as the cold path: the replay collapses into the parent.
        assert_eq!(deduped.len(), 1, "sidechain replay must collapse to parent");
        assert_eq!(deduped[0].data.message.id.as_deref(), Some("msg-parent"));
        assert_eq!(deduped[0].data.request_id.as_deref(), Some("req-parent"));
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 20);
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
        }
    }
}
