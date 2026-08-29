use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, Pricing, PricingMap, Result, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, calculate_cost_from_pricing,
    cli::CostMode, fast::LinePrefilter, format_date_tz, missing_pricing_model_for_usage,
};
use ccusage_adapter_common::jsonl;

/// A single parsed pi session record. Only the fields ccusage consumes are
/// declared; serde skips everything else.
#[derive(Debug, Deserialize)]
struct PiLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(
        rename = "parentSession",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    parent_session: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    message: Option<PiMessage>,
}

/// The link fields carried by every tree-format pi session entry.
#[derive(Debug, Deserialize)]
struct PiEntryLink {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(
        rename = "parentId",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    parent_id: Option<String>,
}

/// The pi `message` block carried by assistant records.
#[derive(Debug, Deserialize)]
struct PiMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    role: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    usage: Option<PiUsage>,
}

/// Token counts and optional display cost carried by a pi assistant message.
#[derive(Debug, Default, Deserialize)]
struct PiUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(rename = "cacheRead", default, deserialize_with = "jsonl::lenient_u64")]
    cache_read: u64,
    #[serde(
        rename = "cacheWrite",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_write: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
    // A non-object `cost` previously left display cost absent without dropping
    // the record, so deserialize it leniently instead of failing the line.
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cost: Option<PiCost>,
}

