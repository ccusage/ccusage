use std::collections::{HashMap, HashSet};

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, collect_files_with_extension, debug_log,
    parse_tz, read_files_parallel,
};

use super::{parser, paths, server};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Amp, shared.json, || {
        load_entries_inner(shared, pricing, false)
    })
}

pub(super) fn load_entries_for_amp_command(
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Amp, shared.json, || {
        load_entries_inner(shared, pricing, true)
    })
}

fn load_entries_inner(
    shared: &SharedArgs,
    pricing: &PricingMap,
    warn_server_failure: bool,
) -> Result<Vec<LoadedEntry>> {
    let mut entries = Vec::new();
    let tz = parse_tz(shared.timezone.as_deref());
    for path in paths::paths()? {
        let threads_dir = path.join("threads");
        let mut files = Vec::new();
        collect_files_with_extension(&threads_dir, "json", &mut files);
        let per_file = read_files_parallel(&files, shared.single_thread, |file| {
            parser::read_thread_file(file, tz.as_ref(), shared.mode, Some(pricing)).unwrap_or_else(
                |error| {
                    debug_log(
                        shared,
                        format!("Failed to read Amp thread file {}: {error}", file.display()),
                    );
                    Vec::new()
                },
            )
        });
        for file_entries in per_file {
            entries.extend(file_entries);
        }
    }
    // Unit tests frequently exercise unified loaders with synthetic fixtures;
    // never let those test processes reach a developer's authenticated Amp
    // account. Server parsing and selection are tested independently.
    if !cfg!(test) && paths::uses_default_source() {
        let mut local_latest_usage = HashMap::new();
        for entry in &entries {
            local_latest_usage
                .entry(entry.session_id.to_string())
                .and_modify(|timestamp: &mut crate::TimestampMs| {
                    *timestamp = (*timestamp).max(entry.timestamp);
                })
                .or_insert(entry.timestamp);
        }
        match server::load_entries(
            &local_latest_usage,
            tz.as_ref(),
            shared.mode,
            pricing,
            shared.single_thread,
            shared.since.as_deref(),
        ) {
            Ok(server_entries) => {
                if server_entries.list_truncated {
                    eprintln!(
                        "WARN  Amp's thread list did not advance past 500 threads; Amp totals may be incomplete."
                    );
                }
                if server_entries.failed_exports > 0 {
                    eprintln!(
                        "WARN  Failed to export {} server-backed Amp thread(s); Amp totals are incomplete.",
                        server_entries.failed_exports
                    );
                }
                append_deduped_server_entries(&mut entries, server_entries.entries);
            }
            Err(error) => {
                debug_log(
                    shared,
                    format!("Failed to load server-backed Amp threads: {error}"),
                );
                if warn_server_failure || !entries.is_empty() {
                    eprintln!(
                        "WARN  Server-backed Amp threads could not be loaded; showing legacy local Amp data only."
                    );
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn append_deduped_server_entries(
    entries: &mut Vec<LoadedEntry>,
    mut server_entries: Vec<LoadedEntry>,
) {
    let mut seen_usage = entries.iter().map(usage_key).collect::<HashSet<_>>();
    server_entries.retain(|entry| seen_usage.insert(usage_key(entry)));
    entries.append(&mut server_entries);
}

fn usage_key(
    entry: &LoadedEntry,
) -> (
    String,
    crate::TimestampMs,
    Option<String>,
    u64,
    u64,
    u64,
    u64,
) {
    let usage = entry.data.message.usage;
    (
        entry.session_id.to_string(),
        entry.timestamp,
        entry.model.clone(),
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CostMode;

    #[test]
    fn keeps_legacy_credits_when_server_export_repeats_usage() {
        let mut local = parser::parse_thread(
            br#"{
                "id":"T-thread",
                "usageLedger":{"events":[{
                    "id":"event-1",
                    "toMessageId":2,
                    "timestamp":"2026-03-31T10:00:00.000Z",
                    "model":"claude-sonnet-4-20250514",
                    "tokens":{"input":12,"output":34},
                    "credits":1.5
                }]},
                "messages":[{"role":"assistant","messageId":2,"usage":{
                    "cacheCreationInputTokens":56,"cacheReadInputTokens":78
                }}]
            }"#,
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();
        let server = parser::parse_server_thread(
            br#"{
                "id":"T-thread",
                "messages":[{"role":"assistant","messageId":2,"usage":{
                    "model":"claude-sonnet-4-20250514",
                    "inputTokens":12,"outputTokens":34,
                    "cacheCreationInputTokens":56,"cacheReadInputTokens":78,
                    "timestamp":"2026-03-31T10:00:00.000Z"
                }}]
            }"#,
            "T-thread",
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();

        append_deduped_server_entries(&mut local, server);

        assert_eq!(local.len(), 1);
        assert_eq!(local[0].credits, Some(1.5));
    }
}
