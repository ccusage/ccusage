use std::{
    collections::BTreeMap,
    fs::{File, FileTimes},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use ccusage_core::Diagnostics;
use ccusage_test_support::Fixture;
use serde_json::{Value, json};

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
        ccusage_core::cli::WeekDay::Monday,
        Some(&crate::PricingMap::load_embedded()),
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

pub(crate) fn token_count_line(timestamp: &str, model: &str, last_token_usage: Value) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": { "model": model, "last_token_usage": last_token_usage },
        },
    })
    .to_string()
}

pub(crate) fn service_tier_line(timestamp: &str, service_tier: &str) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "thread_settings_applied",
            "thread_settings": { "service_tier": service_tier },
        },
    })
    .to_string()
}

pub(crate) fn load_rollouts(
    rollouts: &[(&str, &[String])],
    kind: AgentReportKind,
) -> BTreeMap<String, CodexGroup> {
    let fixture = Fixture::new();
    for (path, lines) in rollouts {
        fixture.write_file(path, lines.join("\n"));
    }
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    load_groups_from_directory(fixture.root(), &shared, kind).unwrap()
}

pub(crate) fn rollout_events(dir: &Path) -> Vec<CodexTokenUsageEvent> {
    rollout_events_with_single_thread(dir, true)
}

pub(crate) fn rollout_events_with_single_thread(
    dir: &Path,
    single_thread: bool,
) -> Vec<CodexTokenUsageEvent> {
    let diagnostics = Diagnostics::new("codex");
    let files = collect_codex_usage_files(dir, &diagnostics);
    let selection = [(dir, files.as_slice())];
    let plan = CodexReplayPlan::for_selection(&selection, selection, single_thread, &diagnostics);
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
            },
            &diagnostics,
        );
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

pub(super) fn collect_codex_usage_files(dir: &Path, diagnostics: &Diagnostics) -> Vec<PathBuf> {
    let source = paths::CodexUsageSource::new(dir.to_path_buf(), dir.to_path_buf());
    let mut files = Vec::new();
    paths::visit_deduped_codex_usage_files(&[source], diagnostics, |_, file| files.push(file));
    files
}
