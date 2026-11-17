use std::{
    collections::BTreeMap,
    fs::{File, FileTimes},
    path::Path,
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
        &crate::PricingMap::load_embedded(),
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
        let _ = fixture.write_file(path, lines.join("\n"));
    }
    let shared = SharedArgs {
        timezone: Some("UTC".to_string()),
        ..SharedArgs::default()
    };
    load_groups_from_directory(fixture.root(), &shared, kind).unwrap()
}

pub(crate) fn rollout_events(dir: &Path) -> Vec<CodexTokenUsageEvent> {
    let diagnostics = Diagnostics::new("codex");
    let files = paths::collect_codex_usage_files(dir, &diagnostics);
    let plan = CodexReplayPlan::new(
        crate::replay::read_session_metadata([(dir, files.as_slice())], true),
        None,
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
