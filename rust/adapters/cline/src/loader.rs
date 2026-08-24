use std::{collections::HashSet, fs, path::Path};

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, read_files_parallel};

use super::{
    parser::{ClineEntry, parse_messages_file, to_loaded_entry},
    paths::cline_messages_files,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Cline"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = crate::parse_tz(shared.timezone.as_deref());
    let files = cline_messages_files()?;
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        load_messages_file(file, shared)
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file_entries in loaded {
        for entry in file_entries {
            // Dedupe by (session_id, timestamp, model) so a re-read of the
            // same transcript doesn't double-count messages.
            let key = (
                entry.session_id.clone(),
                entry.timestamp.as_millis(),
                entry.model.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            entries.push(to_loaded_entry(entry, tz.as_ref(), pricing));
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_messages_file(file_path: &Path, shared: &SharedArgs) -> Vec<ClineEntry> {
    let Ok(contents) = fs::read_to_string(file_path) else {
        crate::debug_log(
            shared,
            format!("Failed to read Cline transcript: {}", file_path.display()),
        );
        return Vec::new();
    };
    parse_messages_file(&contents, shared)
}
