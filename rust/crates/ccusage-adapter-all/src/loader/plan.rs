use std::{collections::BTreeMap, sync::Arc};

use ccusage_adapter_common::{
    claims,
    source::{LoadKind, SourceOptions, UsageSource},
};

use super::{RunSource, RunSourceKind, pi_stores::resolve_named_pi_store_paths};
use crate::{
    PricingMap, Result,
    adapter::{claude, opencode, pi},
    cli::{AgentReportKind, SharedArgs, SourceArgs},
    sources::SOURCES,
    types::{AgentLoadSpec, AgentRows},
};

pub(super) fn run_sources<'a>(
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
    let default_pi = SourceOptions::from_source(sources.get("pi"));
    for store in resolve_named_pi_store_paths(&shared.pi_stores, default_pi)? {
        let name = Box::leak(store.name.into_boxed_str());
        run_sources.push(RunSource {
            source: RunSourceKind::Named(pi::NamedStore::new(name, store.paths)),
            shared,
            options: SourceOptions::default(),
        });
    }
    Ok(run_sources)
}

pub(super) fn load_specs<'a>(
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

pub(super) fn needs_daily_family(kinds: &[AgentReportKind]) -> bool {
    kinds
        .iter()
        .any(|&kind| LoadKind::for_report(kind) == LoadKind::Day)
}

pub(super) fn needs_session(kinds: &[AgentReportKind]) -> bool {
    kinds
        .iter()
        .any(|&kind| LoadKind::for_report(kind) == LoadKind::Session)
}