/// Optional display cost block carried by a pi assistant message.
#[derive(Debug, Default, Deserialize)]
struct PiCost {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    total: Option<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct PiSessionHeader {
    pub(crate) parent_session: Option<PathBuf>,
    pub(crate) parent_session_is_malformed: bool,
    pub(crate) timestamp: Option<crate::TimestampMs>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PiUsageSignature {
    timestamp: crate::TimestampMs,
    model: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    effective_total_tokens: u64,
    billed_cost_bits: Option<u64>,
}

struct PiUsageRecord {
    signature: PiUsageSignature,
    entry_id: Option<String>,
}

enum PiReplayUsagePath {
    Linear,
    Active(Vec<usize>),
    Invalid,
}

pub(crate) struct PiSessionData {
    pub(crate) header: Option<PiSessionHeader>,
    usage: Vec<PiUsageRecord>,
    replay_usage_path: PiReplayUsagePath,
    pub(crate) entries: Vec<LoadedEntry>,
}

impl PiSessionData {
    pub(crate) fn matching_replay_prefix(
        &self,
        child: &Self,
        fork_timestamp: crate::TimestampMs,
    ) -> Option<usize> {
        if matches!(&child.replay_usage_path, PiReplayUsagePath::Invalid) {
            return None;
        }

        match &self.replay_usage_path {
            PiReplayUsagePath::Linear => Some(matching_replay_prefix(
                child.usage.iter().map(|usage| &usage.signature),
                self.usage.iter().map(|usage| &usage.signature),
                fork_timestamp,
            )),
            PiReplayUsagePath::Active(indices) => Some(matching_replay_prefix(
                child.usage.iter().map(|usage| &usage.signature),
                indices.iter().map(|&index| &self.usage[index].signature),
                fork_timestamp,
            )),
            PiReplayUsagePath::Invalid => None,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn read_session_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    Ok(read_session_file_data(path, tz, mode, pricing)?.entries)
}

#[cfg_attr(not(test), allow(dead_code))]
fn read_session_file_for_store(
    path: &Path,
    store_root: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    store_name: &str,
) -> Result<Vec<LoadedEntry>> {
    Ok(read_session_file_data_for_store(path, store_root, tz, mode, pricing, store_name)?.entries)
}

pub(crate) fn read_session_file_data(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<PiSessionData> {
    read_session_file_data_with_context(path, tz, mode, pricing, PiStoreContext::Default)
}

pub(crate) fn read_session_file_data_for_store(
    path: &Path,
    store_root: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    store_name: &str,
) -> Result<PiSessionData> {
    read_session_file_data_with_context(
        path,
        tz,
        mode,
        pricing,
        PiStoreContext::Named {
            root: store_root,
            name: store_name,
        },
    )
}

#[derive(Clone, Copy)]
enum PiStoreContext<'a> {
    Default,
    Named { root: &'a Path, name: &'a str },
}

impl<'a> PiStoreContext<'a> {
    fn store_name(self) -> &'a str {
        match self {
            Self::Default => "pi",
            Self::Named { name, .. } => name,
        }
    }

    fn project(self, path: &Path) -> String {
        match self {
            Self::Default => extract_project(path),
            Self::Named { root, .. } => extract_project_for_store(path, root),
        }
    }

    fn cost(
        self,
        raw_model: Option<&str>,
        display_model: Option<&str>,
        usage: TokenUsageRaw,
        display_cost: Option<f64>,
        mode: CostMode,
        pricing: Option<&PricingMap>,
    ) -> f64 {
        match self {
            Self::Default => {
                calculate_cost_for_usage(display_model, usage, display_cost, mode, pricing)
            }
            Self::Named { .. } => {
                calculate_store_cost(raw_model, display_model, usage, display_cost, mode, pricing)
            }
        }
    }

    fn missing_pricing_model(
        self,
        raw_model: Option<&str>,
        display_model: Option<&str>,
        usage: TokenUsageRaw,
        display_cost: Option<f64>,
        mode: CostMode,
        pricing: Option<&PricingMap>,
    ) -> Option<String> {
        match self {
            Self::Default => {
                missing_pricing_model_for_usage(display_model, usage, display_cost, mode, pricing)
            }
            Self::Named { .. } => missing_store_pricing_model(
                raw_model,
                display_model,
                usage,
                display_cost,
                mode,
                pricing,
            ),
        }
    }
}

fn read_session_file_data_with_context(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    context: PiStoreContext<'_>,
) -> Result<PiSessionData> {
    let content = fs::read(path)?;
    let project = context.project(path);
    let session_id = extract_session_id(path);
    let header = parse_session_header(&content);
    // Usable pi lines carry token counts under a `usage` key nested in a
    // `message` object, so require both substrings before JSON parsing.
    let prefilter = LinePrefilter::all(&[br#""usage""#, br#""message""#]);
    let mut entries = Vec::new();
    let mut usage_records = Vec::new();

    for record in jsonl::records::<PiLine>(&content, Some(&prefilter)) {
        if !is_pi_message_usage(&record) {
            continue;
        }
        let Some(timestamp_text) = record.timestamp.clone() else {
            continue;
        };
        let Some(timestamp) = crate::parse_ts_timestamp(&timestamp_text) else {
            continue;
        };
        let Some(message) = record.message.as_ref() else {
            continue;
        };
        let Some(usage_value) = message.usage.as_ref() else {
            continue;
        };
        let input = usage_value.input;
        let output = usage_value.output;
        let cache_read = usage_value.cache_read;
        let cache_create = usage_value.cache_write;
        let total = usage_value.total_tokens;
        let usage = TokenUsageRaw {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: cache_create,
            cache_read_input_tokens: cache_read,
            speed: None,
            cache_creation: None,
        };
        let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, total);
        if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
            continue;
        }
        let raw_model = message.model.clone();
        let display_cost = usage_value.cost.as_ref().and_then(|cost| cost.total);
        let model = raw_model
            .as_ref()
            .map(|model| format!("[{}] {model}", context.store_name()));
        let cost = context.cost(
            raw_model.as_deref(),
            model.as_deref(),
            usage,
            display_cost,
            mode,
            pricing,
        );
        usage_records.push(PiUsageRecord {
            signature: PiUsageSignature {
                timestamp,
                model: raw_model.clone(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
                effective_total_tokens: crate::total_usage_tokens(usage)
                    .saturating_add(extra_total_tokens),
                billed_cost_bits: replay_billed_cost_bits(mode, cost),
            },
            entry_id: record.id.clone(),
        });
        let missing_pricing_model = context.missing_pricing_model(
            raw_model.as_deref(),
            model.as_deref(),
            usage,
            display_cost,
            mode,
            pricing,
        );
        let data = UsageEntry {
            session_id: Some(session_id.clone()),
            timestamp: timestamp_text,
            version: None,
            message: UsageMessage {
                usage,
                model: model.clone(),
                id: None,
            },
            cost_usd: display_cost,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from(project.as_str()),
            session_id: Arc::from(session_id.as_str()),
            project_path: Arc::from(project.as_str()),
            cost,
            extra_total_tokens,
            credits: None,
            message_count: None,
            model,
            data,
            usage_limit_reset_time: None,
            missing_pricing_model,
        });
    }
    Ok(PiSessionData {
        header,
        replay_usage_path: replay_usage_path(&content, &usage_records),
        usage: usage_records,
        entries,
    })
}

fn matching_replay_prefix<'a>(
    child_usage: impl Iterator<Item = &'a PiUsageSignature>,
    parent_usage: impl Iterator<Item = &'a PiUsageSignature>,
    fork_timestamp: crate::TimestampMs,
) -> usize {
    child_usage
        .zip(parent_usage.take_while(|usage| usage.timestamp <= fork_timestamp))
        .take_while(|(child, parent)| child == parent)
        .count()
}

fn replay_usage_path(content: &[u8], usage: &[PiUsageRecord]) -> PiReplayUsagePath {
    let links = jsonl::records::<PiEntryLink>(content, None)
        .filter(|link| link.r#type.as_deref() != Some("session"))
        .collect::<Vec<_>>();
    if links
        .iter()
        .all(|link| link.id.is_none() && link.parent_id.is_none())
    {
        // Version 1 sessions predate entry links, so their physical order is
        // the only available linear history.
        return PiReplayUsagePath::Linear;
    }

    let mut parents_by_id = HashMap::new();
    let mut leaf_id = None;
    for link in links {
        // A partially linked session cannot identify an active branch safely.
        let Some(id) = link.id else {
            return PiReplayUsagePath::Invalid;
        };
        if parents_by_id.insert(id.clone(), link.parent_id).is_some() {
            return PiReplayUsagePath::Invalid;
        }
        leaf_id = Some(id);
    }
    let Some(mut entry_id) = leaf_id else {
        return PiReplayUsagePath::Linear;
    };
    if parents_by_id
        .values()
        .filter(|parent| parent.is_none())
        .count()
        != 1
        || parents_by_id
            .values()
            .flatten()
            .any(|parent| !parents_by_id.contains_key(parent))
    {
        return PiReplayUsagePath::Invalid;
    }

    let usage_by_id = usage
        .iter()
        .enumerate()
        .filter_map(|(index, usage)| usage.entry_id.as_deref().map(|id| (id, index)))
        .collect::<HashMap<_, _>>();
    let mut active_usage = Vec::new();
    let mut visited = HashSet::new();
    loop {
        // Pi writes the current leaf last; walking its parents excludes sibling
        // branches that remain earlier in the same physical JSONL file.
        if !visited.insert(entry_id.clone()) {
            return PiReplayUsagePath::Invalid;
        }
        if let Some(&usage_index) = usage_by_id.get(entry_id.as_str()) {
            active_usage.push(usage_index);
        }
        let Some(parent_id) = parents_by_id.get(&entry_id) else {
            return PiReplayUsagePath::Invalid;
        };
        let Some(parent_id) = parent_id else {
            break;
        };
        entry_id = parent_id.clone();
    }
    active_usage.reverse();
    PiReplayUsagePath::Active(active_usage)
}

fn replay_billed_cost_bits(mode: CostMode, cost: f64) -> Option<u64> {
    if mode == CostMode::Calculate {
        return None;
    }
    Some(if cost == 0.0 {
        0.0_f64.to_bits()
    } else {
        cost.to_bits()
    })
}

fn parse_session_header(content: &[u8]) -> Option<PiSessionHeader> {
    let first_line = content.split(|byte| *byte == b'\n').next()?;
    let value = serde_json::from_slice::<serde_json::Value>(first_line).ok()?;
    let header = serde_json::from_value::<PiLine>(value.clone()).ok()?;
    (header.r#type.as_deref() == Some("session")).then(|| PiSessionHeader {
        parent_session: header.parent_session.map(PathBuf::from),
        parent_session_is_malformed: value.get("parentSession").is_some_and(|parent| {
            !parent
                .as_str()
                .is_some_and(|parent| !parent.trim().is_empty())
        }),
        timestamp: header
            .timestamp
            .as_deref()
            .and_then(crate::parse_ts_timestamp),
    })
}

fn is_pi_message_usage(record: &PiLine) -> bool {
    if record
        .r#type
        .as_deref()
        .is_some_and(|message_type| message_type != "message")
    {
        return false;
    }
    let Some(message) = record.message.as_ref() else {
        return false;
    };
    message.role.as_deref() == Some("assistant") && message.usage.is_some()
}

fn extract_session_id(path: &Path) -> String {
    let filename = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    filename
        .split_once('_')
        .map_or(filename, |(_, session)| session)
        .to_string()
}

fn extract_project(path: &Path) -> String {
    let mut previous_was_sessions = false;
    for component in path.components() {
        let segment = component.as_os_str().to_string_lossy();
        if previous_was_sessions {
            return segment.into_owned();
        }
        previous_was_sessions = segment == "sessions";
    }
    "unknown".to_string()
}

fn extract_project_for_store(path: &Path, store_root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(store_root)
        && let Some(project) = relative.components().next()
    {
        return project.as_os_str().to_string_lossy().into_owned();
    }
    extract_project(path)
}

pub(crate) fn entry_id(entry: &LoadedEntry) -> String {
    entry_id_for_store("pi", entry)
}

fn calculate_store_cost(
    raw_model: Option<&str>,
    display_model: Option<&str>,
    usage: TokenUsageRaw,
    display_cost: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    match mode {
        CostMode::Display => display_cost.unwrap_or(0.0),
        CostMode::Auto => display_cost.unwrap_or_else(|| {
            calculate_store_cost_from_tokens(raw_model, display_model, usage, pricing)
        }),
        CostMode::Calculate => {
            calculate_store_cost_from_tokens(raw_model, display_model, usage, pricing)
        }
    }
}

fn calculate_store_cost_from_tokens(
    raw_model: Option<&str>,
    display_model: Option<&str>,
    usage: TokenUsageRaw,
    pricing: Option<&PricingMap>,
) -> f64 {
    let Some(pricing) = store_pricing(raw_model, display_model, pricing) else {
        return 0.0;
    };
    calculate_cost_from_pricing(usage, pricing)
}

fn missing_store_pricing_model(
    raw_model: Option<&str>,
    display_model: Option<&str>,
    usage: TokenUsageRaw,
    display_cost: Option<f64>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if mode == CostMode::Display || (mode == CostMode::Auto && display_cost.is_some()) {
        return None;
    }
    if crate::total_usage_tokens(usage) == 0 {
        return None;
    }
    let raw_model = raw_model?;
    store_pricing(Some(raw_model), display_model, pricing)
        .is_none()
        .then(|| crate::model_aliases::resolve_model_name(raw_model).into_owned())
}

fn store_pricing(
    raw_model: Option<&str>,
    display_model: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Option<Pricing> {
    let pricing = pricing?;
    display_model
        .and_then(|model| pricing.find_exact(model))
        .or_else(|| raw_model.and_then(|model| pricing.find(model)))
}

pub(crate) fn entry_id_for_store(store_name: &str, entry: &LoadedEntry) -> String {
    [
        store_name,
        entry.project.as_ref(),
        entry.session_id.as_ref(),
        entry.data.timestamp.as_str(),
        entry.model.as_deref().unwrap_or_default(),
        &entry.data.message.usage.input_tokens.to_string(),
        &entry.data.message.usage.output_tokens.to_string(),
        &entry
            .data
            .message
            .usage
            .cache_creation_input_tokens
            .to_string(),
        &entry.data.message.usage.cache_read_input_tokens.to_string(),
        &entry.extra_total_tokens.to_string(),
        &entry.cost.to_string(),
    ]
    .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn falls_back_to_total_tokens_when_pi_parts_are_missing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"totalTokens":333}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file(&file, None, CostMode::Display, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 333);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn sets_missing_pricing_model_when_model_not_in_pricing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        // Use Calculate mode with an empty PricingMap so model won't be found
        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Calculate, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("[pi] unknown-model-xyz")
        );
    }

