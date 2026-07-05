use std::collections::{HashMap, HashSet};

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{parser, paths};

pub(crate) fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Devin, shared.json, || {
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
    for data_dir in paths::paths(custom_path)? {
        let session_info = load_session_info_for_directory(&data_dir);
        let transcripts_dir = data_dir.join("transcripts");
        let mut files = Vec::new();
        crate::collect_files_with_extension(&transcripts_dir, "json", &mut files);
        let loaded = read_files_parallel(&files, shared.single_thread, |file| {
            let session_id = session_id_from_path(file);
            let info = session_info.get(&session_id);
            parser::read_transcript_file(file, tz.as_ref(), shared.mode, pricing, info)
                .unwrap_or_else(|error| {
                    debug_log(
                        shared,
                        format!(
                            "Failed to read Devin transcript file {}: {error}",
                            file.display()
                        ),
                    );
                    Vec::new()
                })
        });
        for file_entries in loaded {
            for entry in file_entries {
                let id = entry_id(&entry);
                if seen.insert(id) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_session_info_for_directory(
    data_dir: &std::path::Path,
) -> HashMap<String, super::parser::SessionInfo> {
    let db_path = data_dir.join("sessions.db");
    if !db_path.is_file() {
        return HashMap::new();
    }
    parser::load_session_info(&db_path).into_iter().collect()
}

fn session_id_from_path(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

pub(super) fn entry_id(entry: &LoadedEntry) -> String {
    [
        "devin",
        entry.project.as_ref(),
        entry.session_id.as_ref(),
        entry.data.timestamp.as_str(),
        entry.model.as_deref().unwrap_or_default(),
        &entry.data.message.usage.input_tokens.to_string(),
        &entry.data.message.usage.output_tokens.to_string(),
        &entry
            .data
            .message
            .usage
            .cache_creation_input_tokens
            .to_string(),
        &entry.data.message.usage.cache_read_input_tokens.to_string(),
        &entry.cost.to_string(),
    ]
    .join(":")
}
