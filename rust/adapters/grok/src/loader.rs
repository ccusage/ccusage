use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::parse_session_files,
    paths::{GrokSessionFiles, discover_session_files},
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent("Grok"), shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let sessions = discover_session_files()?;
    let updates_paths: Vec<_> = sessions
        .iter()
        .map(|session| session.updates.clone())
        .collect();
    let loaded = read_files_parallel(&updates_paths, shared.single_thread, |updates| {
        let session = GrokSessionFiles {
            updates: updates.to_path_buf(),
            summary: {
                let summary = updates.with_file_name("summary.json");
                summary.is_file().then_some(summary)
            },
        };
        parse_session_files(&session, tz.as_ref(), shared.mode, pricing).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read Grok session file {}: {error}",
                    session.updates.display()
                ),
            );
            Vec::new()
        })
    });

    let mut entries: Vec<_> = loaded.into_iter().flatten().collect();
    // Global dedupe across files (same eventId+model should not double-count).
    let mut seen = HashSet::new();
    entries.retain(|entry| {
        let usage = entry.data.message.usage;
        let key = entry
            .data
            .message
            .id
            .as_deref()
            .map(|event_id| format!("{event_id}|{}", entry.model.as_deref().unwrap_or_default()))
            .unwrap_or_else(|| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    entry.session_id.as_ref(),
                    entry.timestamp.as_millis(),
                    entry.model.as_deref().unwrap_or_default(),
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_input_tokens,
                    entry.extra_total_tokens,
                )
            });
        seen.insert(key)
    });
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

pub fn has_data() -> bool {
    discover_session_files().is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
    use std::ffi::OsString;

    #[test]
    fn loads_session_tree_with_uncached_split() {
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-load","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120}}}},"_meta":{"eventId":"evt-load"}}}"#;
        let fixture = fs_fixture!({
            "sessions/proj/sess-load/updates.jsonl": line,
            "sessions/proj/sess-load/summary.json": r#"{"info":{"id":"sess-load","cwd":"/tmp/proj"}}"#,
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2025-06-15");
        assert_eq!(entries[0].data.message.usage.input_tokens, 60);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 40);
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert_eq!(entries[0].model.as_deref(), Some("grok-4.5-build"));
    }

    #[test]
    fn has_data_detects_updates_jsonl() {
        let fixture = fs_fixture!({
            "sessions/proj/sess/updates.jsonl": "{}\n",
        });
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture.root().as_os_str())),
        )]);
        assert!(has_data());
    }
}
