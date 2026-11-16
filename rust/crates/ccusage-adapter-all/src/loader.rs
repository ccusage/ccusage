use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

#[cfg(test)]
use serde_json::json;

use ccusage_adapter_common::{
    claims::ClaudeCounted,
    rows::UnifiedRow,
    source::{
        PricingSupply, ProgressOwner, SourceOptions, SourceRows, UnifiedRequest, UsageSource,
    },
};

use crate::{
    BUILT_IN_AGENT_NAMES, Diagnostics, LoadedEntry, PricingMap, Result,
    adapter::{claude, opencode, pi},
    cli::{AgentReportKind, CostMode, NamedPiStore, SharedArgs, SourceArgs, WeekDay},
};

#[cfg(test)]
use crate::{
    CodexGroup, UsageSummary, adapter::codex, cli::CodexSpeed, filter_loaded_entries_by_date,
};
#[cfg(test)]
use ccusage_adapter_common::rows::summary_rows;

use super::{
    report::sort_rows,
    sources::SOURCES,
    types::{
        AgentLoadSpec, AgentRows, AllAccumulator, AllLoadResult, AllSectionsLoadResult,
        LoadedAgentRows,
    },
};

/// No per-source options, which is how the tests that do not exercise them
/// call the loader.
#[cfg(test)]
pub(super) static NO_SOURCES: BTreeMap<String, SourceArgs> = BTreeMap::new();

pub(super) fn load_rows(
    kind: AgentReportKind,
    shared: &SharedArgs,
    sources: &BTreeMap<String, SourceArgs>,
    start_of_week: WeekDay,
) -> Result<AllLoadResult> {
    let source_shared = shared_by_source(sources, shared);
    let (pricing, unified_pricing) =
        load_pricing(shared, &source_shared, !shared.pi_stores.is_empty());
    let load_kind = load_kind_for_report(kind);
    let loaded = load_base_rows(
        load_kind,
        shared,
        sources,
        &source_shared,
        &pricing,
        unified_pricing.as_deref(),
    )?;
    Ok(AllLoadResult {
        rows: finish_rows(kind, loaded.rows, shared, start_of_week),
        detected_agents: loaded.detected_agents,
        diagnostics: loaded.diagnostics,
    })
}

pub(super) fn load_sections(
    kinds: &[AgentReportKind],
    shared: &SharedArgs,
    sources: &BTreeMap<String, SourceArgs>,
    start_of_week: WeekDay,
) -> Result<AllSectionsLoadResult> {
    let source_shared = shared_by_source(sources, shared);
    let (pricing, unified_pricing) =
        load_pricing(shared, &source_shared, !shared.pi_stores.is_empty());
    let mut daily_base = needs_daily_family(kinds)
        .then(|| {
            load_base_rows(
                AgentReportKind::Daily,
                shared,
                sources,
                &source_shared,
                &pricing,
                unified_pricing.as_deref(),
            )
        })
        .transpose()?;
    let mut session_base = needs_session(kinds)
        .then(|| {
            load_base_rows(
                AgentReportKind::Session,
                shared,
                sources,
                &source_shared,
                &pricing,
                unified_pricing.as_deref(),
            )
        })
        .transpose()?;

    let daily_detected_agents = daily_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();
    let session_detected_agents = session_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();
    // The two loads read different files for one source, so their skips are
    // merged per source rather than listed twice.
    let mut diagnostics = Vec::new();
    for base in daily_base.iter_mut().chain(&mut session_base) {
        for loaded in std::mem::take(&mut base.diagnostics) {
            match diagnostics
                .iter()
                .find(|merged: &&Diagnostics| merged.source() == loaded.source())
            {
                Some(merged) => merged.merge(&loaded),
                None => diagnostics.push(loaded),
            }
        }
    }

    let mut sections = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let rows = if *kind == AgentReportKind::Session {
            session_base
                .as_ref()
                .map(|base| finish_rows(*kind, base.rows.clone(), shared, start_of_week))
                .unwrap_or_default()
        } else {
            daily_base
                .as_ref()
                .map(|base| finish_rows(*kind, base.rows.clone(), shared, start_of_week))
                .unwrap_or_default()
        };
        sections.push((*kind, rows));
    }

    Ok(AllSectionsLoadResult {
        sections,
        daily_detected_agents,
        session_detected_agents,
        diagnostics,
    })
}

struct CachedPricingMap {
    offline: bool,
    overrides: BTreeMap<String, crate::cli::PricingOverride>,
    pricing: Arc<PricingMap>,
}

fn cached_pricing_map(
    shared: &SharedArgs,
    bases: &mut BTreeMap<bool, Arc<PricingMap>>,
    cached: &mut Vec<CachedPricingMap>,
) -> Arc<PricingMap> {
    if let Some(entry) = cached.iter().find(|entry| {
        entry.offline == shared.offline && entry.overrides == shared.pricing_overrides
    }) {
        return Arc::clone(&entry.pricing);
    }

    let base = bases.entry(shared.offline).or_insert_with(|| {
        Arc::new(PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            std::iter::empty::<(&String, &crate::cli::PricingOverride)>(),
        ))
    });
    let pricing = if shared.pricing_overrides.is_empty() {
        Arc::clone(base)
    } else {
        Arc::new(base.with_user_overrides(shared.pricing_overrides.iter()))
    };
    cached.push(CachedPricingMap {
        offline: shared.offline,
        overrides: shared.pricing_overrides.clone(),
        pricing: Arc::clone(&pricing),
    });
    pricing
}

fn load_pricing(
    shared: &SharedArgs,
    source_shared: &BTreeMap<&'static str, SharedArgs>,
    load_unified: bool,
) -> (
    BTreeMap<&'static str, Arc<PricingMap>>,
    Option<Arc<PricingMap>>,
) {
    let mut bases = BTreeMap::new();
    let mut cached = Vec::new();
    let pricing_by_source = SOURCES
        .iter()
        .filter(|source| source.pricing_supply() == PricingSupply::Pipeline)
        .map(|source| {
            let agent = source.id();
            (
                agent,
                cached_pricing_map(&source_shared[agent], &mut bases, &mut cached),
            )
        })
        .collect();
    let unified_pricing = load_unified.then(|| cached_pricing_map(shared, &mut bases, &mut cached));
    (pricing_by_source, unified_pricing)
}

fn load_kind_for_report(kind: AgentReportKind) -> AgentReportKind {
    match kind {
        AgentReportKind::Session => AgentReportKind::Session,
        AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
            AgentReportKind::Daily
        }
    }
}

