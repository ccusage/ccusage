//! Persistent cache for date-bucketed usage reports.
//!
//! Claude Code deletes session transcripts under `~/.claude/projects/` once they
//! are older than `cleanupPeriodDays` (default 30). Because `daily`/`weekly`/
//! `monthly` rescan and recompute from the surviving JSONL files on every run and
//! persist nothing, pruned days silently disappear from historical reports.
//!
//! This module keeps a small on-disk snapshot of the per-day summaries so that a
//! day whose logs were pruned is served from cache instead of vanishing. The
//! cache only ever *adds back* history: for any day it keeps the higher-cost of
//! `{live, cached}`, and live data wins on ties, so it can never override or
//! inflate what the live scan currently reports.
//!
//! Two independent views share one file, isolated by an `agent` discriminator:
//! the Claude-only commands (`ccusage claude daily|weekly|monthly`) cache
//! project-keyed rows with `agent = None`, while the multi-agent report
//! (`ccusage daily|weekly|monthly`) caches agent-keyed rows. Rows are further
//! isolated by a scope (timezone + cost mode) so incompatible runs never mix.

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ModelBreakdown, UsageSummary,
    cli::{CostMode, SharedArgs},
};

const CACHE_VERSION: u32 = 1;

#[derive(Default, Serialize, Deserialize)]
struct CacheStore {
    version: u32,
    scopes: BTreeMap<String, Vec<CachedSummary>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CachedSummary {
    pub(crate) date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) project: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    #[serde(default)]
    pub(crate) extra_total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) total_tokens: Option<u64>,
    pub(crate) total_cost: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credits: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message_count: Option<u64>,
    #[serde(default)]
    pub(crate) models_used: Vec<String>,
    #[serde(default)]
    pub(crate) model_breakdowns: Vec<CachedBreakdown>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CachedBreakdown {
    pub(crate) model_name: String,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    #[serde(default)]
    pub(crate) extra_total_tokens: u64,
    pub(crate) cost: f64,
    #[serde(default)]
    pub(crate) missing_pricing: bool,
}

impl CachedSummary {
    fn from_summary(row: &UsageSummary) -> Option<Self> {
        let date = row.date.clone()?;
        Some(Self {
            date,
            agent: None,
            project: row.project.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            extra_total_tokens: row.extra_total_tokens,
            total_tokens: None,
            total_cost: row.total_cost,
            credits: row.credits,
            message_count: row.message_count,
            models_used: row.models_used.clone(),
            model_breakdowns: row
                .model_breakdowns
                .iter()
                .map(CachedBreakdown::from_breakdown)
                .collect(),
        })
    }

    fn into_summary(self) -> UsageSummary {
        UsageSummary {
            date: Some(self.date),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            extra_total_tokens: self.extra_total_tokens,
            total_cost: self.total_cost,
            credits: self.credits,
            message_count: self.message_count,
            models_used: self.models_used,
            model_breakdowns: self
                .model_breakdowns
                .into_iter()
                .map(CachedBreakdown::into_breakdown)
                .collect(),
            project: self.project,
            versions: None,
        }
    }
}

impl CachedBreakdown {
    pub(crate) fn from_breakdown(breakdown: &ModelBreakdown) -> Self {
        Self {
            model_name: breakdown.model_name.clone(),
            input_tokens: breakdown.input_tokens,
            output_tokens: breakdown.output_tokens,
            cache_creation_tokens: breakdown.cache_creation_tokens,
            cache_read_tokens: breakdown.cache_read_tokens,
            extra_total_tokens: breakdown.extra_total_tokens,
            cost: breakdown.cost,
            missing_pricing: breakdown.missing_pricing,
        }
    }

    pub(crate) fn into_breakdown(self) -> ModelBreakdown {
        ModelBreakdown {
            model_name: self.model_name,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            extra_total_tokens: self.extra_total_tokens,
            cost: self.cost,
            missing_pricing: self.missing_pricing,
        }
    }
}

/// An opened cache scope, holding the on-disk store so a caller can read the
/// rows for the active scope, merge in fresh data, and commit the result.
pub(crate) struct CacheSession {
    path: PathBuf,
    store: CacheStore,
    scope: String,
}

