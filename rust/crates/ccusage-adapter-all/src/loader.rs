use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

use ccusage_adapter_common::{
    claims,
    rows::SourceRow,
    source::{LoadKind, LoadRequest, ProgressOwner, SourceOptions, UsageSource},
};
use ccusage_core::parallel::spawn_scoped_worker;

use crate::{
    Diagnostics, LoadedEntry, PricingMap, Result, Skip, SkipKind,
    adapter::{claude, opencode, pi},
    cli::{AgentReportKind, NamedPiStore, SharedArgs, SourceArgs, WeekDay},
};

use super::{
    report::sort_rows,
    sources::SOURCES,
    types::{
        AgentLoadSpec, AgentRows, AllAccumulator, AllLoadResult, AllSectionsLoadResult, Bucket,
        DayRow, LoadedAgentRows, ReportDay, ReportRow,
    },
};

pub(super) fn load_rows(
    kind: AgentReportKind,
    shared: &SharedArgs,
    sources: &BTreeMap<String, SourceArgs>,
    start_of_week: WeekDay,
) -> Result<AllLoadResult> {
    let run_sources = run_sources(shared, sources)?;
    let pricing = load_pricing(&run_sources);
    match Bucket::for_report(kind, start_of_week) {
        Some(bucket) => {
            let loaded = load_day_rows(shared, &run_sources, &pricing)?;
            Ok(AllLoadResult {
                rows: finish_days(loaded.rows, bucket, shared),
                detected_agents: loaded.detected_agents,
                diagnostics: loaded.diagnostics,
            })
        }
        None => {
            let loaded = load_session_rows(shared, &run_sources, &pricing)?;
            Ok(AllLoadResult {
                rows: finish_sessions(loaded.rows, shared),
                detected_agents: loaded.detected_agents,
                diagnostics: loaded.diagnostics,
            })
        }
    }
}

pub(super) fn load_sections(
    kinds: &[AgentReportKind],
    shared: &SharedArgs,
    sources: &BTreeMap<String, SourceArgs>,
    start_of_week: WeekDay,
) -> Result<AllSectionsLoadResult> {
    let run_sources = run_sources(shared, sources)?;
    let pricing = load_pricing(&run_sources);
    let mut daily_base = needs_daily_family(kinds)
        .then(|| load_day_rows(shared, &run_sources, &pricing))
        .transpose()?;
    let mut session_base = needs_session(kinds)
        .then(|| load_session_rows(shared, &run_sources, &pricing))
        .transpose()?;

    let daily_detected_agents = daily_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();
    let session_detected_agents = session_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();
    let mut diagnostics = Vec::new();
    let loaded_diagnostics = daily_base
        .iter_mut()
        .map(|base| &mut base.diagnostics)
        .chain(session_base.iter_mut().map(|base| &mut base.diagnostics));
    for base_diagnostics in loaded_diagnostics {
        for loaded in std::mem::take(base_diagnostics) {
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
        let rows = match Bucket::for_report(*kind, start_of_week) {
            Some(bucket) => daily_base
                .as_ref()
                .map(|base| finish_days(base.rows.clone(), bucket, shared)),
            None => session_base
                .as_ref()
                .map(|base| finish_sessions(base.rows.clone(), shared)),
        };
        sections.push((*kind, rows.unwrap_or_default()));
    }

    Ok(AllSectionsLoadResult {
        sections,
        daily_detected_agents,
        session_detected_agents,
        diagnostics,
    })
}

struct CachedPricingMap {
    overrides: BTreeMap<String, crate::cli::PricingOverride>,
    pricing: Arc<PricingMap>,
}

fn cached_pricing_map(
    shared: &SharedArgs,
    base: &Arc<PricingMap>,
    cached: &mut Vec<CachedPricingMap>,
) -> Arc<PricingMap> {
    if let Some(entry) = cached
        .iter()
        .find(|entry| entry.overrides == shared.pricing_overrides)
    {
        return Arc::clone(&entry.pricing);
    }

    let pricing = if shared.pricing_overrides.is_empty() {
        Arc::clone(base)
    } else {
        Arc::new(base.with_user_overrides(shared.pricing_overrides.iter()))
    };
    cached.push(CachedPricingMap {
        overrides: shared.pricing_overrides.clone(),
        pricing: Arc::clone(&pricing),
    });
    pricing
}

enum RunSourceKind {
    Claude,
    OpenCode,
    BuiltIn(&'static dyn UsageSource),
    Named(pi::NamedStore),
}

struct RunSource<'a> {
    source: RunSourceKind,
    shared: &'a SharedArgs,
    options: SourceOptions<'a>,
}

impl RunSource<'_> {
    fn source(&self) -> &dyn UsageSource {
        match &self.source {
            RunSourceKind::Claude => &claude::SOURCE,
            RunSourceKind::OpenCode => &opencode::SOURCE,
            RunSourceKind::BuiltIn(source) => *source,
            RunSourceKind::Named(source) => source,
        }
    }

    fn request<'a>(&'a self, kind: LoadKind, pricing: Option<&'a PricingMap>) -> LoadRequest<'a> {
        LoadRequest {
            kind,
            shared: self.shared,
            options: self.options,
            pricing,
            progress: ProgressOwner::Caller,
        }
    }
}