/// The shared options each source loads with: the ones the config resolved for
/// it, or the unified ones for a source the config does not address.
fn shared_by_source(
    sources: &BTreeMap<String, SourceArgs>,
    shared: &SharedArgs,
) -> BTreeMap<&'static str, SharedArgs> {
    BUILT_IN_AGENT_NAMES
        .iter()
        .map(|agent| {
            let options = sources.get(*agent).map_or(shared, |source| &source.shared);
            (*agent, options.clone())
        })
        .collect()
}

fn load_base_rows(
    load_kind: AgentReportKind,
    shared: &SharedArgs,
    sources: &BTreeMap<String, SourceArgs>,
    source_shared: &BTreeMap<&'static str, SharedArgs>,
    pricing: &BTreeMap<&'static str, Arc<PricingMap>>,
    unified_pricing: Option<&PricingMap>,
) -> Result<AllLoadResult> {
    let mut progress = crate::progress::UsageLoadProgress::new(
        crate::log_level() != Some(0)
            && crate::progress::should_show_usage_load_progress(
                shared.json,
                crate::progress::usage_load_output_is_tty(),
            ),
    );
    let pi_path = sources
        .get("pi")
        .and_then(|source| source.pi_path.as_deref());
    let (claude_counted_sender, claude_counted_receiver) = mpsc::sync_channel(1);
    let mut claude_sender = Some(claude_counted_sender);
    let mut claude_receiver = Some(claude_counted_receiver);
    let mut specs = Vec::with_capacity(SOURCES.len() + shared.pi_stores.len());
    for (index, source) in SOURCES.iter().copied().enumerate() {
        let id = source.id();
        let source_shared = &source_shared[id];
        let source_args = sources.get(id);
        let source_pricing = pricing.get(id).map(|pricing| pricing.as_ref());
        let progress_agent = crate::progress::UsageLoadAgent(source.label());
        let load: Box<dyn FnOnce() -> Result<AgentRows> + Send + '_> = match id {
            "claude" => {
                let sender = claude_sender.take().expect("Claude source is registered");
                Box::new(move || {
                    let source_rows = load_source_result(
                        source,
                        load_kind,
                        source_shared,
                        source_args,
                        source_pricing,
                        None,
                    )?;
                    let ccusage_adapter_common::source::SourceRows {
                        rows,
                        detected,
                        diagnostics,
                        claude_counted,
                    } = source_rows;
                    let counted = claude_counted.ok_or_else(|| {
                        crate::cli_error("Claude source did not return counted usage")
                    })?;
                    let _ = sender.send(counted);
                    Ok(AgentRows {
                        rows,
                        detected,
                        diagnostics,
                    })
                })
            }
            "opencode" => {
                let receiver = claude_receiver
                    .take()
                    .expect("OpenCode source is registered");
                Box::new(move || {
                    let claude_counted =
                        move || receiver.recv().unwrap_or_else(|_| ClaudeCounted::nothing());
                    let source_rows = load_source_result(
                        source,
                        load_kind,
                        source_shared,
                        source_args,
                        source_pricing,
                        Some(&claude_counted),
                    )?;
                    Ok(agent_rows(source_rows))
                })
            }
            _ => Box::new(move || {
                let source_rows = load_source_result(
                    source,
                    load_kind,
                    source_shared,
                    source_args,
                    source_pricing,
                    None,
                )?;
                Ok(agent_rows(source_rows))
            }),
        };
        specs.push(AgentLoadSpec {
            index,
            agent: id,
            progress_agent,
            load,
        });
    }
    drop(claude_sender);
    drop(claude_receiver);
    let named_pi_stores = resolve_named_pi_store_paths(&shared.pi_stores, pi_path)?;
    for store in named_pi_stores {
        let agent = leak_agent_name(&store.name);
        let store = pi::NamedStore::new(agent, store.paths);
        let index = specs.len();
        let pricing_ref =
            unified_pricing.expect("unified pricing is loaded when named pi stores are configured");
        specs.push(AgentLoadSpec {
            index,
            agent,
            progress_agent: crate::progress::UsageLoadAgent(agent),
            load: Box::new(move || {
                load_source_result(&store, load_kind, shared, None, Some(pricing_ref), None)
                    .map(agent_rows)
            }),
        });
    }
    let loaded = load_agent_rows_parallel(specs, &mut progress)?;
    let mut detected_agents = Vec::new();
    let mut rows = Vec::new();
    let mut diagnostics = Vec::with_capacity(loaded.len());
    for loaded in loaded {
        let AgentRows {
            rows: agent_rows,
            detected,
            diagnostics: agent_diagnostics,
        } = loaded.agent_rows;
        if detected {
            detected_agents.push(loaded.agent);
        }
        rows.extend(agent_rows);
        diagnostics.push(agent_diagnostics);
    }
    Ok(AllLoadResult {
        rows,
        detected_agents,
        diagnostics,
    })
}

fn finish_rows(
    kind: AgentReportKind,
    mut rows: Vec<UnifiedRow>,
    shared: &SharedArgs,
    start_of_week: WeekDay,
) -> Vec<UnifiedRow> {
    if kind == AgentReportKind::Session {
        for row in &mut rows {
            row.metadata_agents = None;
        }
        rows.sort_by(|a, b| {
            let cost_order = if shared.order_explicit && shared.order == crate::cli::SortOrder::Asc
            {
                a.total_cost.total_cmp(&b.total_cost)
            } else {
                b.total_cost.total_cmp(&a.total_cost)
            };
            cost_order
                .then_with(|| a.period.cmp(&b.period))
                .then_with(|| a.agent.cmp(b.agent))
        });
        return rows;
    }

    let mut aggregated = aggregate_rows(rows, kind, start_of_week);
    sort_rows(&mut aggregated, &shared.order);
    aggregated
}

