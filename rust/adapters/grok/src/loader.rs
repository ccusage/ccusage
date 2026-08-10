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
    // Global dedupe across files: the same server event can be exported into more
    // than one session, and `eventId` is what identifies it. Entries without one
    // are left alone rather than matched on their token counts, because
    // `parse_session_files` already deduped its own file using the full record —
    // including reasoning tokens, which `LoadedEntry` does not carry. Rebuilding a
    // coarser key here could only collapse turns the parser deliberately kept apart.
    let mut seen = HashSet::new();
    entries.retain(|entry| {
        let Some(event_id) = entry.data.message.id.as_deref() else {
            return true;
        };
        seen.insert(format!(
            "{event_id}|{}",
            entry.model.as_deref().unwrap_or_default()
        ))
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

    fn turn(
        event_id: &str,
        model: &str,
        input: u64,
        output: u64,
        cache: u64,
        reasoning: u64,
        seconds: i64,
    ) -> String {
        serde_json::json!({
            "timestamp": seconds,
            "params": {
                "sessionId": "sess",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "usage": {
                        "modelUsage": {
                            model: {
                                "inputTokens": input,
                                "outputTokens": output,
                                "cachedReadTokens": cache,
                                "reasoningTokens": reasoning,
                            }
                        }
                    }
                },
                "_meta": { "eventId": event_id }
            }
        })
        .to_string()
    }

    fn with_grok_home(fixture_root: &std::path::Path) -> EnvVarsGuard {
        EnvVarsGuard::set_many([(
            super::super::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture_root.as_os_str())),
        )])
    }

    #[test]
    fn loads_session_tree_with_uncached_split() {
        let line = turn("evt-load", "grok-4.5-build", 100, 20, 40, 10, 1_750_000_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-load/updates.jsonl": line,
            "sessions/proj/sess-load/summary.json": r#"{"info":{"id":"sess-load","cwd":"/tmp/proj"}}"#,
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = with_grok_home(fixture.root());
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2025-06-15");
        assert_eq!(entries[0].data.message.usage.input_tokens, 60);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 40);
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[0].model.as_deref(), Some("grok-4.5-build"));
        assert_eq!(entries[0].project_path.as_ref(), "/tmp/proj");
    }

    #[test]
    fn dedupes_the_same_event_across_session_files() {
        // The same server event can land in more than one session export; count it once.
        let shared_event = turn("evt-shared", "grok-4.5-build", 100, 20, 0, 0, 1_750_000_000);
        let other = turn("evt-other", "grok-4.5-build", 50, 5, 0, 0, 1_750_000_100);
        let fixture = fs_fixture!({
            "sessions/proj/sess-a/updates.jsonl": shared_event.clone(),
            "sessions/proj/sess-b/updates.jsonl": format!("{shared_event}\n{other}"),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = with_grok_home(fixture.root());
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        let input: u64 = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(input, 150);
    }

    #[test]
    fn keeps_usage_from_a_readable_session_when_a_sibling_file_is_corrupt() {
        let good = turn("evt-good", "grok-4.5-build", 80, 8, 0, 0, 1_750_000_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-good/updates.jsonl": good,
            "sessions/proj/sess-bad/updates.jsonl": "\0\0\0not-jsonl\n{broken",
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = with_grok_home(fixture.root());
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        // One corrupt file must not cost the rest of the home directory.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 80);
    }

    #[test]
    fn keeps_event_id_less_turns_that_differ_only_in_reasoning() {
        // Reasoning sits inside outputTokens and is not carried on LoadedEntry, so a
        // token-based key cannot tell these two turns apart. The parser keeps both;
        // the global dedupe must not undo that.
        let base = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-nr","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":REASONING}}}}}}"#;
        let fixture = fs_fixture!({
            "sessions/proj/sess-nr/updates.jsonl": format!(
                "{}\n{}\n",
                base.replace("REASONING", "5"),
                base.replace("REASONING", "10"),
            ),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = with_grok_home(fixture.root());
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        let input: u64 = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(input, 120);
    }

    #[test]
    fn sorts_entries_by_timestamp_across_sessions() {
        let early = turn("evt-early", "grok-4.5-build", 10, 1, 0, 0, 1_750_000_000);
        let late = turn("evt-late", "grok-4.5-build", 20, 2, 0, 0, 1_750_100_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-late/updates.jsonl": late,
            "sessions/proj/sess-early/updates.jsonl": early,
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let _guard = with_grok_home(fixture.root());
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("evt-early"));
        assert_eq!(entries[1].data.message.id.as_deref(), Some("evt-late"));
    }

    #[test]
    fn has_data_detects_updates_jsonl() {
        let fixture = fs_fixture!({
            "sessions/proj/sess/updates.jsonl": "{}\n",
        });
        let _guard = with_grok_home(fixture.root());
        assert!(has_data());
    }

    #[test]
    fn has_data_is_false_when_the_home_has_no_sessions() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": "{}\n",
        });
        let _guard = with_grok_home(fixture.root());
        assert!(!has_data());
    }
}