fn run_sources<'a>(
    shared: &'a SharedArgs,
    sources: &'a BTreeMap<String, SourceArgs>,
) -> Result<Vec<RunSource<'a>>> {
    let mut run_sources = SOURCES
        .iter()
        .map(|source| {
            let args = sources.get(source.id());
            let kind = match source.id() {
                id if id == claude::SOURCE.id() => RunSourceKind::Claude,
                id if id == opencode::SOURCE.id() => RunSourceKind::OpenCode,
                _ => RunSourceKind::BuiltIn(*source),
            };
            RunSource {
                source: kind,
                shared: args.map_or(shared, |args| &args.shared),
                options: SourceOptions::from_source(args),
            }
        })
        .collect::<Vec<_>>();
    let pi_path = sources
        .get("pi")
        .and_then(|source| source.options.pi_path.as_deref());
    for store in resolve_named_pi_store_paths(&shared.pi_stores, pi_path)? {
        let name = Box::leak(store.name.into_boxed_str());
        run_sources.push(RunSource {
            source: RunSourceKind::Named(pi::NamedStore::new(name, store.paths)),
            shared,
            options: SourceOptions::default(),
        });
    }
    Ok(run_sources)
}

fn load_pricing(sources: &[RunSource<'_>]) -> BTreeMap<&'static str, Arc<PricingMap>> {
    let needs_pricing = |source: &&RunSource<'_>| {
        source.source().uses_shared_pricing() && source.shared.mode != crate::cli::CostMode::Display
    };
    let modes = sources
        .iter()
        .filter(needs_pricing)
        .map(|source| source.shared.offline)
        .collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();
    for offline in modes {
        let base = Arc::new(PricingMap::load_with_overrides(
            offline,
            crate::log_level() != Some(0),
            std::iter::empty(),
        ));
        let mut cached = Vec::new();
        for source in sources
            .iter()
            .filter(needs_pricing)
            .filter(|source| source.shared.offline == offline)
        {
            result.insert(
                source.source().id(),
                cached_pricing_map(source.shared, &base, &mut cached),
            );
        }
    }
    result
}

struct BaseRows<R> {
    rows: Vec<R>,
    detected_agents: Vec<&'static str>,
    diagnostics: Vec<Diagnostics>,
}

fn load_day_rows(
    shared: &SharedArgs,
    sources: &[RunSource<'_>],
    pricing: &BTreeMap<&'static str, Arc<PricingMap>>,
) -> Result<BaseRows<DayRow>> {
    load_base_rows(
        LoadKind::Day,
        shared,
        sources,
        pricing,
        |row, diagnostics| match ReportDay::parse(&row.period) {
            Some(day) => Some(DayRow { day, row }),
            None => {
                diagnostics.record(Skip::unit(
                    Path::new(&row.period),
                    SkipKind::InvalidTimestamp,
                    "daily row period is not a calendar day",
                ));
                None
            }
        },
    )
}

fn load_session_rows(
    shared: &SharedArgs,
    sources: &[RunSource<'_>],
    pricing: &BTreeMap<&'static str, Arc<PricingMap>>,
) -> Result<BaseRows<SourceRow>> {
    load_base_rows(LoadKind::Session, shared, sources, pricing, |row, _| {
        Some(row)
    })
}

fn load_base_rows<R>(
    load_kind: LoadKind,
    shared: &SharedArgs,
    sources: &[RunSource<'_>],
    pricing: &BTreeMap<&'static str, Arc<PricingMap>>,
    admit: impl Fn(SourceRow, &Diagnostics) -> Option<R>,
) -> Result<BaseRows<R>> {
    let mut progress = crate::progress::UsageLoadProgress::new(
        crate::log_level() != Some(0)
            && crate::progress::should_show_usage_load_progress(
                shared.json,
                crate::progress::usage_load_output_is_tty(),
            ),
    );
    let specs = load_specs(load_kind, sources, pricing);
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
        rows.extend(
            agent_rows
                .into_iter()
                .filter_map(|row| admit(row, &agent_diagnostics)),
        );
        diagnostics.push(agent_diagnostics);
    }
    Ok(BaseRows {
        rows,
        detected_agents,
        diagnostics,
    })
}

fn load_specs<'a>(
    load_kind: LoadKind,
    sources: &'a [RunSource<'a>],
    pricing: &'a BTreeMap<&'static str, Arc<PricingMap>>,
) -> Vec<AgentLoadSpec<'a>> {
    let runs =
        |wanted: fn(&RunSourceKind) -> bool| sources.iter().any(|source| wanted(&source.source));
    // NOTE(2026-11-01, #2007): OpenCode's claude-agent provider records the same main-thread turns that Claude Code writes to its transcript. Cross-source claims keep matching turns from being counted twice.
    let (mut claims_tx, mut claims_rx) = if runs(|kind| matches!(kind, RunSourceKind::Claude))
        && runs(|kind| matches!(kind, RunSourceKind::OpenCode))
    {
        let (sender, receiver) = claims::channel();
        (Some(sender), Some(receiver))
    } else {
        (None, None)
    };
    let mut specs = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let id = source.source().id();
        let request = source.request(load_kind, pricing.get(id).map(Arc::as_ref));
        let load: Box<dyn FnOnce() -> Result<AgentRows> + Send + '_> = match &source.source {
            RunSourceKind::Claude => match claims_tx.take() {
                Some(sender) => Box::new(move || claude::SOURCE.load_with_claims(&request, sender)),
                None => Box::new(move || claude::SOURCE.load_rows(&request)),
            },
            RunSourceKind::OpenCode => match claims_rx.take() {
                Some(receiver) => {
                    Box::new(move || opencode::SOURCE.load_with_claims(&request, receiver))
                }
                None => Box::new(move || opencode::SOURCE.load_rows(&request)),
            },
            RunSourceKind::BuiltIn(_) | RunSourceKind::Named(_) => {
                Box::new(move || source.source().load_rows(&request))
            }
        };
        specs.push(AgentLoadSpec {
            index,
            agent: id,
            progress_agent: crate::progress::UsageLoadAgent(source.source().label()),
            load,
        });
    }
    specs
}

