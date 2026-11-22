use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, mpsc},
    thread,
};

use ccusage_adapter_common::{rows::SourceRow, source::LoadKind};
use ccusage_core::parallel::spawn_scoped_worker;

use super::{BaseRows, RunSource, plan::load_specs};
use crate::{
    Diagnostics, PricingMap, Result, Skip, SkipKind,
    cli::SharedArgs,
    report::sort_rows,
    types::{
        AgentLoadSpec, AgentRows, AllAccumulator, Bucket, DayRow, LoadedAgentRows, ReportDay,
        ReportRow,
    },
};

pub(super) fn load_day_rows(
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

pub(super) fn load_session_rows(
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

pub(super) fn finish_sessions(rows: Vec<SourceRow>, shared: &SharedArgs) -> Vec<ReportRow> {
    let mut rows = rows.into_iter().map(ReportRow::Source).collect::<Vec<_>>();
    ccusage_core::sort::stable_by(&mut rows, |a, b| {
        let cost_order = match crate::cli::session_cost_ordering(shared) {
            crate::cli::SortOrder::Asc => a.total_cost.total_cmp(&b.total_cost),
            crate::cli::SortOrder::Desc => b.total_cost.total_cmp(&a.total_cost),
        };
        cost_order
            .then_with(|| a.period.cmp(&b.period))
            .then_with(|| a.agent().cmp(b.agent()))
    });
    rows
}

pub(super) fn finish_days(
    rows: Vec<DayRow>,
    bucket: Bucket,
    shared: &SharedArgs,
) -> Vec<ReportRow> {
    let mut aggregated = aggregate_rows(rows, bucket);
    sort_rows(&mut aggregated, &shared.order.unwrap_or_default());
    aggregated
}

pub(crate) fn load_agent_rows_parallel(
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

        ccusage_core::sort::stable_by_key(&mut errors, |(index, _)| *index);
        if let Some((_, error)) = errors.into_iter().next() {
            return Err(error);
        }

        ccusage_core::sort::stable_by_key(&mut loaded, |loaded| loaded.index);
        Ok(loaded)
    })
}

pub(crate) fn aggregate_rows(rows: Vec<DayRow>, bucket: Bucket) -> Vec<ReportRow> {
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
