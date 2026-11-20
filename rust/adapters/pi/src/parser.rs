use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::AgentModel,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, TokenUsageRaw, apply_total_token_fallback, cli::CostMode, fast::LinePrefilter,
    format_date_tz, stored_cost_for_mode,
};
use ccusage_adapter_common::{jsonl, units::UnitError};
use ccusage_core::{Diagnostics, Skip, SkipKind};

#[derive(Debug, Deserialize)]
struct PiLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    message: Option<PiMessage>,
}

#[derive(Debug, Deserialize)]
struct PiHeaderLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(rename = "parentSession", default)]
    parent_session: ParentSession,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ParentSession {
    #[default]
    Absent,
    Malformed,
    Path(PathBuf),
}

impl<'de> Deserialize<'de> for ParentSession {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(ccusage_core::non_empty_json_string(Some(&value))
            .map_or(Self::Malformed, |path| Self::Path(PathBuf::from(path))))
    }
}

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

#[derive(Debug, Deserialize)]
struct PiMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    role: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    usage: Option<PiUsage>,
}

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
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cost: Option<PiCost>,
}

#[derive(Debug, Default, Deserialize)]
struct PiCost {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    total: Option<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct PiSessionHeader {
    pub(crate) parent_session: ParentSession,
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

pub(crate) struct PiLoadedEntry {
    pub(crate) entry: LoadedEntry,
    source_timestamp: String,
}

impl std::ops::Deref for PiLoadedEntry {
    type Target = LoadedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

pub(crate) struct PiSessionData {
    pub(crate) header: Option<PiSessionHeader>,
    usage: Vec<PiUsageRecord>,
    replay_usage_path: PiReplayUsagePath,
    pub(crate) entries: Vec<PiLoadedEntry>,
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

pub(crate) fn read_session_file_data_for_store_with_diagnostics(
    path: &Path,
    store_root: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    store_name: &str,
    diagnostics: &Diagnostics,
) -> std::result::Result<PiSessionData, UnitError> {
    read_session_file_data_with_context(
        path,
        tz,
        mode,
        pricing,
        PiStoreContext {
            root: store_root,
            name: store_name,
        },
        diagnostics,
    )
}

#[derive(Clone, Copy)]
struct PiStoreContext<'a> {
    root: &'a Path,
    name: &'a str,
}

fn read_session_file_data_with_context(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    context: PiStoreContext<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<PiSessionData, UnitError> {
    let project = extract_project_for_store(path, context.root);
    let session_id = extract_session_id(path);
    let mut header = None;
    let mut links = Vec::new();
    let prefilter = LinePrefilter::all(&[br#""usage""#, br#""message""#]);
    let mut entries = Vec::new();
    let mut usage_records = Vec::new();

    jsonl::read_file(path, diagnostics, |line, diagnostics| {
        if line.index == 0 {
            header = parse_session_header(line.bytes());
        }
        if let Ok(link) = line.decode::<PiEntryLink>()
            && link.r#type.as_deref() != Some("session")
        {
            links.push(link);
        }
        if !line.selected(Some(&prefilter)) {
            return;
        }
        let Some(record) = line.record::<PiLine>(path, diagnostics, Some(is_pi_usage_envelope))
        else {
            return;
        };
        let number = line.number();
        if !is_pi_message_usage(&record) {
            return;
        }
        let Some(message) = record.message.as_ref() else {
            return;
        };
        let Some(usage_value) = message.usage.as_ref() else {
            return;
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
            return;
        }
        let Some((timestamp_text, timestamp)) = record.timestamp.as_ref().and_then(|text| {
            crate::parse_ts_timestamp(text).map(|timestamp| (text.clone(), timestamp))
        }) else {
            diagnostics.record(Skip::record(
                path,
                number,
                SkipKind::InvalidTimestamp,
                "pi assistant usage has no valid timestamp",
            ));
            return;
        };
        let raw_model = message.model.clone();
        let display_cost = usage_value.cost.as_ref().and_then(|cost| cost.total);
        let identity = raw_model
            .as_deref()
            .map(|model| AgentModel::new(context.name, model));
        let override_candidate = identity
            .as_ref()
            .and_then(|model| model.overriding_candidate(pricing.as_map()));
        let candidates: Vec<_> = override_candidate
            .into_iter()
            .chain(identity.iter().flat_map(AgentModel::candidates))
            .collect();
        let source_cost = source_cost_for_mode(display_cost, mode, override_candidate.is_some());
        let priced = price_entry(
            EntryPricing {
                usage,
                reasoning_tokens: 0,
                residual_tokens: 0,
                reported_model: identity.as_ref().map(AgentModel::display),
                candidates: &candidates,
                stored_cost: source_cost,
                timestamp: Some(timestamp),
            },
            pricing,
        );
        let cost = priced.cost;
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
        let missing_pricing_model = (mode != CostMode::Display
            && !(mode == CostMode::Auto && source_cost.is_some())
            && crate::total_usage_tokens(usage) != 0
            && priced.matched.is_none())
        .then(|| {
            raw_model
                .as_deref()
                .map(|model| crate::model_aliases::resolve_model_name(model).into_owned())
        })
        .flatten();
        let stored_cost = stored_cost_for_mode(source_cost, mode);
        let model = identity.map(AgentModel::into_display);
        entries.push(PiLoadedEntry {
            source_timestamp: timestamp_text,
            entry: LoadedEntry {
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
                usage,
                message_id: None,
                version: None,
                recorded_cost: display_cost,
                usage_limit_reset_time: None,
                missing_pricing_model,
                stored_cost,
            },
        });
    })?;
    Ok(PiSessionData {
        header,
        replay_usage_path: replay_usage_path(links, &usage_records),
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

fn replay_usage_path(links: Vec<PiEntryLink>, usage: &[PiUsageRecord]) -> PiReplayUsagePath {
    // NOTE(2026-08-29, #1662): Version 1 Pi sessions predate entry links. Their physical order is the only available replay history.
    if links
        .iter()
        .all(|link| link.id.is_none() && link.parent_id.is_none())
    {
        return PiReplayUsagePath::Linear;
    }

    let mut parents_by_id = HashMap::new();
    let mut leaf_id = None;
    for link in links {
        let Some(id) = link.id else {
            return PiReplayUsagePath::Invalid;
        };
        if parents_by_id.insert(id.clone(), link.parent_id).is_some() {
            return PiReplayUsagePath::Invalid;
        }
        leaf_id = Some(id);
    }
    // NOTE(2026-08-29, #1662): Pi writes the current leaf last and leaves abandoned siblings earlier in the file. Follow the leaf's parents to identify the replayable branch.
    let Some(mut entry_id) = leaf_id else {
        return PiReplayUsagePath::Linear;
    };
    if parents_by_id
        .values()
        .flatten()
        .any(|parent| !parents_by_id.contains_key(parent))
        || has_cyclic_entry_links(&parents_by_id)
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

fn has_cyclic_entry_links(parents_by_id: &HashMap<String, Option<String>>) -> bool {
    let mut validated = HashSet::new();
    for start in parents_by_id.keys() {
        if validated.contains(start) {
            continue;
        }

        let mut path = HashSet::new();
        let mut entry_id = start.clone();
        while !validated.contains(&entry_id) {
            if !path.insert(entry_id.clone()) {
                return true;
            }
            let Some(parent_id) = parents_by_id.get(&entry_id) else {
                return true;
            };
            let Some(parent_id) = parent_id else {
                break;
            };
            entry_id = parent_id.clone();
        }
        validated.extend(path);
    }
    false
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

fn parse_session_header(line: &[u8]) -> Option<PiSessionHeader> {
    let header = jsonl::parse::<PiHeaderLine>(line).ok()?;
    (header.r#type.as_deref() == Some("session")).then(|| PiSessionHeader {
        parent_session: header.parent_session,
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

fn is_pi_usage_envelope(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(|message_type| message_type == "message")
        && value
            .get("message")
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
        && value
            .get("message")
            .is_some_and(|message| message.get("usage").is_some())
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

fn extract_project_for_store(path: &Path, store_root: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.strip_prefix(store_root).ok())
        .and_then(|relative| relative.components().next())
        .map_or_else(
            || "unknown".to_string(),
            |project| project.as_os_str().to_string_lossy().into_owned(),
        )
}

fn source_cost_for_mode(
    display_cost: Option<f64>,
    mode: CostMode,
    has_override: bool,
) -> Option<f64> {
    match mode {
        CostMode::Auto if has_override => None,
        CostMode::Auto => display_cost.filter(|cost| cost.is_finite() && *cost >= 0.0),
        CostMode::Calculate | CostMode::Display => display_cost,
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PiEntryId {
    project: Arc<str>,
    session_id: Arc<str>,
    timestamp: String,
    model: Option<String>,
    tokens: [u64; 5],
    cost_bits: u64,
}

pub(crate) fn entry_id(entry: &PiLoadedEntry) -> PiEntryId {
    let usage = entry.usage;
    PiEntryId {
        project: Arc::clone(&entry.project),
        session_id: Arc::clone(&entry.session_id),
        timestamp: entry.source_timestamp.clone(),
        model: entry.model.clone(),
        tokens: [
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
            entry.extra_total_tokens,
        ],
        cost_bits: entry.cost.to_bits(),
    }
}

#[cfg(test)]
mod tests {
    use ccusage_core::pricing::MatchKind;

    use super::*;
    use ccusage_test_support::fs_fixture;
    use std::path::Path;

    fn read_session_file(
        path: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: PriceTable<'_>,
    ) -> crate::Result<Vec<PiLoadedEntry>> {
        read_session_file_data_for_store_with_diagnostics(
            path,
            path.parent().and_then(Path::parent).unwrap(),
            tz,
            mode,
            pricing,
            "pi",
            &Diagnostics::without_clock("pi"),
        )
        .map(|data| data.entries)
        .map_err(|error| ccusage_core::cli_error(error.to_string()))
    }

    fn read_session_file_for_store(
        path: &Path,
        store_root: &Path,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: PriceTable<'_>,
        store_name: &str,
    ) -> crate::Result<Vec<PiLoadedEntry>> {
        read_session_file_data_for_store_with_diagnostics(
            path,
            store_root,
            tz,
            mode,
            pricing,
            store_name,
            &Diagnostics::without_clock("pi"),
        )
        .map(|data| data.entries)
        .map_err(|error| ccusage_core::cli_error(error.to_string()))
    }

    fn pricing_for_cost_tests() -> ccusage_core::PricingMap {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "[pi] test-model": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                },
                "[omp] test-model": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
        );
        pricing
    }

    fn usage_for_cost_tests() -> crate::TokenUsageRaw {
        crate::TokenUsageRaw {
            input_tokens: 1000,
            output_tokens: 2000,
            ..crate::TokenUsageRaw::default()
        }
    }

    fn pricing_with_override(model: &str) -> ccusage_core::PricingMap {
        let model = model.to_string();
        let override_value = crate::cli::PricingOverride {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..Default::default()
        };
        ccusage_core::PricingMap::load_with_overrides(true, false, [(&model, &override_value)])
    }

    #[test]
    fn default_store_auto_uses_pricing_override_over_recorded_cost() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let pricing = pricing_with_override("[pi] my-custom-model");

        let auto = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap();
        assert_eq!(auto[0].cost, 5.0);
        assert_eq!(auto[0].recorded_cost, Some(0.0));
        assert_eq!(auto[0].missing_pricing_model, None);

        let display = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        )
        .unwrap();
        assert_eq!(display[0].cost, 0.0);
    }

    #[test]
    fn named_store_auto_uses_pricing_override_over_recorded_cost() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let pricing = pricing_with_override("[omp] my-custom-model");

        let auto = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
            "omp",
        )
        .unwrap();
        assert_eq!(auto[0].cost, 5.0);
        assert_eq!(auto[0].recorded_cost, Some(0.0));
        assert_eq!(auto[0].missing_pricing_model, None);

        let display = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            "omp",
        )
        .unwrap();
        assert_eq!(display[0].cost, 0.0);
    }

    #[test]
    fn bare_model_override_applies_to_default_and_named_stores() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let pricing = pricing_with_override("my-custom-model");

        let default = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap();
        assert_eq!(default[0].cost, 5.0);
        assert_eq!(default[0].missing_pricing_model, None);

        let named = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
            "omp",
        )
        .unwrap();
        assert_eq!(named[0].cost, 5.0);
        assert_eq!(named[0].missing_pricing_model, None);
    }

    #[test]
    fn diagnoses_assistant_usage_without_a_valid_timestamp() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": concat!(
                r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
                "\n",
                r#"{"type":"message","timestamp":"SECRET_CANARY","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
                "\n",
                r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":0,"output":0}}}"#,
                "\n",
            ),
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let diagnostics = Diagnostics::without_clock("pi");

        let data = read_session_file_data_for_store_with_diagnostics(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            "pi",
            &diagnostics,
        )
        .unwrap();

        assert_eq!(data.entries.len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
        let warnings = diagnostics
            .warnings(&crate::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(warnings.contains("agent_session-a.jsonl:2"));
        assert!(warnings.contains("agent_session-a.jsonl:3"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn falls_back_to_total_tokens_when_pi_parts_are_missing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"totalTokens":333}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.output_tokens, 333);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn sets_missing_pricing_model_when_model_not_in_pricing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = ccusage_core::PricingMap::default();
        let entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("unknown-model-xyz")
        );
    }

    #[test]
    fn named_store_name_does_not_price_unknown_models() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"totally-unknown-model","usage":{"input":1000000,"output":1000000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = ccusage_core::PricingMap::default();
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
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
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
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"o3","usage":{"input":1000,"output":2000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = ccusage_core::PricingMap::default();
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
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            "deepseek-chat",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.018000000000000002);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn named_store_prefixed_pricing_override_wins_before_unprefixed_lookup() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5.4","usage":{"input":1000,"output":2000}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let mut pricing = ccusage_core::PricingMap::default();
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
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
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

        let entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn no_missing_pricing_model_when_auto_mode_has_display_cost() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200,"cost":{"total":0.05}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = ccusage_core::PricingMap::default();
        let entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn keeps_record_when_cost_is_not_an_object() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200,"cost":0}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 100);
        assert_eq!(entries[0].usage.output_tokens, 200);
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
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            "omp",
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[omp] gpt-5"));
        assert_eq!(entries[0].model.as_deref(), Some("[omp] gpt-5"));
    }

    #[test]
    fn includes_named_store_in_dedupe_identity() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pi = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
        )
        .unwrap()
        .pop()
        .unwrap();
        let omp = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            "omp",
        )
        .unwrap()
        .pop()
        .unwrap();

