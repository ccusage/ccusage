use std::path::Path;

use ccusage_core::Diagnostics;
use ccusage_test_support::fs_fixture;
use serde_json::{Value, json};

use crate::{
    cli::{AgentReportKind, SharedArgs},
    paths::CodexUsageSource,
    replay::CodexReplayPlan,
    test_support::load_groups_from_directory,
    types::CodexTokenUsageEvent,
};

mod archived;
mod auto_review;
mod bounded;
mod fork_boundary;
mod forks;
mod records;
mod replay;
mod service_tiers;

fn replay_metadata(timestamp: &str, id: &str, parent: Option<&str>) -> String {
    json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {"id": id, "forked_from_id": parent},
    })
    .to_string()
}

fn replay_token_count(timestamp: &str, input_tokens: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": 1,
                    "total_tokens": input_tokens + 1,
                },
            },
        },
    })
    .to_string()
}

fn replay_input_tokens_by_session(events: &[CodexTokenUsageEvent]) -> Vec<(String, u64)> {
    events
        .iter()
        .map(|event| (event.session_id.clone(), event.input_tokens))
        .collect()
}

fn check_replay_modes(dir: &Path, mut check: impl FnMut(bool, &[CodexTokenUsageEvent])) {
    for single_thread in [true, false] {
        let events = crate::test_support::rollout_events_with_single_thread(dir, single_thread);
        check(single_thread, &events);
    }
}

fn session_input_tokens(
    sources: &[CodexUsageSource],
    shared: &SharedArgs,
    diagnostics: &Diagnostics,
) -> (Vec<(String, u64)>, bool) {
    let (groups, detected) = crate::aggregate::load_groups_from_sources_with_diagnostics(
        sources,
        shared,
        AgentReportKind::Session,
        ccusage_core::cli::WeekDay::Monday,
        Some(&crate::PricingMap::load_embedded()),
        diagnostics,
    )
    .unwrap();
    let sessions = groups
        .into_iter()
        .map(|(session, group)| (session, group.input_tokens))
        .collect();
    (sessions, detected)
}
