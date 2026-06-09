use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, collect_files_with_extension, debug_log,
    parse_tz,
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
    let mut files = Vec::new();
    for path in paths::paths(custom_path)? {
        collect_files_with_extension(&path, "jsonl", &mut files);
    }
    let parsed = crate::cache::load_with_cache(
        "pi",
        &files,
        shared.single_thread,
        crate::cache::cost_fingerprint(pricing.as_ref().map(|p| p.fingerprint()), shared.mode),
        shared.live_only,
        |file| {
            Ok(
                parser::read_session_file(file, tz.as_ref(), shared.mode, pricing).unwrap_or_else(
                    |error| {
                        debug_log(
                            shared,
                            format!("Failed to read pi session file {}: {error}", file.display()),
                        );
                        Vec::new()
                    },
                ),
            )
        },
    )?;
    let mut seen = HashSet::new();
    let mut entries = Vec::with_capacity(parsed.len());
    for entry in parsed {
        let id = parser::entry_id(&entry);
        if seen.insert(id) {
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