/// Open the cache for this run, or `None` when caching is disabled (`--no-cache`)
/// or no cache location can be resolved.
pub(crate) fn open(shared: &SharedArgs) -> Option<CacheSession> {
    if shared.no_cache {
        return None;
    }
    let path = cache_file_path()?;
    let scope = scope_key(shared);
    let store = load_store(&path);
    Some(CacheSession { path, store, scope })
}

impl CacheSession {
    /// Remove and return all cached rows for the active scope. The caller
    /// partitions these into the rows relevant to its view and the rest, then
    /// passes both back to [`commit`](Self::commit).
    pub(crate) fn take_rows(&mut self) -> Vec<CachedSummary> {
        self.store.scopes.remove(&self.scope).unwrap_or_default()
    }

    /// Persist the full set of rows for the active scope (best effort).
    pub(crate) fn commit(mut self, rows: Vec<CachedSummary>) {
        self.store.scopes.insert(self.scope.clone(), rows);
        save_store(&self.path, &self.store);
    }
}

/// Merge freshly computed Claude per-day summaries with the on-disk cache,
/// persist the result, and return the merged rows. Used by the Claude-only
/// `daily`/`weekly`/`monthly` commands.
///
/// `group_by_project` selects the view (per-project vs project-agnostic) and a
/// single-project `project_filter` keeps `daily --project foo` from resurrecting
/// other projects. Disabled runs return the live rows unchanged.
pub(crate) fn merge_daily_summaries(
    rows: Vec<UsageSummary>,
    shared: &SharedArgs,
    group_by_project: bool,
    project_filter: Option<&str>,
) -> Vec<UsageSummary> {
    let Some(mut session) = open(shared) else {
        return rows;
    };

    // A row belongs to this run only when it is project-keyed (no agent), matches
    // the view, and — when a single project is requested — that project.
    let (relevant, other_view): (Vec<CachedSummary>, Vec<CachedSummary>) =
        session.take_rows().into_iter().partition(|row| {
            row.agent.is_none()
                && row.project.is_some() == group_by_project
                && project_filter.is_none_or(|filter| row.project.as_deref() == Some(filter))
        });

    let merged = merge_view(relevant, rows);

    let mut to_store = other_view;
    to_store.extend(merged.iter().filter_map(CachedSummary::from_summary));
    session.commit(to_store);

    merged
}

/// Pure merge step for the Claude view: keep the higher-cost summary per
/// `(project, date)`, preferring live on ties, and preserve cached-only days.
fn merge_view(cached: Vec<CachedSummary>, live: Vec<UsageSummary>) -> Vec<UsageSummary> {
    let mut merged: BTreeMap<(Option<String>, String), UsageSummary> = BTreeMap::new();
    let mut passthrough = Vec::new();

    for row in cached {
        let key = (row.project.clone(), row.date.clone());
        merged.insert(key, row.into_summary());
    }

    for row in live {
        let Some(date) = row.date.clone() else {
            passthrough.push(row);
            continue;
        };
        let key = (row.project.clone(), date);
        match merged.get(&key) {
            Some(existing) if existing.total_cost > row.total_cost => {}
            _ => {
                merged.insert(key, row);
            }
        }
    }

    let mut result = merged.into_values().collect::<Vec<_>>();
    result.append(&mut passthrough);
    result
}

fn scope_key(shared: &SharedArgs) -> String {
    let tz = shared.timezone.as_deref().unwrap_or("system");
    let mode = match shared.mode {
        CostMode::Auto => "auto",
        CostMode::Calculate => "calculate",
        CostMode::Display => "display",
    };
    format!("{tz}|{mode}")
}

fn cache_file_path() -> Option<PathBuf> {
    if let Ok(dir) = env::var("CCUSAGE_CACHE_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("daily-cache.json"));
        }
    }
    // Store alongside the Claude config dir but outside `projects/`, the path
    // Claude prunes, so the cache itself survives cleanup.
    let base = crate::claude_paths().ok()?.into_iter().next()?;
    Some(base.join("ccusage").join("daily-cache.json"))
}

