use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, collect_files_with_extension, debug_log,
    parse_tz, read_files_parallel,
};

use super::{parser, paths};

pub(crate) fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Pi, shared.json, || {
        load_entries_inner(shared, custom_path, pricing)
    })
}

fn load_entries_inner(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths::paths(custom_path)? {
        let mut files = Vec::new();
        collect_files_with_extension(&path, "jsonl", &mut files);
        // Read session files in parallel; the first-wins dedup runs sequentially
        // over the original file order so the surviving record per id matches the
        // single-threaded read.
        let loaded = read_files_parallel(&files, shared.single_thread, |file| {
            parser::read_session_file(file, tz.as_ref(), shared.mode, pricing).unwrap_or_else(
                |error| {
                    debug_log(
                        shared,
                        format!("Failed to read pi session file {}: {error}", file.display()),
                    );
                    Vec::new()
                },
            )
        });
        for file_entries in loaded {
            for entry in file_entries {
                let id = parser::entry_id(&entry);
                if seen.insert(id) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