    #[test]
    fn named_store_name_does_not_price_unknown_models() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"totally-unknown-model","usage":{"input":1000000,"output":1000000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "o3": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
        );

        let entries = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            None,
            CostMode::Calculate,
            Some(&pricing),
            "o3",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.0);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("totally-unknown-model")
        );
    }

    #[test]
    fn named_store_name_does_not_outmatch_real_model_pricing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"o3","usage":{"input":1000,"output":2000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "o3": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                },
                "deepseek-chat": {
                    "input_cost_per_token": 0.001,
                    "output_cost_per_token": 0.001
                }
            }"#,
        );

        let entries = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            None,
            CostMode::Calculate,
            Some(&pricing),
            "deepseek-chat",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.018000000000000002);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn named_store_prefixed_pricing_override_wins_before_unprefixed_lookup() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5.4","usage":{"input":1000,"output":2000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.4": {
                    "input_cost_per_token": 0.001,
                    "output_cost_per_token": 0.001
                },
                "[omp] gpt-5.4": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
        );

        let entries = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            None,
            CostMode::Calculate,
            Some(&pricing),
            "omp",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.018000000000000002);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn no_missing_pricing_model_in_display_mode() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Display, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn no_missing_pricing_model_when_auto_mode_has_display_cost() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200,"cost":{"total":0.05}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Auto, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        // In Auto mode with a display cost present, no missing pricing warning
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn keeps_record_when_cost_is_not_an_object() {
        // A non-object `cost` must not fail the whole line; the usage tokens
        // should still be counted with display cost treated as missing.
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200,"cost":0}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file(&file, None, CostMode::Display, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 200);
    }

    #[test]
    fn prefixes_named_store_models_with_store_name() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            None,
            CostMode::Display,
            None,
            "omp",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[omp] gpt-5"));
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("[omp] gpt-5")
        );
    }

    #[test]
    fn includes_named_store_in_dedupe_identity() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pi = read_session_file(&file, None, CostMode::Display, None)
            .unwrap()
            .pop()
            .unwrap();
        let omp = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            None,
            CostMode::Display,
            None,
            "omp",
        )
        .unwrap()
        .pop()
        .unwrap();

        assert_ne!(entry_id(&pi), entry_id_for_store("omp", &omp));
    }
}