        assert_ne!(entry_id(&pi), entry_id(&omp));
    }

    #[test]
    fn classifies_the_parent_session_of_a_header() {
        let header = |parent: &str| {
            let line =
                format!(r#"{{"type":"session","timestamp":"2026-01-01T00:00:00.000Z"{parent}}}"#);
            parse_session_header(line.as_bytes())
                .unwrap()
                .parent_session
        };

        assert_eq!(header(""), ParentSession::Absent);
        assert_eq!(header(r#","parentSession":null"#), ParentSession::Malformed);
        assert_eq!(header(r#","parentSession":5"#), ParentSession::Malformed);
        assert_eq!(header(r#","parentSession":"  ""#), ParentSession::Malformed);
        assert_eq!(
            header(r#","parentSession":" /a/b.jsonl ""#),
            ParentSession::Path(PathBuf::from("/a/b.jsonl"))
        );
    }

    #[test]
    fn auto_recalculates_negative_but_keeps_zero_source_cost_for_default_and_named_stores() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": [
                r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"test-model","usage":{"input":1000,"output":2000,"cost":{"total":-1.25}}}}"#,
                r#"{"type":"message","timestamp":"2026-01-02T00:01:00.000Z","message":{"role":"assistant","model":"test-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
            ]
            .join("\n"),
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let pricing = pricing_for_cost_tests();
        let expected_cost = 0.018000000000000002;

        let default_entries = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap();
        assert_eq!(default_entries.len(), 2);
        assert_eq!(default_entries[0].cost, expected_cost);
        assert_eq!(default_entries[0].recorded_cost, Some(-1.25));
        assert_eq!(default_entries[1].cost, 0.0);
        assert_eq!(default_entries[1].recorded_cost, Some(0.0));

        let named_entries = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
            "omp",
        )
        .unwrap();
        assert_eq!(named_entries.len(), 2);
        assert_eq!(named_entries[0].cost, expected_cost);
        assert_eq!(named_entries[0].recorded_cost, Some(-1.25));
        assert_eq!(named_entries[1].cost, 0.0);
        assert_eq!(named_entries[1].recorded_cost, Some(0.0));
    }

    #[test]
    fn auto_recalculates_non_finite_source_cost_while_display_preserves_it() {
        let pricing = pricing_for_cost_tests();
        let usage = usage_for_cost_tests();
        let candidates = [("test-model", MatchKind::Fuzzy)];
        let cost = |stored_cost, mode| {
            price_entry(
                EntryPricing {
                    usage,
                    reasoning_tokens: 0,
                    residual_tokens: 0,
                    reported_model: None,
                    candidates: &candidates,
                    stored_cost,
                    timestamp: None,
                },
                PriceTable::for_map(mode, &pricing),
            )
            .cost
        };
        assert!(cost(Some(f64::NAN), CostMode::Display).is_nan());
        assert_eq!(
            cost(
                source_cost_for_mode(Some(f64::INFINITY), CostMode::Auto, false),
                CostMode::Auto,
            ),
            0.018000000000000002
        );
        assert_eq!(
            cost(Some(f64::NEG_INFINITY), CostMode::Calculate),
            0.018000000000000002
        );
    }

    #[test]
    fn auto_reports_missing_pricing_for_invalid_source_cost() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model","usage":{"input":1000,"output":2000,"cost":{"total":-1.25}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
        let pricing = ccusage_core::PricingMap::default();

        let default_entry = read_session_file(
            &file,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(default_entry.cost, 0.0);
        assert_eq!(
            default_entry.missing_pricing_model.as_deref(),
            Some("unknown-model")
        );

        let named_entry = read_session_file_for_store(
            &file,
            &fixture.path("sessions"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::Auto(&pricing),
            "omp",
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(named_entry.cost, 0.0);
        assert_eq!(
            named_entry.missing_pricing_model.as_deref(),
            Some("unknown-model")
        );
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let fixture = ccusage_test_support::fs_fixture!({
            "sessions/p/s.jsonl": r#"{"type":"message","timestamp":"2026-11-17T10:00:00Z","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1000,"output":0}}}"#
        });
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let rows = read_session_file_data_for_store_with_diagnostics(
            &fixture.path("sessions/p/s.jsonl"),
            &fixture.path("sessions"),
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            "pi",
            &Diagnostics::without_clock("pi"),
        )
        .unwrap()
        .entries;
        assert_eq!(rows[0].model.as_deref(), Some("[pi] gpt-5.2"));
        assert!((rows[0].cost - 0.00175).abs() < 1e-12);
    }
}
