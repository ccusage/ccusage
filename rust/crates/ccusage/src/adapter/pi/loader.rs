use std::{collections::HashSet, path::PathBuf};

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
    load_entries_from_paths(
        shared,
        paths::paths(custom_path)?,
        pricing,
        PiLoadScope::Default,
    )
}

#[cfg(test)]
pub(crate) fn load_entries_for_store_path(
    shared: &SharedArgs,
    store_path: &str,
    store_name: &str,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries_for_store_paths(
        shared,
        paths::named_store_paths(store_path)?,
        store_name,
        pricing,
    )
}

pub(crate) fn load_entries_for_store_paths(
    shared: &SharedArgs,
    store_paths: Vec<PathBuf>,
    store_name: &str,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries_from_paths(
        shared,
        store_paths,
        pricing,
        PiLoadScope::Named { store_name },
    )
}

#[derive(Clone, Copy)]
enum PiLoadScope<'a> {
    Default,
    Named { store_name: &'a str },
}

impl<'a> PiLoadScope<'a> {
    fn store_name(self) -> &'a str {
        match self {
            Self::Default => "pi",
            Self::Named { store_name } => store_name,
        }
    }

    fn debug_label(self) -> String {
        match self {
            Self::Default => "pi".to_string(),
            Self::Named { store_name } => format!("pi-format store '{store_name}'"),
        }
    }
}

fn load_entries_from_paths(
    shared: &SharedArgs,
    paths: Vec<PathBuf>,
    pricing: Option<&PricingMap>,
    scope: PiLoadScope<'_>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        let mut files = Vec::new();
        collect_files_with_extension(&path, "jsonl", &mut files);
        // Read session files in parallel; the first-wins dedup runs sequentially
        // over the original file order so the surviving record per id matches the
        // single-threaded read.
        let loaded = read_files_parallel(&files, shared.single_thread, |file| {
            let result = match scope {
                PiLoadScope::Default => {
                    parser::read_session_file(file, tz.as_ref(), shared.mode, pricing)
                }
                PiLoadScope::Named { store_name } => parser::read_session_file_for_store(
                    file,
                    &path,
                    tz.as_ref(),
                    shared.mode,
                    pricing,
                    store_name,
                ),
            };
            result.unwrap_or_else(|error| {
                let label = scope.debug_label();
                debug_log(
                    shared,
                    format!(
                        "Failed to read {label} session file {}: {error}",
                        file.display()
                    ),
                );
                Vec::new()
            })
        });
        for file_entries in loaded {
            for entry in file_entries {
                let id = match scope {
                    PiLoadScope::Default => parser::entry_id(&entry),
                    PiLoadScope::Named { .. } => {
                        parser::entry_id_for_store(scope.store_name(), &entry)
                    }
                };
                if seen.insert(id) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
