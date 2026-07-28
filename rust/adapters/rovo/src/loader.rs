use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{read_session_file, rovo_entry_key, rovo_entry_to_loaded},
    paths::discover_session_files,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Rovo Dev"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_session_files()?;
    // Read session files in parallel, then apply the first-wins dedup
    // sequentially over the original discovery order so the surviving entry
    // per key matches the single-threaded read. Forked sessions duplicate the
    // parent's responses (same provider response ids, or the same
    // timestamp+token tuples for id-less legacy records), so the dedup is what
    // keeps fork families from double-counting.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        read_session_file(file).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!("Failed to read Rovo Dev session file: {error}"),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file_entries in loaded {
        for entry in file_entries {
            let key = rovo_entry_key(&entry);
            if seen.insert(key) {
                entries.push(rovo_entry_to_loaded(
                    entry,
                    tz.as_ref(),
                    shared.mode,
                    pricing,
                ));
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::super::paths::ROVO_DATA_DIR_ENV;
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn response_json(response_id: &str, timestamp: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "kind": "response",
            "model_name": "claude-sonnet-4-5-20250929",
            "provider_response_id": response_id,
            "timestamp": timestamp,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "details": {
                    "input_tokens": input,
                    "output_tokens": output,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0
                }
            }
        })
        .to_string()
    }

    #[test]
    fn loads_rovo_session_usage_entries() {
        let fixture = fs_fixture!({
            "sessions/session-a/session_context.json": format!(
                r#"{{"id": "session-a", "timestamp": 1771880550, "workspace_path": "/w",
                     "message_history": [{}]}}"#,
                response_json("msg_vrtx_1", "2026-02-23T21:03:45.664227Z", 100, 50)
            ),
        });
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-02-23");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].project_path.as_ref(), "/w");
        assert!(
            entries[0].cost > 0.0,
            "embedded pricing must price sonnet 4.5"
        );
    }

    #[test]
    fn dedupes_forked_sessions_by_provider_response_id() {
        let shared_turn = response_json("msg_vrtx_shared", "2026-02-23T21:03:45.664227Z", 100, 50);
        let fork_turn = response_json("msg_vrtx_fork", "2026-02-24T09:00:00.000000Z", 10, 5);
        let fixture = fs_fixture!({
            "sessions/parent/session_context.json": format!(
                r#"{{"message_history": [{shared_turn}]}}"#
            ),
            "sessions/fork/session_context.json": format!(
                r#"{{"message_history": [{shared_turn}, {fork_turn}]}}"#
            ),
        });
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            entries.len(),
            2,
            "the copied parent turn must not double-count"
        );
        let total_input: u64 = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(total_input, 110);
    }

    #[test]
    fn dedupes_forked_legacy_sessions_without_response_ids() {
        // Legacy (CLI 0.6.x) responses have no provider_response_id/vendor_id,
        // so the copied fork prefix must dedupe on the timestamp+token key.
        let shared_turn = r#"{
            "kind": "response",
            "model_name": null,
            "vendor_id": null,
            "timestamp": "2025-06-22T12:04:55.802259+00:00",
            "usage": {
                "requests": 0,
                "request_tokens": 100,
                "response_tokens": 40,
                "total_tokens": 140,
                "details": {
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "input_tokens": 100,
                    "output_tokens": 40
                }
            }
        }"#;
        let fork_turn = shared_turn.replace("12:04:55.802259", "12:09:01.000001");
        let fixture = fs_fixture!({
            "sessions/parent/session_context.json": format!(
                r#"{{"message_history": [{shared_turn}]}}"#
            ),
            "sessions/fork/session_context.json": format!(
                r#"{{"message_history": [{shared_turn}, {fork_turn}]}}"#
            ),
        });
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            single_thread: true,
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            entries.len(),
            2,
            "the copied legacy parent turn must not double-count"
        );
        let total_input: u64 = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(total_input, 200);
    }

    #[test]
    fn survives_malformed_session_files() {
        let fixture = fs_fixture!({
            "sessions/bad/session_context.json": "not json",
            "sessions/good/session_context.json": format!(
                r#"{{"message_history": [{}]}}"#,
                response_json("msg_vrtx_2", "2026-02-23T21:03:45.664227Z", 7, 3)
            ),
        });
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.root());
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "good");
    }
}
