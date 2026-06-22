use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{kimi_entry_key, kimi_entry_to_loaded, read_wire_file},
    paths::discover_wire_files,
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Kimi, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_wire_files()?;
    // Read wire files in parallel, then apply the first-wins dedup sequentially
    // over the original discovery order so the surviving entry per key matches
    // the single-threaded read.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        read_wire_file(file).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!("Failed to read Kimi wire file {}: {error}", file.display()),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file_entries in loaded {
        for entry in file_entries {
            let key = kimi_entry_key(&entry);
            if seen.insert(key) {
                entries.push(kimi_entry_to_loaded(
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
    use super::super::paths::KIMI_DATA_DIR_ENV;
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn loads_status_update_token_usage_from_wire_files() {
        let fixture = fs_fixture!({
            "config.json": r#"{"model":"kimi-k2"}"#,
            "sessions/group/session-a/wire.jsonl": [
                r#"{"type":"metadata","protocol_version":"1.3"}"#,
                r#"{"timestamp":1770983426.420942,"message":{"type":"TurnBegin","payload":{"user_input":"hello"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"msg-1"}}}"#,
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-02-13");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("kimi-k2"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
    }

    #[test]
    fn loads_kimi_code_usage_record_format() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-b/agents/agent-1/wire.jsonl": [
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":3064,"output":76,"inputCacheRead":14848,"inputCacheCreation":0},"usageScope":"turn","time":1782113184943}"#,
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":5000,"output":200,"inputCacheRead":20000,"inputCacheCreation":100},"usageScope":"session","time":1782113185000}"#,
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1, "session-scoped record must be skipped");
        assert_eq!(entries[0].session_id.as_ref(), "session-b");
        assert_eq!(entries[0].model.as_deref(), Some("kimi-for-coding"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 3064);
        assert_eq!(entries[0].data.message.usage.output_tokens, 76);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 14848);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 0);
    }

    #[test]
    fn skips_malformed_and_zero_token_wire_lines() {
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": [
                "not json",
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":0,"output":0,"input_cache_read":0,"input_cache_creation":0}}}}"#,
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert!(entries.is_empty());
    }
}