fn load_store(path: &PathBuf) -> CacheStore {
    let Ok(contents) = fs::read_to_string(path) else {
        return CacheStore::default();
    };
    match serde_json::from_str::<CacheStore>(&contents) {
        Ok(store) if store.version == CACHE_VERSION => store,
        // Unknown/older format: start fresh rather than risk corrupt merges.
        _ => CacheStore::default(),
    }
}

fn save_store(path: &PathBuf, store: &CacheStore) {
    let store = CacheStore {
        version: CACHE_VERSION,
        scopes: store.scopes.clone(),
    };
    let Ok(serialized) = serde_json::to_string(&store) else {
        return;
    };
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    // Best effort, atomic-ish write: a failure must never break the report.
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, serialized).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(date: &str, project: Option<&str>, cost: f64, input: u64) -> UsageSummary {
        UsageSummary {
            date: Some(date.to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: input,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: cost,
            credits: None,
            message_count: None,
            models_used: vec!["claude-sonnet-4-20250514".to_string()],
            model_breakdowns: vec![ModelBreakdown {
                model_name: "claude-sonnet-4-20250514".to_string(),
                input_tokens: input,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                extra_total_tokens: 0,
                cost,
                missing_pricing: false,
            }],
            project: project.map(str::to_string),
            versions: None,
        }
    }

    fn cached(date: &str, project: Option<&str>, cost: f64, input: u64) -> CachedSummary {
        CachedSummary::from_summary(&summary(date, project, cost, input)).unwrap()
    }

    fn by_date(rows: &[UsageSummary], date: &str) -> Option<f64> {
        rows.iter()
            .find(|row| row.date.as_deref() == Some(date))
            .map(|row| row.total_cost)
    }

    #[test]
    fn keeps_cached_day_when_live_logs_were_pruned() {
        let cached = vec![cached("2026-01-01", None, 10.0, 100)];
        let live = vec![summary("2026-02-01", None, 5.0, 50)];

        let merged = merge_view(cached, live);

        assert_eq!(by_date(&merged, "2026-01-01"), Some(10.0));
        assert_eq!(by_date(&merged, "2026-02-01"), Some(5.0));
    }

    #[test]
    fn never_lets_a_day_shrink_below_its_cached_cost() {
        // Same day, live partially pruned (lower cost) than the cached snapshot.
        let cached = vec![cached("2026-01-01", None, 10.0, 100)];
        let live = vec![summary("2026-01-01", None, 4.0, 40)];

        let merged = merge_view(cached, live);

        assert_eq!(by_date(&merged, "2026-01-01"), Some(10.0));
    }

    #[test]
    fn refreshes_day_when_live_cost_grew() {
        // A still-active day that accumulated more usage since the cache write.
        let cached = vec![cached("2026-01-01", None, 10.0, 100)];
        let live = vec![summary("2026-01-01", None, 14.0, 140)];

        let merged = merge_view(cached, live);

        assert_eq!(by_date(&merged, "2026-01-01"), Some(14.0));
        let row = merged
            .iter()
            .find(|row| row.date.as_deref() == Some("2026-01-01"))
            .unwrap();
        assert_eq!(row.input_tokens, 140);
    }

    #[test]
    fn live_wins_on_a_cost_tie() {
        let cached = vec![cached("2026-01-01", None, 10.0, 100)];
        let live = vec![summary("2026-01-01", None, 10.0, 999)];

        let merged = merge_view(cached, live);

        let row = merged
            .iter()
            .find(|row| row.date.as_deref() == Some("2026-01-01"))
            .unwrap();
        assert_eq!(row.input_tokens, 999);
    }

    #[test]
    fn per_project_rows_do_not_collide_across_projects() {
        let cached = vec![
            cached("2026-01-01", Some("/a"), 10.0, 100),
            cached("2026-01-01", Some("/b"), 7.0, 70),
        ];
        let live = vec![summary("2026-01-01", Some("/a"), 4.0, 40)];

        let merged = merge_view(cached, live);

        assert_eq!(merged.len(), 2);
        let a = merged
            .iter()
            .find(|row| row.project.as_deref() == Some("/a"))
            .unwrap();
        let b = merged
            .iter()
            .find(|row| row.project.as_deref() == Some("/b"))
            .unwrap();
        assert_eq!(a.total_cost, 10.0);
        assert_eq!(b.total_cost, 7.0);
    }
}
