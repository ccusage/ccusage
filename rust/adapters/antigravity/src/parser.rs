use ccusage_core::cost::PriceTable;
use std::{collections::HashMap, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use ccusage_core::{Diagnostics, Skip, SkipKind};

use crate::{LoadedEntry, TimestampMs, TokenUsageRaw, date_at};

mod model;
mod proto;
mod sqlite;

use model::{
    is_resolved_model, model_from_id, model_resolution, normalize_antigravity_model,
    price_antigravity,
};
pub(super) use sqlite::parse_sqlite_file;

const DEFAULT_MODEL: &str = "gemini-internal-model";

#[derive(Debug, Clone)]
pub(super) struct AntigravityUsageEvent {
    pub(super) timestamp: TimestampMs,

    session_id: String,
    model: String,
    logged_model: Option<String>,
    provider: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_output_tokens: u64,
    total_tokens: u64,
    message_id: Option<String>,
    pub(super) identities: Vec<String>,
    timestamp_rank: u8,
    message_id_rank: u8,
}

#[derive(Debug, Default)]
struct GeneratorMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<TimestampMs>,
    step_indices: Vec<i64>,
}

#[derive(Debug, Default)]
struct StepMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    provider: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<TimestampMs>,
}

#[derive(Debug, Default)]
struct ModelUsage {
    model_id: Option<u64>,
    input_tokens: u64,
    total_output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    visible_output_tokens: u64,
    provider: Option<u64>,
    message_id: Option<String>,
    response_id: Option<String>,
    provider_assigned_message_id: Option<String>,
}

impl ModelUsage {
    fn is_token_bearing(&self) -> bool {
        self.input_tokens > 0
            || self.total_output_tokens > 0
            || self.cache_creation_tokens > 0
            || self.cache_read_tokens > 0
            || self.reasoning_tokens > 0
            || self.visible_output_tokens > 0
    }

    fn same_token_counts(&self, other: &Self) -> bool {
        self.model_id == other.model_id
            && self.input_tokens == other.input_tokens
            && self.total_output_tokens == other.total_output_tokens
            && self.cache_creation_tokens == other.cache_creation_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.reasoning_tokens == other.reasoning_tokens
            && self.visible_output_tokens == other.visible_output_tokens
    }

    fn is_mirror_of(&self, primary: &Self) -> bool {
        self.same_token_counts(primary)
            && self
                .identity_keys()
                .iter()
                .all(|identity| primary.identity_keys().contains(identity))
    }

    fn identity_keys(&self) -> Vec<String> {
        [
            self.response_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("response:{value}")),
            self.provider_assigned_message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("provider:{value}")),
            self.message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("message:{value}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn preferred_message_id(&self) -> (Option<String>, u8) {
        if let Some(value) = self
            .response_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return (Some(value.to_string()), 3);
        }
        if let Some(value) = self
            .provider_assigned_message_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return (Some(value.to_string()), 2);
        }
        (
            self.message_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            1,
        )
    }
}

#[derive(Debug)]
pub(super) enum ParsedDatabase {
    Events(Vec<AntigravityUsageEvent>),
    EmptySnapshot,
    NoGenerationTable,
}

#[derive(Clone, Copy)]
struct EventContext<'a> {
    model: Option<&'a str>,
    model_id: Option<u64>,
    provider: Option<u64>,
    timestamp: Option<(TimestampMs, u8)>,
    trajectory_timestamp: Option<TimestampMs>,
    diagnostics: &'a Diagnostics,
    record: (&'a Path, u32),
    session_id: &'a str,
    fallback_identities: &'a [String],
}

