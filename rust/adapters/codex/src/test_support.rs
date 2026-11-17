use std::{
    collections::BTreeMap,
    fs::{File, FileTimes},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use ccusage_core::Diagnostics;

use crate::{
    CodexGroup, CodexTokenUsageEvent, Result, TimestampMs, aggregate,
    cli::{AgentReportKind, SharedArgs},
    parser,
    paths::{self, CodexUsageSource},
    replay::CodexReplayPlan,
};

pub(crate) fn directory_source(dir: &Path) -> CodexUsageSource {
    CodexUsageSource::new(dir.to_path_buf(), dir.to_path_buf())
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

pub(crate) fn set_file_modified(path: &Path, timestamp: TimestampMs) {
    let milliseconds = u64::try_from(timestamp.as_millis()).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(milliseconds)))
        .unwrap();
}
