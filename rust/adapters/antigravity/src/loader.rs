use std::collections::HashMap;

use ccusage_adapter_common::source::LoadContext;

use crate::{LoadedEntry, Result};

use super::{
    parser::{
        AntigravityUsageEvent, ParsedDatabase, event_to_loaded, merge_usage_event,
        parse_sqlite_file,
    },
    paths::conversation_db_paths,
};

pub(crate) fn load_entries_for_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let mut events = Vec::new();
    for database_path in conversation_db_paths(cx.diagnostics, cx.shared.debug)? {
        match parse_sqlite_file(&database_path, cx.diagnostics) {
            Ok(ParsedDatabase::Events(parsed)) => events.extend(parsed),
            Ok(ParsedDatabase::EmptySnapshot) => cx.diagnostics.debug_note(
                &database_path,
                format!(
                    "Skipping uninitialized Antigravity database '{}': empty SQLite snapshot",
                    database_path.display()
                ),
            ),
            Ok(ParsedDatabase::NoGenerationTable) => cx.diagnostics.debug_note(
                &database_path,
                format!(
                    "Skipping Antigravity database '{}': no gen_metadata table",
                    database_path.display()
                ),
            ),
            Err(error) => error.record(&database_path, cx.diagnostics),
        }
    }
    let mut events = deduplicate_events(events);
    events.sort_by_key(|event| event.timestamp);
    Ok(events
        .into_iter()
        .map(|event| event_to_loaded(event, cx.window.timezone(), pricing))
        .collect())
}

fn deduplicate_events(
    events: impl IntoIterator<Item = AntigravityUsageEvent>,
) -> Vec<AntigravityUsageEvent> {
    let mut slots: Vec<Option<AntigravityUsageEvent>> = Vec::new();
    let mut identity_indexes = HashMap::new();
    for event in events {
        let mut matching_indexes = event
            .identities
            .iter()
            .filter_map(|identity| identity_indexes.get(identity).copied())
            .collect::<Vec<_>>();
        matching_indexes.sort_unstable();
        matching_indexes.dedup();
        let Some(target_index) = matching_indexes.first().copied() else {
            let target_index = slots.len();
            for identity in &event.identities {
                identity_indexes.insert(identity.clone(), target_index);
            }
            slots.push(Some(event));
            continue;
        };

        for duplicate_index in matching_indexes.into_iter().skip(1) {
            if let Some(duplicate) = slots[duplicate_index].take() {
                merge_usage_event(
                    slots[target_index]
                        .as_mut()
                        .expect("deduplication target must exist"),
                    duplicate,
                );
            }
        }
        merge_usage_event(
            slots[target_index]
                .as_mut()
                .expect("deduplication target must exist"),
            event,
        );
        if let Some(target) = slots[target_index].as_ref() {
            for identity in &target.identities {
                identity_indexes.insert(identity.clone(), target_index);
            }
        }
    }
    slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests;
