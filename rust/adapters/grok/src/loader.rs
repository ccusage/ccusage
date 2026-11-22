use std::collections::HashSet;

use ccusage_adapter_common::{source::LoadContext, units::read_typed_units};

use crate::{LoadedEntry, Result};

use super::{parser::GrokDedupeKey, paths::discover_session_files};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let sessions = super::paths::discover_session_files(cx.diagnostics)?;
    let loaded = read_typed_units(&sessions, shared.single_thread, cx.diagnostics, |updates| {
        super::parser::parse_session_files_with_diagnostics(
            updates,
            cx.window.timezone(),
            shared.mode,
            pricing,
            cx.diagnostics,
        )
    });

    // NOTE(2026-08-10, Grok CLI 1.0.0, #1593): Grok can export one server event into several sessions. Use its `eventId` to count that turn once across files.
    let mut seen_events = HashSet::new();
    let mut entries: Vec<_> = loaded
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|(key, entry)| match key {
            GrokDedupeKey::Event(event) => seen_events.insert(event).then_some(entry),
            GrokDedupeKey::Content { .. } => Some(entry),
        })
        .collect();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

pub(super) fn has_data() -> bool {
    discover_session_files(&ccusage_core::Diagnostics::new("grok"))
        .is_ok_and(|files| !files.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SharedArgs;
    use ccusage_core::PricingMap;
    use ccusage_test_support::{Environment, fs_fixture};
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

    fn with_grok_home(fixture_root: &std::path::Path) -> Environment {
        Environment::with_vars([(
            super::super::paths::GROK_HOME_ENV,
            Some(OsString::from(fixture_root.as_os_str())),
        )])
    }

    #[test]
    fn loads_session_tree_with_uncached_split() {
        ccusage_test_support::install_pricing_snapshots();
        let line = turn("evt-load", "grok-4.5-build", 100, 20, 40, 10, 1_750_000_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-load/updates.jsonl": line,
            "sessions/proj/sess-load/summary.json": r#"{"info":{"id":"sess-load","cwd":"/tmp/proj"}}"#,
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2025-06-15");
            assert_eq!(entries[0].usage.input_tokens, 60);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 40);
            assert_eq!(entries[0].usage.output_tokens, 20);
            assert_eq!(entries[0].extra_total_tokens, 0);
            assert_eq!(entries[0].model.as_deref(), Some("grok-4.5-build"));
            assert_eq!(entries[0].project_path.as_ref(), "/tmp/proj");
        })
    }

    #[test]
    fn dedupes_the_same_event_across_session_files() {
        ccusage_test_support::install_pricing_snapshots();
        let shared_event = turn("evt-shared", "grok-4.5-build", 100, 20, 0, 0, 1_750_000_000);
        let other = turn("evt-other", "grok-4.5-build", 50, 5, 0, 0, 1_750_000_100);
        let fixture = fs_fixture!({
            "sessions/proj/sess-a/updates.jsonl": shared_event.clone(),
            "sessions/proj/sess-b/updates.jsonl": format!("{shared_event}\n{other}"),
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 2);
            let input: u64 = entries.iter().map(|entry| entry.usage.input_tokens).sum();
            assert_eq!(input, 150);
        })
    }

    #[test]
    fn keeps_usage_from_a_readable_session_when_a_sibling_file_is_corrupt() {
        ccusage_test_support::install_pricing_snapshots();
        let good = turn("evt-good", "grok-4.5-build", 80, 8, 0, 0, 1_750_000_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-good/updates.jsonl": good,
            "sessions/proj/sess-bad/updates.jsonl": "\0\0\0not-jsonl\n{broken",
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].usage.input_tokens, 80);
        })
    }

    #[test]
    fn keeps_event_id_less_turns_that_differ_only_in_reasoning() {
        ccusage_test_support::install_pricing_snapshots();
        let base = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-nr","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":REASONING}}}}}}"#;
        let fixture = fs_fixture!({
            "sessions/proj/sess-nr/updates.jsonl": format!(
                "{}\n{}\n",
                base.replace("REASONING", "5"),
                base.replace("REASONING", "10"),
            ),
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 2);
            let input: u64 = entries.iter().map(|entry| entry.usage.input_tokens).sum();
            assert_eq!(input, 120);
        })
    }

    #[test]
    fn sorts_entries_by_timestamp_across_sessions() {
        ccusage_test_support::install_pricing_snapshots();
        let early = turn("evt-early", "grok-4.5-build", 10, 1, 0, 0, 1_750_000_000);
        let late = turn("evt-late", "grok-4.5-build", 20, 2, 0, 0, 1_750_100_000);
        let fixture = fs_fixture!({
            "sessions/proj/sess-late/updates.jsonl": late,
            "sessions/proj/sess-early/updates.jsonl": early,
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].message_id.as_deref(), Some("evt-early"));
            assert_eq!(entries[1].message_id.as_deref(), Some("evt-late"));
        })
    }

    #[test]
    fn keeps_project_paths_distinct_when_encoded_names_are_not_valid_utf8() {
        ccusage_test_support::install_pricing_snapshots();
        let first = turn("evt-lossy-a", "grok-4.5-build", 10, 1, 0, 0, 1_750_000_000);
        let second = turn("evt-lossy-b", "grok-4.5-build", 20, 2, 0, 0, 1_750_000_100);
        let fixture = fs_fixture!({
            "sessions/%2Ftmp%2F%FFproject/sess-a/updates.jsonl": first,
            "sessions/%2Ftmp%2F%FEproject/sess-b/updates.jsonl": second,
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        with_grok_home(fixture.root()).run(|| {
            let mut entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();
            entries.sort_by(|left, right| left.project_path.cmp(&right.project_path));

            assert_eq!(entries.len(), 2);
            assert_eq!(
                entries[0].project_path.as_ref(),
                "%2Ftmp%2F%FEproject",
                "{:?}",
                entries
                    .iter()
                    .map(|entry| entry.project_path.as_ref())
                    .collect::<Vec<_>>()
            );
            assert_eq!(entries[1].project_path.as_ref(), "%2Ftmp%2F%FFproject");
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.usage.input_tokens)
                    .sum::<u64>(),
                30
            );
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.usage.output_tokens)
                    .sum::<u64>(),
                3
            );
        })
    }

    #[test]
    fn has_data_detects_updates_jsonl() {
        let fixture = fs_fixture!({
            "sessions/proj/sess/updates.jsonl": "{}\n",
        });
        with_grok_home(fixture.root()).run(|| {
            assert!(has_data());
        })
    }

    #[test]
    fn has_data_is_false_when_the_home_has_no_sessions() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": "{}\n",
        });
        with_grok_home(fixture.root()).run(|| {
            assert!(!has_data());
        })
    }
}
