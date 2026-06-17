//! Cache integration for the multi-agent (`ccusage daily|weekly|monthly`) report.
//!
//! Works on the per-agent, per-day [`AllRow`]s produced before bucket
//! aggregation. Live rows are kept intact (so metadata and exact token totals
//! survive); the cache only steps in for days a live scan no longer sees, or
//! whose cost shrank because logs were pruned. See [`crate::cache`] for the
//! shared store and the add-back-only merge policy.

use std::collections::BTreeMap;

use crate::{
    cache::{self, CachedBreakdown, CachedSummary},
    cli::SharedArgs,
};

use super::types::AllRow;

enum Choice {
    Live(AllRow),
    Cached(CachedSummary),
}

/// Merge the freshly loaded daily rows with the cached agent-keyed history,
/// persist the union, and return the rows to aggregate. Disabled runs
/// (`--no-cache`) return the live rows unchanged.
pub(super) fn merge_all_daily_rows(rows: Vec<AllRow>, shared: &SharedArgs) -> Vec<AllRow> {
    let Some(mut session) = cache::open(shared) else {
        return rows;
    };

    // The multi-agent view owns agent-keyed rows; project-keyed rows belong to
    // the Claude-only commands and are preserved untouched.
    let (relevant, other_view): (Vec<CachedSummary>, Vec<CachedSummary>) = session
        .take_rows()
        .into_iter()
        .partition(|row| row.agent.is_some());

    let mut chosen: BTreeMap<(String, String), Choice> = BTreeMap::new();
    for record in relevant {
        let agent = record.agent.clone().unwrap_or_default();
        chosen.insert((agent, record.date.clone()), Choice::Cached(record));
    }
    for row in rows {
        let key = (row.agent.to_string(), row.period.clone());
        let keep_cached = matches!(
            chosen.get(&key),
            Some(Choice::Cached(record)) if record.total_cost > row.total_cost
        );
        if !keep_cached {
            chosen.insert(key, Choice::Live(row));
        }
    }

    let mut out = Vec::with_capacity(chosen.len());
    let mut to_store = other_view;
    for (_key, choice) in chosen {
        match choice {
            Choice::Live(row) => {
                to_store.push(record_from_row(&row));
                out.push(row);
            }
            Choice::Cached(record) => {
                if let Some(row) = row_from_record(&record) {
                    out.push(row);
                }
                to_store.push(record);
            }
        }
    }
    session.commit(to_store);
    out
}

fn record_from_row(row: &AllRow) -> CachedSummary {
    CachedSummary {
        date: row.period.clone(),
        agent: Some(row.agent.to_string()),
        project: None,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_creation_tokens: row.cache_creation_tokens,
        cache_read_tokens: row.cache_read_tokens,
        extra_total_tokens: 0,
        total_tokens: Some(row.total_tokens),
        total_cost: row.total_cost,
        credits: None,
        message_count: None,
        models_used: row.models_used.clone(),
        model_breakdowns: row
            .model_breakdowns
            .iter()
            .map(CachedBreakdown::from_breakdown)
            .collect(),
    }
}

fn row_from_record(record: &CachedSummary) -> Option<AllRow> {
    let agent = static_agent_name(record.agent.as_deref()?)?;
    let total_tokens = record.total_tokens.unwrap_or(
        record.input_tokens
            + record.output_tokens
            + record.cache_creation_tokens
            + record.cache_read_tokens
            + record.extra_total_tokens,
    );
    Some(AllRow {
        period: record.date.clone(),
        agent,
        models_used: record.models_used.clone(),
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_creation_tokens: record.cache_creation_tokens,
        cache_read_tokens: record.cache_read_tokens,
        total_tokens,
        total_cost: record.total_cost,
        metadata: None,
        metadata_agents: Some(vec![agent]),
        agent_breakdowns: None,
        model_breakdowns: record
            .model_breakdowns
            .iter()
            .cloned()
            .map(CachedBreakdown::into_breakdown)
            .collect(),
    })
}

/// Resolve a stored agent name back to the `&'static str` the report expects.
/// Unknown names (e.g. a cache written by a newer ccusage) are dropped rather
/// than guessed.
fn static_agent_name(name: &str) -> Option<&'static str> {
    let resolved = match name {
        "claude" => "claude",
        "codex" => "codex",
        "opencode" => "opencode",
        "amp" => "amp",
        "droid" => "droid",
        "codebuff" => "codebuff",
        "hermes" => "hermes",
        "pi" => "pi",
        "goose" => "goose",
        "openclaw" => "openclaw",
        "kilo" => "kilo",
        "copilot" => "copilot",
        "gemini" => "gemini",
        "kimi" => "kimi",
        "qwen" => "qwen",
        _ => return None,
    };
    Some(resolved)
}