fn finish_sessions(rows: Vec<SourceRow>, shared: &SharedArgs) -> Vec<ReportRow> {
    let mut rows = rows.into_iter().map(ReportRow::Source).collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        let cost_order = match ccusage_report::session_cost_ordering(shared) {
            crate::cli::SortOrder::Asc => a.total_cost.total_cmp(&b.total_cost),
            crate::cli::SortOrder::Desc => b.total_cost.total_cmp(&a.total_cost),
        };
        cost_order
            .then_with(|| a.period.cmp(&b.period))
            .then_with(|| a.agent().cmp(b.agent()))
    });
    rows
}

fn finish_days(rows: Vec<DayRow>, bucket: Bucket, shared: &SharedArgs) -> Vec<ReportRow> {
    let mut aggregated = aggregate_rows(rows, bucket);
    sort_rows(&mut aggregated, &shared.order.unwrap_or_default());
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
                spawn_scoped_worker(scope, move || {
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

pub(super) fn load_opencode_entries(
    shared: &SharedArgs,
    kind: AgentReportKind,
    include_claude_agent: bool,
    claude_source: Option<&ccusage_cli::SourceArgs>,
) -> Result<(Vec<LoadedEntry>, Option<Diagnostics>)> {
    let entries = opencode::load_entries_with_native_sessions(shared, kind)?;
    if include_claude_agent || entries.iter().all(|(_, session)| session.is_none()) {
        return Ok((entries.into_iter().map(|(entry, _)| entry).collect(), None));
    }
    let claude_shared = claude_source.map_or(shared, |source| &source.shared);
    let diagnostics = Diagnostics::for_report("claude", claude_shared.clock);
    let project = claude_source.and_then(|source| source.options.claude_project.as_deref());
    let counted = claude::SOURCE.counted(claude_shared, project, &diagnostics)?;
    Ok((counted.retain_uncounted(entries), Some(diagnostics)))
}

fn needs_daily_family(kinds: &[AgentReportKind]) -> bool {
    kinds
        .iter()
        .any(|&kind| LoadKind::for_report(kind) == LoadKind::Day)
}

fn needs_session(kinds: &[AgentReportKind]) -> bool {
    kinds
        .iter()
        .any(|&kind| LoadKind::for_report(kind) == LoadKind::Session)
}

pub(super) fn aggregate_rows(rows: Vec<DayRow>, bucket: Bucket) -> Vec<ReportRow> {
    let mut groups = BTreeMap::<String, AllAccumulator>::new();
    for DayRow { day, mut row } in rows {
        let period = bucket.key(day);
        row.period = period.clone();
        groups.entry(period).or_default().add(row);
    }
    groups
        .into_iter()
        .map(|(period, group)| group.into_row(period))
        .collect()
}

#[cfg(test)]
mod tests;