pub(super) fn load_agent_rows_parallel(
    specs: Vec<AgentLoadSpec<'_>>,
    progress: &mut crate::progress::UsageLoadProgress,
) -> Result<Vec<LoadedAgentRows>> {
    for spec in &specs {
        progress.start(spec.progress_agent);
    }

    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let mut handles = Vec::with_capacity(specs.len());
        for spec in specs {
            let sender = sender.clone();
            handles.push((
                spec.index,
                spec.progress_agent,
                scope.spawn(move || {
                    let result = (spec.load)();
                    let _ = sender.send((spec.index, spec.agent, spec.progress_agent, result));
                }),
            ));
        }
        drop(sender);

        let mut loaded = Vec::with_capacity(handles.len());
        let mut errors = Vec::new();
        for (index, agent, progress_agent, result) in receiver {
            match result {
                Ok(agent_rows) => {
                    progress.succeed(progress_agent);
                    loaded.push(LoadedAgentRows {
                        index,
                        agent,
                        agent_rows,
                    });
                }
                Err(error) => {
                    progress.fail(progress_agent);
                    errors.push((index, error));
                }
            }
        }

        for (index, progress_agent, handle) in handles {
            if handle.join().is_err() {
                progress.fail(progress_agent);
                errors.push((index, crate::cli_error("agent loader panicked")));
            }
        }

        errors.sort_by_key(|(index, _)| *index);
        if let Some((_, error)) = errors.into_iter().next() {
            return Err(error);
        }

        loaded.sort_by_key(|loaded| loaded.index);
        Ok(loaded)
    })
}

