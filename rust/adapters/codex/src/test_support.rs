use std::{collections::BTreeMap, path::Path};

use ccusage_core::Diagnostics;

use crate::{
    CodexGroup, CodexTokenUsageEvent, Result, aggregate,
    cli::{AgentReportKind, SharedArgs},
    parser,
    paths::{self, CodexUsageSource},
    replay::CodexReplayPlan,
};

pub(crate) fn directory_source(dir: &Path) -> CodexUsageSource {
    CodexUsageSource::new_for_test(dir.to_path_buf(), dir.to_path_buf())
}

pub(crate) fn load_groups_from_sources(
    sources: &[CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    aggregate::load_groups_from_sources_with_diagnostics(
        sources,
        shared,
        kind,
        &Diagnostics::new("codex"),
    )
    .map(|(groups, _)| groups)
}

pub(crate) fn load_groups_from_directory(
    dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<BTreeMap<String, CodexGroup>> {
    load_groups_from_sources(&[directory_source(dir)], shared, kind)
}

/// The usage events the rollouts under `dir` record after fork replay is
/// skipped, in file order and without dedupe across files.
pub(crate) fn rollout_events(dir: &Path) -> Vec<CodexTokenUsageEvent> {
    let diagnostics = Diagnostics::new("codex");
    let files = paths::collect_codex_usage_files(dir, &diagnostics);
    let plan = CodexReplayPlan::for_bounded_files(
        [(dir, files.as_slice())],
        [(dir, files.as_slice())],
        true,
        &diagnostics,
    );
    let mut events = Vec::new();
    for file in &files {
        parser::visit_codex_session_file_with_diagnostics(
            dir,
            file,
            plan.fork_replay(file),
            |event| {
                if !plan.is_replayed_compaction(file, event.response_id.as_deref()) {
                    events.push(event);
                }
                Ok(())
            },
            &diagnostics,
        )
        .unwrap();
    }
    events
}
