use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::AgentModel,
};
use std::{
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, TokenUsageRaw, cli::CostMode, date_at, fast::LinePrefilter, stored_cost_for_mode,
};
use ccusage_adapter_common::{json, jsonl, units::UnitError};
use ccusage_core::{Diagnostics, Skip, SkipKind};

mod replay;

use replay::{matching_replay_prefix, replay_billed_cost_bits, replay_usage_path};

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

pub(crate) fn read_session_header(
    path: &Path,
    diagnostics: &Diagnostics,
) -> std::result::Result<PiSessionData, UnitError> {
    let header = jsonl::read(
        path,
        Some(diagnostics),
        jsonl::ReadPolicy::PLAIN,
        |line, _, _| ControlFlow::Break(parse_session_header(line.bytes())),
    )?;
    Ok(PiSessionData {
        header: header.flatten(),
        usage: Vec::new(),
        replay_usage_path: PiReplayUsagePath::Linear,
        entries: Vec::new(),
    })
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
        let additional = ccusage_core::AdditionalTokens::from_total(usage, 0, total);
        let Some(counted_tokens) =
            ccusage_core::checked_total_usage_tokens(usage, additional.report_remainder)
        else {
            diagnostics.record(Skip::record(
                path,
                number,
                SkipKind::InvalidData,
                "Pi token counts exceed the u64 range",
            ));
            return;
        };
        if counted_tokens == 0 {
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
                granularity: UsageGranularity::Request,
                usage,
                reasoning_tokens: 0,
                reported_model: identity.as_ref().map(AgentModel::display),
                candidates: &candidates,
                stored_cost: source_cost,
                timestamp: Some(timestamp),
            },
            pricing,
        );
        let cost = priced.cost;
        let estimated_pricing = priced.estimated;
        let extra_total_tokens = additional.report_remainder;
        usage_records.push(PiUsageRecord {
            signature: PiUsageSignature {
                timestamp,
                model: raw_model.clone(),
                input_tokens: usage.input_tokens,
                output_tokens: additional.replay_counts(usage.output_tokens).0,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
                effective_total_tokens: crate::total_usage_tokens(usage)
                    .saturating_add(extra_total_tokens),
                billed_cost_bits: replay_billed_cost_bits(mode, cost),
            },
            entry_id: record.id.clone(),
        });
        let missing_pricing_model = priced
            .missing_pricing
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
                date: date_at(timestamp, tz),
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
                estimated_pricing,
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

fn parse_session_header(line: &[u8]) -> Option<PiSessionHeader> {
    let header = json::parse::<PiHeaderLine>(line).ok()?;
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
    let (output_tokens, remainder) = ccusage_core::AdditionalTokens {
        reasoning: 0,
        report_remainder: entry.extra_total_tokens,
    }
    .replay_counts(usage.output_tokens);
    PiEntryId {
        project: Arc::clone(&entry.project),
        session_id: Arc::clone(&entry.session_id),
        timestamp: entry.source_timestamp.clone(),
        model: entry.model.clone(),
        tokens: [
            usage.input_tokens,
            output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
            remainder,
        ],
        cost_bits: entry.cost.to_bits(),
    }
}

#[cfg(test)]
mod tests;

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
            &Diagnostics::new("pi"),
        )
        .unwrap()
        .entries;
        assert_eq!(rows[0].model.as_deref(), Some("[pi] gpt-5.2"));
        assert!((rows[0].cost - 0.00175).abs() < 1e-12);
    }
}
