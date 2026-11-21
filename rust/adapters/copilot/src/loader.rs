use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{collections::HashMap, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        CopilotSourceUsage, CopilotUsageEntry, parse_session_state_file, read_otel_calls,
        reconcile_otel_calls,
    },
    paths::CopilotSourceKind,
};
use ccusage_adapter_common::{
    source::LoadContext,
    units::{UnitError, read_typed_units},
};
use ccusage_core::Diagnostics;

use crate::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, stored_cost_for_mode,
};

mod credits;
mod reconcile;

use credits::{credit_gap_entries, receipt_ledgers};
use reconcile::{
    deduplicate_usage_records, otel_call_timestamps, reconcile_session_state_entries,
    recorded_otel_calls,
};

pub(crate) fn load_entries_for_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let sources = super::paths::paths(cx.diagnostics)?;
    let parsed = read_typed_units(&sources, shared.single_thread, cx.diagnostics, |source| {
        read_source_file(&source.path, source.kind, cx.diagnostics)
    });
    let mut otel_calls = Vec::new();
    let mut session_state_entries = Vec::new();
    let mut usage_records = Vec::new();
    let mut credit_snapshots = Vec::new();
    for (source, file_usage) in sources.iter().map(|source| source.kind).zip(parsed) {
        let Some(file_usage) = file_usage else {
            continue;
        };
        match source {
            CopilotSourceKind::Otel => otel_calls.extend(file_usage.otel_calls),
            CopilotSourceKind::SessionState => {
                session_state_entries.extend(file_usage.entries);
                usage_records.extend(file_usage.usage_records);
                credit_snapshots.extend(file_usage.credit_snapshots);
            }
        }
    }
    let otel_entries = reconcile_otel_calls(otel_calls);
    let (since_millis, until_millis) = cx.window.bounds_ms();
    let session_state = reconcile_session_state_entries(
        session_state_entries,
        &otel_call_timestamps(&otel_entries),
        since_millis,
        until_millis,
    );
    let otel_after = session_state
        .otel_after
        .iter()
        .map(|((session_id, model), cutoff)| ((session_id.as_str(), model.as_str()), cutoff))
        .collect::<HashMap<_, _>>();
    let survives_shutdown = |entry: &CopilotUsageEntry, in_report| {
        otel_after
            .get(&(entry.session_id.as_str(), entry.model.as_str()))
            .and_then(|cutoff| {
                if in_report {
                    cutoff.visible
                } else {
                    cutoff.full
                }
            })
            .is_none_or(|after| entry.timestamp > after)
    };
    let usage_records = deduplicate_usage_records(usage_records);
    let matches = recorded_otel_calls(&otel_entries, &usage_records);
    let credit_gaps = if credit_snapshots
        .iter()
        .any(|snapshot| !snapshot.is_shutdown)
    {
        let uncovered = otel_entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                survives_shutdown(entry, false) && matches.get(index) != Some(&true)
            })
            .fold(
                HashMap::<&str, TimestampMs>::new(),
                |mut earliest, (_, entry)| {
                    earliest
                        .entry(entry.session_id.as_str())
                        .and_modify(|timestamp| *timestamp = (*timestamp).min(entry.timestamp))
                        .or_insert(entry.timestamp);
                    earliest
                },
            );
        let receipts = receipt_ledgers(&credit_snapshots, &usage_records);
        credit_gap_entries(
            credit_snapshots,
            &session_state.attributed_nano_aiu,
            &uncovered,
            &receipts,
            since_millis,
            until_millis,
        )
    } else {
        Vec::new()
    };
    let visible_otel = otel_entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (survives_shutdown(&entry, true) && !matches.contains_key(&index)).then_some(entry)
        });

    let mut entries = session_state
        .entries
        .into_iter()
        .chain(usage_records.into_iter().map(|record| record.entry))
        .chain(visible_otel)
        .chain(credit_gaps)
        .map(|entry| usage_entry_to_loaded(entry, tz, shared.mode, pricing))
        .collect::<Vec<_>>();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

fn read_source_file(
    path: &Path,
    kind: CopilotSourceKind,
    diagnostics: &Diagnostics,
) -> std::result::Result<CopilotSourceUsage, UnitError> {
    match kind {
        CopilotSourceKind::Otel => Ok(CopilotSourceUsage {
            otel_calls: Some(read_otel_calls(path, diagnostics)?),
            ..CopilotSourceUsage::default()
        }),
        CopilotSourceKind::SessionState => parse_session_state_file(path, diagnostics),
    }
}

// NOTE(2026-10-10, #1824): One AIU is one GitHub AI credit worth USD 0.01. A nano-AIU is a billionth of that credit.
fn nano_aiu_to_usd(nano_aiu: u64) -> f64 {
    nano_aiu as f64 / 1e11
}

fn usage_entry_to_loaded(
    entry: CopilotUsageEntry,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usd = entry.nano_aiu.map(nano_aiu_to_usd);
    let remainder = entry
        .extra_total_tokens
        .saturating_add(entry.fallback_output_tokens);
    let reasoning_tokens = entry.reasoning_output_tokens.min(remainder);
    let logged = entry.logged_model.as_deref().unwrap_or(&entry.model);
    let model = (entry.model.as_str(), MatchKind::Fuzzy);
    let with_logged = [(logged, MatchKind::Exact), model];
    let candidates: &[(_, _)] = if entry.logged_model.is_some() {
        &with_logged
    } else {
        std::slice::from_ref(&model)
    };
    let priced = price_entry(
        EntryPricing {
            // NOTE(2026-11-20, #2365): Shutdown intervals total several calls and record no per-call split. Only an entry that is exactly one call can select the long-context tier.
            granularity: if entry.request_count == 1 {
                UsageGranularity::Request
            } else {
                UsageGranularity::Aggregate
            },
            usage: TokenUsageRaw {
                output_tokens: usage
                    .output_tokens
                    .saturating_sub(entry.fallback_output_tokens),
                ..usage
            },
            reasoning_tokens,
            residual_tokens: remainder - reasoning_tokens,
            reported_model: Some(&entry.model),
            candidates,
            stored_cost: cost_usd,
            timestamp: Some(entry.timestamp),
        },
        pricing,
    );
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| crate::model_aliases::resolve_model_name(&entry.model).into_owned());
    let stored_cost = stored_cost_for_mode(cost_usd, mode);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: (entry.request_count > 0).then_some(entry.request_count),
        model: Some(entry.model),
        usage,
        message_id: Some(entry.dedup_key),
        version: None,
        recorded_cost: cost_usd,
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        stored_cost,
    }
}

#[cfg(test)]
mod tests;