fn leak_agent_name(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

struct ResolvedNamedPiStore {
    name: String,
    paths: Vec<PathBuf>,
}

fn resolve_named_pi_store_paths(
    stores: &[NamedPiStore],
    default_pi_path: Option<&str>,
) -> Result<Vec<ResolvedNamedPiStore>> {
    let mut owners = BTreeMap::<PathBuf, String>::new();
    for path in pi::default_paths(default_pi_path)? {
        owners.insert(path_key(&path), "default pi store".to_string());
    }

    let mut resolved = Vec::with_capacity(stores.len());
    for store in stores {
        let mut paths = Vec::new();
        let mut collision_owners = BTreeSet::new();
        for path in pi::named_store_paths(&store.path)? {
            let key = path_key(&path);
            if let Some(owner) = owners
                .iter()
                .find(|(owned, _)| paths_overlap(&key, owned))
                .map(|(_, owner)| owner.clone())
            {
                collision_owners.insert(owner);
                continue;
            }
            owners.insert(key, format!("pi.stores name '{}'", store.name));
            paths.push(path);
        }

        if !collision_owners.is_empty() {
            return Err(crate::cli_error(format!(
                "Invalid ccusage config: pi.stores name '{}' paths overlap {}",
                store.name,
                collision_owners.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }

        resolved.push(ResolvedNamedPiStore {
            name: store.name.clone(),
            paths,
        });
    }
    Ok(resolved)
}

fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Session files are collected recursively, so a store rooted at an ancestor
/// of another store (or of the default pi store) would ingest the same files
/// twice under different dedupe identities. Treat equal and nested paths as
/// overlapping.
fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

/// Asks a source that implements the contract for its unified rows.
fn load_source_result(
    source: &dyn UsageSource,
    load_kind: AgentReportKind,
    shared: &SharedArgs,
    source_args: Option<&SourceArgs>,
    pricing: Option<&PricingMap>,
    claude_counted: Option<&(dyn Fn() -> ClaudeCounted + Send)>,
) -> Result<SourceRows> {
    source.unified_rows(&UnifiedRequest {
        kind: load_kind,
        shared,
        options: SourceOptions::from_source(source_args),
        pricing,
        progress: ProgressOwner::Caller,
        multi_source_driver: true,
        claude_counted,
    })
}

fn agent_rows(rows: SourceRows) -> AgentRows {
    AgentRows {
        rows: rows.rows,
        detected: rows.detected,
        diagnostics: rows.diagnostics,
    }
}

#[cfg(test)]
fn load_pi_format_agent_rows(
    custom_path: Option<&str>,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    pi::SOURCE
        .unified_rows(&UnifiedRequest {
            kind,
            shared,
            options: SourceOptions {
                pi_path: custom_path,
                ..SourceOptions::default()
            },
            pricing: Some(pricing),
            progress: ProgressOwner::Caller,
            multi_source_driver: true,
            claude_counted: None,
        })
        .map(agent_rows)
}

#[cfg(test)]
fn load_named_pi_store_rows(
    agent: &'static str,
    store_path: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let store = pi::NamedStore::new(agent, pi::named_store_paths(store_path)?);
    load_source_result(&store, kind, shared, None, Some(pricing), None).map(agent_rows)
}

/// Loads `ccusage opencode` entries. The `claude-agent` turns the Claude
/// source counts are left to it unless `include_claude_agent` is set, so the
/// focused reports add up to the unified one.
///
/// Also returns what the Claude load skipped, if it ran: a skipped transcript
/// leaves its turns to OpenCode, so the skip changes these totals too.
pub(super) fn load_opencode_entries(
    shared: &SharedArgs,
    kind: AgentReportKind,
    include_claude_agent: bool,
) -> Result<(Vec<LoadedEntry>, Option<Diagnostics>)> {
    let entries = opencode::load_entries_with_native_sessions(shared, kind)?;
    if include_claude_agent || entries.iter().all(|(_, session)| session.is_none()) {
        return Ok((entries.into_iter().map(|(entry, _)| entry).collect(), None));
    }
    let claude_shared = SharedArgs {
        json: true,
        mode: CostMode::Display,
        ..shared.clone()
    };
    // Focused Claude reports, sessions included, count only in-window days, so match by
    // session day. Missing Claude directories load as no session days, so those turns stay.
    let claude_rows = claude::SOURCE.unified_rows(&UnifiedRequest {
        kind: AgentReportKind::Daily,
        shared: &claude_shared,
        options: SourceOptions::default(),
        pricing: None,
        progress: ProgressOwner::Source,
        multi_source_driver: true,
        claude_counted: None,
    })?;
    let claude_counted = claude_rows
        .claude_counted
        .ok_or_else(|| crate::cli_error("Claude source did not return counted usage"))?;
    Ok((
        claude_counted.retain_uncounted(entries),
        Some(claude_rows.diagnostics),
    ))
}

/// Retains the daily summaries inside the requested date bounds.
#[cfg(test)]
fn filter_daily_summaries_by_date(rows: &mut Vec<UsageSummary>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    rows.retain(|row| {
        let date = row.date.as_deref().unwrap_or_default().replace('-', "");
        shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
    });
}

fn needs_daily_family(kinds: &[AgentReportKind]) -> bool {
    kinds.iter().any(|kind| *kind != AgentReportKind::Session)
}

fn needs_session(kinds: &[AgentReportKind]) -> bool {
    kinds.contains(&AgentReportKind::Session)
}

#[cfg(test)]
pub(super) fn codex_group_row<S>(
    period: &str,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: S,
    mode: CostMode,
) -> UnifiedRow
where
    S: Into<codex::CodexSpeedPolicy> + Copy,
{
    codex::unified_row_from_group(period, group, pricing, speed, mode)
}

pub(super) fn aggregate_rows(
    rows: Vec<UnifiedRow>,
    kind: AgentReportKind,
    start_of_week: WeekDay,
) -> Vec<UnifiedRow> {
    let mut groups = BTreeMap::<String, AllAccumulator>::new();
    for mut row in rows {
        let period = match kind {
            AgentReportKind::Daily => row.period.clone(),
            AgentReportKind::Monthly => row
                .period
                .get(..7)
                .map_or_else(|| row.period.clone(), str::to_string),
            AgentReportKind::Weekly => {
                crate::week_start(&row.period, start_of_week).unwrap_or_else(|| row.period.clone())
            }
            AgentReportKind::Session => row.period.clone(),
        };
        row.period = period.clone();
        groups.entry(period).or_default().add(row);
    }
    groups
        .into_iter()
        .map(|(period, group)| group.into_row(period))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_cli::NamedPiStore;
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    fn usage_summary(date: &str, input_tokens: u64) -> UsageSummary {
        UsageSummary {
            date: Some(date.to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: Vec::new(),
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }

    fn sorting_row(period: &str, agent: &'static str, cost: f64) -> UnifiedRow {
        let mut summary = usage_summary(period, 10);
        summary.total_cost = cost;
        summary_rows(agent, vec![summary], false).pop().unwrap()
    }

    #[test]
    fn pricing_maps_exclude_sources_that_load_their_own_pricing() {
        let shared = SharedArgs {
            offline: true,
            ..SharedArgs::default()
        };
        let source_shared = shared_by_source(&BTreeMap::new(), &shared);

        let (pricing_by_source, unified_pricing) = load_pricing(&shared, &source_shared, false);

        for agent in ["claude", "opencode", "qwen"] {
            assert!(
                !pricing_by_source.contains_key(agent),
                "{agent} has its own pricing map"
            );
        }
        for agent in BUILT_IN_AGENT_NAMES {
            if !["claude", "opencode", "qwen"].contains(agent) {
                assert!(
                    pricing_by_source.contains_key(agent),
                    "{agent} needs a pricing map"
                );
            }
        }
        assert!(unified_pricing.is_none());
    }

    #[test]
    fn source_claude_project_filters_daily_and_session_rows_without_filtering_other_sources() {
        let fixture = fs_fixture!({
            "claude-config/projects/project-a/session-a.jsonl": r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-a","costUSD":0.01,"message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-a"}}"#,
            "claude-config/projects/project-b/session-b.jsonl": r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-b","costUSD":0.02,"message":{"usage":{"input_tokens":1000,"output_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-b"}}"#,
            "pi/sessions/project-pi/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":7,"output":3}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            json: true,
            mode: CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: shared.clone(),
                claude_project: Some("project-a".to_string()),
                ..SourceArgs::default()
            },
        )]);

        let daily = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(daily.rows.len(), 1);
        assert_eq!(daily.rows[0].period, "2099-01-02");
        assert_eq!(daily.rows[0].metadata_agents, Some(vec!["claude", "pi"]));
        assert_eq!(daily.rows[0].input_tokens, 17);
        assert_eq!(daily.rows[0].output_tokens, 8);
        assert!((daily.rows[0].total_cost - 0.01).abs() < f64::EPSILON);

        let sessions =
            load_rows(AgentReportKind::Session, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(
            sessions
                .rows
                .iter()
                .any(|row| row.agent == "claude" && row.period == "session-a")
        );
        assert!(
            !sessions
                .rows
                .iter()
                .any(|row| row.agent == "claude" && row.period == "session-b")
        );
        assert!(
            sessions
                .rows
                .iter()
                .any(|row| row.agent == "pi" && row.period == "pi-session")
        );
    }

    fn isolated_source_environment(fixture: &ccusage_test_support::Fixture) -> EnvVarsGuard {
        isolated_source_environment_with_pi_agent_dir(fixture, "pi/sessions")
    }

    fn isolated_source_environment_with_pi_agent_dir(
        fixture: &ccusage_test_support::Fixture,
        pi_agent_dir: &str,
    ) -> EnvVarsGuard {
        let _ = fixture.create_dir_all("claude-config/projects");
        let _ = fixture.create_dir_all("pi/sessions");
        let _ = fixture.create_dir_all("empty-xdg-config");
        let _ = fixture.create_dir_all("empty-xdg-data");
        EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (
                "USERPROFILE",
                Some(fixture.path("empty-userprofile").into_os_string()),
            ),
            (
                "CLAUDE_CONFIG_DIR",
                Some(fixture.path("claude-config").into_os_string()),
            ),
            (
                "CODEX_HOME",
                Some(fixture.path("codex-home").into_os_string()),
            ),
            ("OPENCODE_DATA_DIR", None),
            ("AMP_DATA_DIR", None),
            ("DROID_SESSIONS_DIR", None),
            ("CODEBUFF_DATA_DIR", None),
            ("HERMES_HOME", None),
            (
                "PI_AGENT_DIR",
                Some(fixture.path(pi_agent_dir).into_os_string()),
            ),
            ("GOOSE_PATH_ROOT", None),
            (
                "OPENCLAW_DIR",
                Some(fixture.path("openclaw-home").into_os_string()),
            ),
            ("KILO_DATA_DIR", None),
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", None),
            ("COPILOT_HOME", None),
            ("GEMINI_DATA_DIR", None),
            ("KIMI_DATA_DIR", None),
            ("QWEN_DATA_DIR", None),
            ("GROK_HOME", None),
            ("ZCODE_HOME", None),
            ("ANTIGRAVITY_DATA_DIR", None),
            ("CLINE_DATA_DIR", None),
            ("APPDATA", None),
            (
                "XDG_CONFIG_HOME",
                Some(fixture.path("empty-xdg-config").into_os_string()),
            ),
            (
                "XDG_DATA_HOME",
                Some(fixture.path("empty-xdg-data").into_os_string()),
            ),
        ])
    }

    fn codex_rollout(date: &str, model: &str, input_tokens: u64, output_tokens: u64) -> String {
        [
            serde_json::json!({"type":"turn_context","payload":{"model":model}}).to_string(),
            serde_json::json!({
                "timestamp": format!("{date}T00:00:00.000Z"),
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": {"service_tier": "default"},
                },
            })
            .to_string(),
            serde_json::json!({
                "timestamp": format!("{date}T00:00:01.000Z"),
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": model,
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "output_tokens": output_tokens,
                            "total_tokens": input_tokens + output_tokens,
                        },
                        "total_token_usage": {
                            "input_tokens": input_tokens,
                            "output_tokens": output_tokens,
                            "total_tokens": input_tokens + output_tokens,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn session_cost_order_honors_explicit_ascending_order() {
        let shared = SharedArgs {
            order: crate::cli::SortOrder::Asc,
            order_explicit: true,
            ..SharedArgs::default()
        };
        let rows = vec![
            sorting_row("a-expensive", "claude", 20.0),
            sorting_row("z-cheap", "codex", 1.0),
            sorting_row("z-cheap", "claude", 1.0),
            sorting_row("b-zero", "claude", 0.0),
        ];
        let sorted = finish_rows(AgentReportKind::Session, rows, &shared, WeekDay::Monday);
        let keys: Vec<_> = sorted
            .iter()
            .map(|row| (row.period.as_str(), row.agent))
            .collect();
        assert_eq!(
            keys,
            [
                ("b-zero", "claude"),
                ("z-cheap", "claude"),
                ("z-cheap", "codex"),
                ("a-expensive", "claude")
            ]
        );
    }

    #[test]
    fn session_cost_order_uses_id_then_agent_for_ties() {
        for (order, order_explicit) in [
            (crate::cli::SortOrder::Asc, false),
            (crate::cli::SortOrder::Desc, true),
        ] {
            let rows = vec![
                sorting_row("b-tie", "codex", 20.0),
                sorting_row("a-zero", "claude", 0.0),
                sorting_row("z-tie", "claude", 20.0),
                sorting_row("b-tie", "claude", 20.0),
                sorting_row("c-cheap", "codex", 1.0),
            ];
            let shared = SharedArgs {
                order,
                order_explicit,
                ..SharedArgs::default()
            };
            let sorted = finish_rows(AgentReportKind::Session, rows, &shared, WeekDay::Monday);
            let keys: Vec<_> = sorted
                .iter()
                .map(|row| (row.period.as_str(), row.agent))
                .collect();
            assert_eq!(
                keys,
                [
                    ("b-tie", "claude"),
                    ("b-tie", "codex"),
                    ("z-tie", "claude"),
                    ("c-cheap", "codex"),
                    ("a-zero", "claude"),
                ]
            );
        }
    }

    #[test]
    fn session_cost_order_handles_empty_and_single_row_reports() {
        let shared = SharedArgs::default();
        assert!(
            finish_rows(
                AgentReportKind::Session,
                Vec::new(),
                &shared,
                WeekDay::Monday
            )
            .is_empty()
        );
        let rows = finish_rows(
            AgentReportKind::Session,
            vec![sorting_row("only-session", "claude", 0.0)],
            &shared,
            WeekDay::Monday,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].period, "only-session");
    }

    #[test]
    fn period_reports_keep_chronological_order_regardless_of_cost() {
        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
        ] {
            for (order, expected_costs) in [
                (crate::cli::SortOrder::Asc, [1.0, 20.0, 3.0]),
                (crate::cli::SortOrder::Desc, [3.0, 20.0, 1.0]),
            ] {
                let rows = vec![
                    sorting_row("2026-03-02", "claude", 3.0),
                    sorting_row("2026-01-05", "claude", 1.0),
                    sorting_row("2026-02-02", "claude", 20.0),
                ];
                let shared = SharedArgs {
                    order,
                    ..SharedArgs::default()
                };
                let sorted = finish_rows(kind, rows, &shared, WeekDay::Monday);
                let costs: Vec<_> = sorted.iter().map(|row| row.total_cost).collect();
                assert_eq!(costs, expected_costs, "{kind:?} {order:?}");
            }
        }
    }

    #[test]
    fn filters_daily_summaries_with_compact_date_bounds() {
        let mut rows = vec![
            usage_summary("2026-01-01", 10),
            usage_summary("2026-01-02", 20),
            usage_summary("2026-01-03", 30),
        ];
        let shared = SharedArgs {
            since: Some("20260102".to_string()),
            until: Some("20260102".to_string()),
            ..SharedArgs::default()
        };

        filter_daily_summaries_by_date(&mut rows, &shared);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date.as_deref(), Some("2026-01-02"));
        assert_eq!(rows[0].input_tokens, 20);
    }

    #[test]
    fn unified_row_matches_focused_cost_for_mixed_recorded_tiers() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
        );
        let usage = crate::CodexModelUsage {
            input_tokens: 30,
            total_tokens: 30,
            recorded_standard_usage: codex::CodexUsageBucket {
                input_tokens: 10,
                ..codex::CodexUsageBucket::default()
            },
            recorded_fast_usage: codex::CodexUsageBucket {
                input_tokens: 10,
                ..codex::CodexUsageBucket::default()
            },
            ..crate::CodexModelUsage::default()
        };
        let mut group = CodexGroup {
            input_tokens: 30,
            total_tokens: 30,
            ..CodexGroup::default()
        };
        group.models.insert("gpt-test".to_string(), usage);
        let speed = codex::CodexSpeedPolicy::Auto(codex::CodexServiceTier::Standard);

        let focused_cost = codex::calculate_group_cost(&group, &pricing, speed);
        let unified = codex_group_row("2026-07-22", &group, &pricing, speed, CostMode::Calculate);

        assert!((focused_cost - 40e-6).abs() < f64::EPSILON);
        assert!((unified.total_cost - focused_cost).abs() < f64::EPSILON);
        assert!((unified.model_breakdowns[0].cost - focused_cost).abs() < f64::EPSILON);
    }

    #[test]
    fn unified_row_reports_codex_cache_write_tokens_and_cost() {
        let pricing = PricingMap::load_embedded();
        let usage = crate::CodexModelUsage {
            input_tokens: 935_040,
            cached_input_tokens: 875_306,
            cache_creation_tokens: 57_610,
            output_tokens: 11_150,
            total_tokens: 946_190,
            long_context_input_tokens: 935_040,
            long_context_cached_input_tokens: 875_306,
            long_context_cache_creation_tokens: 57_610,
            long_context_output_tokens: 11_150,
            ..crate::CodexModelUsage::default()
        };
        let mut group = CodexGroup {
            input_tokens: 935_040,
            cached_input_tokens: 875_306,
            cache_creation_tokens: 57_610,
            output_tokens: 11_150,
            total_tokens: 946_190,
            ..CodexGroup::default()
        };
        group.models.insert("gpt-5.6-terra".to_string(), usage);

        let row = codex_group_row(
            "2026-08-20",
            &group,
            &pricing,
            codex::CodexSpeedPolicy::Forced(codex::CodexServiceTier::Standard),
            CostMode::Calculate,
        );
        let expected_cost =
            2_124.0 * 4e-6 + 875_306.0 * 0.4e-6 + 57_610.0 * 5e-6 + 11_150.0 * 18e-6;

        assert_eq!(row.input_tokens, 2_124);
        assert_eq!(row.cache_creation_tokens, 57_610);
        assert_eq!(row.cache_read_tokens, 875_306);
        assert!((row.total_cost - expected_cost).abs() < 1e-12);
        assert_eq!(row.model_breakdowns[0].input_tokens, 2_124);
        assert_eq!(row.model_breakdowns[0].cache_creation_tokens, 57_610);
        assert_eq!(row.model_breakdowns[0].cache_read_tokens, 875_306);
        assert!((row.model_breakdowns[0].cost - expected_cost).abs() < 1e-12);
    }

    fn pi_path_subcommand_rows(
        store_path: &str,
        kind: AgentReportKind,
        shared: &SharedArgs,
        pricing: &PricingMap,
    ) -> Result<Vec<UnifiedRow>> {
        let mut entries = pi::load_entries(shared, Some(store_path), Some(pricing))?;
        filter_loaded_entries_by_date(&mut entries, shared);
        let summaries = pi::summarize_entries(&entries, kind)?;
        Ok(summary_rows("pi", summaries, true))
    }

    fn usage_fingerprint(rows: &[UnifiedRow]) -> Vec<(String, u64, u64, u64, u64, u64)> {
        rows.iter()
            .map(|row| {
                (
                    row.period.clone(),
                    row.input_tokens,
                    row.output_tokens,
                    row.cache_creation_tokens,
                    row.cache_read_tokens,
                    row.total_tokens,
                )
            })
            .collect()
    }

    #[test]
    fn load_rows_filters_named_pi_store_sessions_like_pi_path_subcommand() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_inside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "omp/sessions/project-a/agent_outside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
        });
        let _environment = isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default");
        let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260704".to_string()),
            until: Some("20260704".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: store_path.clone(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let omp_rows = result
            .rows
            .iter()
            .filter(|row| row.agent == "omp")
            .collect::<Vec<_>>();
        let pi_rows = pi_path_subcommand_rows(
            &store_path,
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(omp_rows.len(), 1);
        assert_eq!(omp_rows[0].period, "inside-session");
        assert_eq!(omp_rows[0].input_tokens, 10);
        assert_eq!(omp_rows[0].output_tokens, 1);
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("inside-session".to_string(), 10, 1, 0, 0, 11)]
        );
    }

    #[test]
    fn named_pi_store_returns_sanitized_diagnostics_with_healthy_usage() {
        let fixture = fs_fixture!({
            "store/project-a/agent_session-a.jsonl": [
                r#"{"type":"session","id":"session-a","timestamp":"2026-01-01T00:00:00.000Z"}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
                r#"{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            ]
            .join("\n"),
        });
        let shared = SharedArgs {
            mode: CostMode::Display,
            debug: true,
            offline: true,
            ..SharedArgs::default()
        };

        let rows = load_named_pi_store_rows(
            "omp",
            fixture.path("store").to_str().unwrap(),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert!(rows.detected);
        assert_eq!(rows.rows.len(), 1);
        assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
            warning.contains("invalid data")
                && warning.starts_with("omp:")
                && warning.contains("agent_session-a.jsonl:2")
                && !warning.contains("SECRET_CANARY")
        }));
    }

    #[test]
    fn named_pi_store_windowed_period_rows_match_pi_path_subcommand() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_july-third.jsonl": r#"{"type":"message","timestamp":"2026-07-03T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "omp/sessions/project-a/agent_july-fourth.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
            "omp/sessions/project-a/agent_july-fifth.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":3}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260703".to_string()),
            until: Some("20260704".to_string()),
            ..SharedArgs::default()
        };
        let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
        let pricing = PricingMap::default();
        let cases = [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
        ];

        for kind in cases {
            let named_rows = load_named_pi_store_rows("omp", &store_path, kind, &shared, &pricing)
                .unwrap()
                .rows;
            let pi_rows = pi_path_subcommand_rows(&store_path, kind, &shared, &pricing).unwrap();

            assert_eq!(usage_fingerprint(&named_rows), usage_fingerprint(&pi_rows));
        }
    }

    #[test]
    fn default_pi_unified_session_window_keeps_until_day_like_pi_path() {
        let fixture = fs_fixture!({
            "pi/sessions/project-a/agent_until-day.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "pi/sessions/project-a/agent_after-day.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
        });
        let _environment = isolated_source_environment_with_pi_agent_dir(&fixture, "pi/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260704".to_string()),
            until: Some("20260704".to_string()),
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let pi_rows = result
            .rows
            .iter()
            .filter(|row| row.agent == "pi")
            .cloned()
            .collect::<Vec<_>>();
        let pi_path_rows = pi_path_subcommand_rows(
            fixture.path("pi/sessions").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(
            usage_fingerprint(&pi_rows),
            usage_fingerprint(&pi_path_rows)
        );
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("until-day".to_string(), 10, 1, 0, 0, 11)]
        );
    }

    #[test]
    fn unified_claude_session_window_keeps_sessions_last_active_on_the_until_day() {
        let fixture = fs_fixture!({
            "claude-config/projects/project-a/until-day.jsonl": r#"{"timestamp":"2026-07-05T03:00:00.000Z","sessionId":"until-day","costUSD":0.01,"message":{"usage":{"input_tokens":10,"output_tokens":1},"model":"claude-sonnet-4-20250514","id":"msg-until"}}"#,
            "claude-config/projects/project-a/after-day.jsonl": r#"{"timestamp":"2026-07-06T03:00:00.000Z","sessionId":"after-day","costUSD":0.02,"message":{"usage":{"input_tokens":20,"output_tokens":2},"model":"claude-sonnet-4-20250514","id":"msg-after"}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            json: true,
            mode: CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260704".to_string()),
            until: Some("20260704".to_string()),
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let claude_rows = result
            .rows
            .iter()
            .filter(|row| row.agent == "claude")
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            usage_fingerprint(&claude_rows),
            vec![("until-day".to_string(), 10, 1, 0, 0, 11)]
        );
    }

    #[test]
    fn default_pi_session_rows_keep_main_project_path_derivation() {
        let fixture = fs_fixture!({
            "default-root/archive/sessions/project-a/agent_default-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_pi_format_agent_rows(
            Some(fixture.path("default-root").to_str().unwrap()),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0].agent, "pi");
        assert_eq!(
            rows.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-a")
        );
    }

    #[test]
    fn named_pi_store_rows_keep_agent_model_and_project_metadata() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_named_pi_store_rows(
            "omp",
            fixture.path("omp/sessions").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert!(rows.detected);
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0].agent, "omp");
        assert_eq!(rows.rows[0].models_used, vec!["[omp] gpt-5"]);
        assert_eq!(
            rows.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-a")
        );
    }

    #[test]
    fn named_pi_store_sessions_split_by_project_like_the_default_store() {
        let session = |input: u64| {
            [
                r#"{"type":"session","id":"same-session","timestamp":"2026-01-01T00:00:00.000Z"}"#.to_string(),
                format!(
                    r#"{{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{{"role":"assistant","model":"gpt-5","usage":{{"input":{input},"output":1}}}}}}"#
                ),
            ]
            .join("\n")
        };
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_same-session.jsonl": session(10),
            "omp/sessions/project-b/agent_same-session.jsonl": session(20),
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };
        let store_path = fixture.path("omp/sessions");

        let named = load_named_pi_store_rows(
            "omp",
            store_path.to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();
        let default = load_pi_format_agent_rows(
            store_path.to_str(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        let sessions = |rows: &AgentRows| {
            rows.rows
                .iter()
                .map(|row| {
                    (
                        row.metadata.as_ref().unwrap()["projectPath"].clone(),
                        row.input_tokens,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sessions(&named),
            [(json!("project-a"), 10), (json!("project-b"), 20)]
        );
        assert_eq!(sessions(&named), sessions(&default));
    }

    #[test]
    fn named_pi_store_rows_derive_project_relative_to_store_root() {
        let fixture = fs_fixture!({
            "store/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
            "backups/sessions/omp-copy/project-c/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-03T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let plain_root = load_named_pi_store_rows(
            "omp",
            fixture.path("store").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();
        let earlier_sessions_root = load_named_pi_store_rows(
            "omp",
            fixture.path("backups/sessions/omp-copy").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(
            plain_root.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-b")
        );
        assert_eq!(
            earlier_sessions_root.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-c")
        );
    }

    #[test]
    fn named_pi_store_absent_path_is_clean_empty() {
        let fixture = fs_fixture!({});
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_named_pi_store_rows(
            "omp",
            fixture.path("missing/sessions").to_str().unwrap(),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert!(!rows.detected);
        assert!(rows.rows.is_empty());
    }

    #[test]
    fn load_rows_rejects_named_pi_store_that_collides_with_default_pi_path() {
        let fixture = fs_fixture!({
            ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _environment =
            isolated_source_environment_with_pi_agent_dir(&fixture, ".pi/agent/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_that_collides_with_another_store() {
        let fixture = fs_fixture!({
            "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _environment = isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default");
        let path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path,
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_nested_inside_default_pi_path() {
        let fixture = fs_fixture!({
            ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _environment =
            isolated_source_environment_with_pi_agent_dir(&fixture, ".pi/agent/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions/project-a")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected nested default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_partially_overlapping_another_store() {
        let fixture = fs_fixture!({
            "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _environment = isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default");
        let shared_path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let distinct = fixture.create_dir_all("fork-only/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: shared_path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path: format!("{}, {}", distinct.display(), shared_path),
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected partial named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    }

    #[test]
    fn load_rows_includes_named_pi_store_through_production_wiring() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let _environment = isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture.path("omp/sessions").to_string_lossy().into_owned(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        assert!(result.detected_agents.contains(&"omp"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].metadata_agents, Some(vec!["omp"]));
        assert_eq!(result.rows[0].models_used, vec!["[omp] gpt-5"]);
        assert_eq!(result.rows[0].input_tokens, 30);
        assert_eq!(result.rows[0].output_tokens, 40);
    }

    #[test]
    fn load_rows_reads_the_default_pi_store_from_its_source_pi_path() {
        let fixture = fs_fixture!({
            "custom/pi/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: shared.clone(),
                pi_path: Some(fixture.path("custom/pi").to_string_lossy().into_owned()),
                ..SourceArgs::default()
            },
        )]);

        let unified = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(!unified.detected_agents.contains(&"pi"));
        assert!(result.detected_agents.contains(&"pi"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].metadata_agents, Some(vec!["pi"]));
        assert_eq!(result.rows[0].input_tokens, 30);
        assert_eq!(result.rows[0].output_tokens, 40);
    }

    #[test]
    fn load_rows_filters_a_source_by_its_own_date_window() {
        let fixture = fs_fixture!({
            "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990101".to_string()),
            until: Some("20990103".to_string()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990105".to_string()),
                    until: Some("20990106".to_string()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        let unified = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(unified.rows.len(), 1);
        assert!(result.rows.is_empty());
        assert!(result.detected_agents.contains(&"pi"));
    }

    #[test]
    fn loads_codex_and_pi_with_source_pricing_speed_and_date_options() {
        let fixture = fs_fixture!({
            "codex-home/sessions/2026/07/21/before.jsonl": codex_rollout(
                "2026-07-21",
                "gpt-source-config",
                900,
                100,
            ),
            "codex-home/sessions/2026/07/22/in-range.jsonl": codex_rollout(
                "2026-07-22",
                "gpt-source-config",
                1000,
                500,
            ),
            "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2026-07-22T00:00:00.000Z","message":{"role":"assistant","model":"gpt-source-config","usage":{"input":1000,"output":500}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20260720".to_string()),
            until: Some("20260723".to_string()),
            ..SharedArgs::default()
        };
        let pricing_overrides = BTreeMap::from([(
            "gpt-source-config".to_string(),
            crate::cli::PricingOverride {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                fast_multiplier: Some(3.0),
                ..Default::default()
            },
        )]);
        let pi_pricing_overrides = BTreeMap::from([(
            "gpt-source-config".to_string(),
            crate::cli::PricingOverride {
                input_cost_per_token: Some(5e-6),
                output_cost_per_token: Some(10e-6),
                ..Default::default()
            },
        )]);
        let sources = BTreeMap::from([
            (
                "codex".to_string(),
                SourceArgs {
                    shared: SharedArgs {
                        since: Some("20260722".to_string()),
                        until: Some("20260722".to_string()),
                        pricing_overrides,
                        ..shared.clone()
                    },
                    codex_speed: CodexSpeed::Fast,
                    ..SourceArgs::default()
                },
            ),
            (
                "pi".to_string(),
                SourceArgs {
                    shared: SharedArgs {
                        pricing_overrides: pi_pricing_overrides,
                        ..shared.clone()
                    },
                    ..SourceArgs::default()
                },
            ),
        ]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].metadata_agents, Some(vec!["codex", "pi"]));
        let breakdowns = result.rows[0].agent_breakdowns.as_ref().unwrap();
        let codex_row = breakdowns
            .iter()
            .find(|row| row.agent == "codex")
            .expect("Codex fixture should produce a row");
        assert_eq!(codex_row.period, "2026-07-22");
        assert_eq!(codex_row.input_tokens, 1000);
        assert_eq!(codex_row.output_tokens, 500);
        assert!((codex_row.total_cost - 0.006).abs() < f64::EPSILON);
        let pi_row = breakdowns
            .iter()
            .find(|row| row.agent == "pi")
            .expect("Pi fixture should produce a row");
        assert!((pi_row.total_cost - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn loads_codex_with_source_pricing_speed_without_date_bounds() {
        let fixture = fs_fixture!({
            "codex-home/sessions/2026/07/22/in-range.jsonl": codex_rollout(
                "2026-07-22",
                "gpt-source-config",
                1000,
                500,
            ),
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "codex".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    pricing_overrides: BTreeMap::from([(
                        "gpt-source-config".to_string(),
                        crate::cli::PricingOverride {
                            input_cost_per_token: Some(1e-6),
                            output_cost_per_token: Some(2e-6),
                            fast_multiplier: Some(3.0),
                            ..Default::default()
                        },
                    )]),
                    ..shared.clone()
                },
                codex_speed: CodexSpeed::Fast,
                ..SourceArgs::default()
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].input_tokens, 1000);
        assert_eq!(result.rows[0].output_tokens, 500);
        assert!((result.rows[0].total_cost - 0.006).abs() < f64::EPSILON);
    }

    #[test]
    fn reads_openclaw_entries_from_the_source_custom_path() {
        let fixture = fs_fixture!({
            "custom-openclaw/agents/main/sessions/custom.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1660,"output":55,"totalTokens":1715,"cost":{"total":0.02}},"timestamp":1769753935279}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let shared = SharedArgs {
            mode: CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "openclaw".to_string(),
            SourceArgs {
                shared: shared.clone(),
                open_claw_path: Some(
                    fixture
                        .path("custom-openclaw")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..SourceArgs::default()
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(result.detected_agents.contains(&"openclaw"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].input_tokens, 1660);
        assert_eq!(result.rows[0].output_tokens, 55);
        assert!((result.rows[0].total_cost - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_named_pi_store_overlapping_the_source_custom_path() {
        let fixture = fs_fixture!({
            "custom/pi/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let custom_pi_path = fixture.path("custom/pi").to_string_lossy().into_owned();
        let shared = SharedArgs {
            offline: true,
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "fork".to_string(),
                path: custom_pi_path.clone(),
            }],
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: shared.clone(),
                pi_path: Some(custom_pi_path),
                ..SourceArgs::default()
            },
        )]);

        let Err(error) = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday)
        else {
            panic!("expected named pi store to collide with the configured pi path");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("default pi store"));
    }

    #[test]
    fn named_pi_stores_keep_unified_shared_and_pricing_options() {
        let fixture = fs_fixture!({
            "fork/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-named-source-config","usage":{"input":1000,"output":500}}}"#,
        });
        let _environment = isolated_source_environment(&fixture);
        let pricing_key = "[fork] gpt-named-source-config".to_string();
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "fork".to_string(),
                path: fixture.path("fork/sessions").to_string_lossy().into_owned(),
            }],
            pricing_overrides: BTreeMap::from([(
                pricing_key.clone(),
                crate::cli::PricingOverride {
                    input_cost_per_token: Some(1e-6),
                    output_cost_per_token: Some(2e-6),
                    ..Default::default()
                },
            )]),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990105".to_string()),
                    until: Some("20990106".to_string()),
                    pricing_overrides: BTreeMap::from([(
                        pricing_key,
                        crate::cli::PricingOverride {
                            input_cost_per_token: Some(10e-6),
                            output_cost_per_token: Some(20e-6),
                            ..Default::default()
                        },
                    )]),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(result.detected_agents.contains(&"fork"));
        let named_store = result.rows[0]
            .agent_breakdowns
            .as_ref()
            .unwrap()
            .iter()
            .find(|row| row.agent == "fork")
            .expect("named store should use the unified date window");
        assert_eq!(named_store.input_tokens, 1000);
        assert_eq!(named_store.output_tokens, 500);
        assert!((named_store.total_cost - 0.002).abs() < f64::EPSILON);
    }

    #[test]
    fn named_pi_store_rows_are_additive_to_default_pi_rows() {
        let fixture = fs_fixture!({
            "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
            "omp/sessions/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };
        let mut rows = load_pi_format_agent_rows(
            Some(fixture.path("pi/sessions").to_str().unwrap()),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap()
        .rows;
        rows.extend(
            load_named_pi_store_rows(
                "omp",
                fixture.path("omp/sessions").to_str().unwrap(),
                AgentReportKind::Daily,
                &shared,
                &PricingMap::default(),
            )
            .unwrap()
            .rows,
        );

        let aggregated = aggregate_rows(rows, AgentReportKind::Daily, WeekDay::Monday);

        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].metadata_agents, Some(vec!["omp", "pi"]));
        let breakdowns = aggregated[0].agent_breakdowns.as_ref().unwrap();
        assert_eq!(
            breakdowns
                .iter()
                .map(|row| (row.agent, row.models_used.as_slice()))
                .collect::<Vec<_>>(),
            vec![
                ("omp", ["[omp] gpt-5".to_string()].as_slice()),
                ("pi", ["[pi] gpt-5".to_string()].as_slice()),
            ]
        );
    }
}