fn append_usage_event(
    events: &mut Vec<AntigravityUsageEvent>,
    identity_timestamps: &mut HashMap<String, (TimestampMs, u8)>,
    usage: ModelUsage,
    context: EventContext<'_>,
) {
    if !usage.is_token_bearing() {
        return;
    }
    let mut identities = usage.identity_keys();
    if identities.is_empty() {
        identities.extend(context.fallback_identities.iter().cloned());
    }
    let Some((timestamp, timestamp_rank)) = context
        .timestamp
        .or_else(|| {
            identities
                .iter()
                .chain(context.fallback_identities)
                .find_map(|identity| identity_timestamps.get(identity).copied())
        })
        .or_else(|| context.trajectory_timestamp.map(|timestamp| (timestamp, 1)))
    else {
        context.diagnostics.record(Skip::record(
            context.record.0,
            context.record.1,
            SkipKind::InvalidTimestamp,
            "Antigravity usage record has no valid generation, step or trajectory timestamp",
        ));
        return;
    };
    let total_output_tokens = usage.total_output_tokens.max(
        usage
            .visible_output_tokens
            .saturating_add(usage.reasoning_tokens),
    );
    let output_tokens = usage
        .visible_output_tokens
        .max(total_output_tokens.saturating_sub(usage.reasoning_tokens));
    let reasoning_tokens = usage
        .reasoning_tokens
        .max(total_output_tokens.saturating_sub(output_tokens));
    let id_model = usage.model_id.and_then(model_from_id);
    let row_model = context.model.and_then(normalize_antigravity_model);
    let same_model_id = context.model_id == usage.model_id;
    let model = id_model
        .clone()
        .filter(|model| is_resolved_model(model))
        .or_else(|| {
            row_model
                .as_ref()
                .filter(|model| same_model_id && is_resolved_model(model))
                .cloned()
        })
        .or(id_model)
        .or_else(|| row_model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let logged_model = context
        .model
        .filter(|raw| *raw != model && (same_model_id || row_model.as_ref() == Some(&model)))
        .map(str::to_owned);
    let provider = usage.provider.or(context.provider);
    let (message_id, message_id_rank) = usage.preferred_message_id();
    let timestamp_value = timestamp;
    for identity in &identities {
        let should_update =
            identity_timestamps
                .get(identity)
                .is_none_or(|(old_timestamp, old_rank)| {
                    timestamp_rank > *old_rank
                        || (timestamp_rank == *old_rank && timestamp_value < *old_timestamp)
                });
        if should_update {
            identity_timestamps.insert(identity.clone(), (timestamp_value, timestamp_rank));
        }
    }
    events.push(AntigravityUsageEvent {
        timestamp,

        session_id: context.session_id.to_string(),
        model,
        logged_model,
        provider,
        input_tokens: usage.input_tokens,
        output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        reasoning_tokens,
        total_output_tokens,
        total_tokens: usage
            .input_tokens
            .saturating_add(usage.cache_creation_tokens)
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(total_output_tokens),
        message_id,
        identities,
        timestamp_rank,
        message_id_rank,
    });
}

pub(super) fn merge_usage_event(
    target: &mut AntigravityUsageEvent,
    duplicate: AntigravityUsageEvent,
) {
    target.input_tokens = target.input_tokens.max(duplicate.input_tokens);
    target.output_tokens = target.output_tokens.max(duplicate.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .max(duplicate.cache_creation_tokens);
    target.cache_read_tokens = target.cache_read_tokens.max(duplicate.cache_read_tokens);
    target.reasoning_tokens = target.reasoning_tokens.max(duplicate.reasoning_tokens);
    target.total_output_tokens = target
        .total_output_tokens
        .max(duplicate.total_output_tokens)
        .max(target.output_tokens.saturating_add(target.reasoning_tokens));
    target.total_tokens = target
        .input_tokens
        .saturating_add(target.cache_creation_tokens)
        .saturating_add(target.cache_read_tokens)
        .saturating_add(target.total_output_tokens);
    if model_resolution(&duplicate.model) > model_resolution(&target.model) {
        target.model = duplicate.model.clone();
        target.logged_model = duplicate.logged_model;
    } else if target.model == duplicate.model && target.logged_model.is_none() {
        target.logged_model = duplicate.logged_model;
    }
    if target.provider.is_none() {
        target.provider = duplicate.provider;
    }
    let use_duplicate_timestamp = duplicate.timestamp_rank > target.timestamp_rank
        || (duplicate.timestamp_rank == target.timestamp_rank
            && duplicate.timestamp < target.timestamp);
    if use_duplicate_timestamp {
        target.timestamp = duplicate.timestamp;

        target.timestamp_rank = duplicate.timestamp_rank;
    }
    if duplicate.message_id_rank > target.message_id_rank {
        target.message_id = duplicate.message_id;
        target.message_id_rank = duplicate.message_id_rank;
    }
    for identity in duplicate.identities {
        if !target.identities.contains(&identity) {
            target.identities.push(identity);
        }
    }
}

pub(super) fn event_to_loaded(
    event: AntigravityUsageEvent,
    timezone: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        cache_creation_input_tokens: event.cache_creation_tokens,
        cache_read_input_tokens: event.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: event.total_output_tokens,
        cache_creation: None,
        ..usage
    };
    let extra_total_tokens = event
        .total_output_tokens
        .saturating_sub(event.output_tokens);
    let (cost, missing_pricing_model, estimated_pricing) = price_antigravity(
        &event.model,
        event.logged_model.as_deref(),
        event.provider,
        cost_usage,
        event.timestamp,
        pricing,
    );
    LoadedEntry {
        date: date_at(event.timestamp, timezone),
        timestamp: event.timestamp,
        project: Arc::from("antigravity"),
        session_id: Arc::from(event.session_id),
        project_path: Arc::from("Antigravity"),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(event.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        usage,
        message_id: event.message_id,
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

#[cfg(test)]
pub(super) mod test_support;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let model = normalize_antigravity_model("gemini-3.6-flash-high").unwrap();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"gemini-3.6-flash":{"input_cost_per_token":0.00000075,"output_cost_per_token":0.00000375},"gemini/gemini-3.6-flash-lite":{"input_cost_per_token":0.0000001,"output_cost_per_token":0.0000004}}"#);
        let (cost, missing) = price_antigravity(
            &model,
            None,
            None,
            TokenUsageRaw {
                input_tokens: 1000,
                ..Default::default()
            },
            ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),
            PriceTable::Calculate(&pricing),
        );
        assert!((cost - 0.00075).abs() < 1e-12);
        assert_eq!(missing, None);
    }
}
