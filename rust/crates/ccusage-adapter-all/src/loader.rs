use std::{collections::BTreeMap, path::PathBuf};

use ccusage_adapter_common::{
    claims::native_sessions,
    source::{LoadKind, LoadRequest, ProgressOwner, SourceOptions, UsageSource},
};

use crate::{
    Diagnostics, LoadedEntry, PricingMap, Result,
    adapter::{claude, opencode, pi},
    cli::{AgentReportKind, SharedArgs, SourceArgs, WeekDay},
};

use super::types::{AllLoadResult, AllSectionsLoadResult, Bucket};

mod pi_stores;
mod plan;
mod pricing;
mod rows;

use plan::{needs_daily_family, needs_session, run_sources};
use pricing::load_pricing;
#[cfg(test)]
pub(super) use rows::{aggregate_rows, load_agent_rows_parallel};
use rows::{finish_days, finish_sessions, load_day_rows, load_session_rows};

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

struct BaseRows<R> {
    rows: Vec<R>,
    detected_agents: Vec<&'static str>,
    diagnostics: Vec<Diagnostics>,
}

struct ResolvedNamedPiStore {
    name: String,
    paths: Vec<PathBuf>,
}

pub(super) fn load_opencode_entries(
    shared: &SharedArgs,
    kind: AgentReportKind,
    include_claude_agent: bool,
    claude_source: Option<&ccusage_cli::SourceArgs>,
) -> Result<(Vec<LoadedEntry>, Option<Diagnostics>)> {
    let entries = opencode::load_entries_with_native_sessions(shared, kind)?;
    if include_claude_agent {
        return Ok((entries.into_iter().map(|(entry, _)| entry).collect(), None));
    }
    let sessions = native_sessions(&entries);
    if sessions.is_empty() {
        return Ok((entries.into_iter().map(|(entry, _)| entry).collect(), None));
    }
    let claude_shared = claude_source.map_or(shared, |source| &source.shared);
    let diagnostics = Diagnostics::new("claude");
    let counted = claude::SOURCE.counted(claude_shared, sessions, &diagnostics)?;
    Ok((counted.retain_uncounted(entries), Some(diagnostics)))
}

#[cfg(test)]
mod tests;
