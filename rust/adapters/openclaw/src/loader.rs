use std::{collections::HashSet, sync::Arc};

use crate::{LoadedEntry, Result};
use ccusage_adapter_common::{source::LoadContext, units::read_typed_units};

use super::{
    parser::{content_id, migration_id, parse_session_file},
    paths::{collect_session_files, paths},
    sqlite::collect_sqlite_entries,
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let mut sqlite_entries = Vec::new();
    let mut jsonl_entries = Vec::new();
    for root in paths(cx.options.open_claw_path) {
        sqlite_entries.extend(collect_sqlite_entries(
            &root,
            cx.window.timezone(),
            cx.shared.mode,
            pricing,
            cx.diagnostics,
        ));
        let files = collect_session_files(&root, cx.diagnostics);
        let loaded = read_typed_units(&files, cx.shared.single_thread, cx.diagnostics, |file| {
            parse_session_file(
                file,
                cx.window.timezone(),
                cx.shared.mode,
                pricing,
                cx.diagnostics,
            )
        });
        jsonl_entries.extend(loaded.into_iter().flatten().flatten());
    }

    // NOTE(2026-09-16, #1727): `openclaw doctor --fix` leaves the original JSONL files after migrating their events into SQLite. Prefer the SQLite copies because they can carry corrected provider-billed cost.
    let (identified, id_less): (Vec<_>, Vec<_>) = sqlite_entries
        .into_iter()
        .partition(|entry| entry.message_id.is_some());
    let mut events = HashSet::new();
    let mut contents = HashSet::new();
    let mut sqlite_events = HashSet::new();
    let mut entries = Vec::new();
    for entry in identified.into_iter().chain(id_less) {
        let content = content_id(&entry);
        let is_new = match &entry.message_id {
            Some(event_id) => events.insert((Arc::clone(&entry.session_id), event_id.clone())),
            None => !contents.contains(&content),
        };
        if is_new {
            contents.insert(content);
            sqlite_events.insert(migration_id(&entry));
            entries.push(entry);
        }
    }
    for entry in jsonl_entries {
        if !sqlite_events.contains(&migration_id(&entry)) && contents.insert(content_id(&entry)) {
            entries.push(entry);
        }
    }
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use ccusage_core::{LoadedEntry, PricingMap, cli::SharedArgs};
    use ccusage_test_support::{Fixture, fs_fixture};

    fn load_fixture(fixture: &Fixture, shared: &SharedArgs) -> Vec<LoadedEntry> {
        ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            shared,
            ccusage_adapter_common::source::SourceOptions {
                open_claw_path: fixture.root().to_str(),
                ..Default::default()
            },
            Some(&PricingMap::default()),
        )
        .expect("load the OpenClaw fixture")
    }

    #[test]
    fn loads_assistant_usage_and_uses_model_change_events() {
        let fixture = fs_fixture!({
            "agents/main/sessions/abc.jsonl": [
                r#"{"type":"model_change","provider":"openai-codex","modelId":"gpt-5.2"}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":1660,"output":55,"cacheRead":108928,"cost":{"total":0.02}},"timestamp":1769753935279}}"#,
            ]
            .join("\n"),
        });
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let entries = load_fixture(&fixture, &shared);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2026-01-30");
        assert_eq!(entries[0].session_id.as_ref(), "abc");
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].version.as_deref(), Some("openai-codex"));
        assert_eq!(entries[0].usage.input_tokens, 1660);
        assert_eq!(entries[0].usage.output_tokens, 55);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 108_928);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert!((entries[0].cost - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn deduplicates_repeated_openclaw_records() {
        let line = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1,"output":1,"totalTokens":2},"timestamp":1769753935279}}"#;
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!("{line}\n{line}\n"),
        });
        let entries = load_fixture(&fixture, &SharedArgs::default());

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn calculates_cost_from_pricing_overrides() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "agents/main/sessions/abc.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1000,"output":500,"cost":{"total":0.99}},"timestamp":1769753935279}}"#,
        });
        let mut shared = SharedArgs {
            mode: crate::cli::CostMode::Calculate,
            offline: true,
            ..SharedArgs::default()
        };
        shared.pricing_overrides.insert(
            "[openclaw] gpt-5.2".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                ..Default::default()
            },
        );
        let pricing =
            PricingMap::load_with_overrides(shared.offline, false, shared.pricing_overrides.iter());

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            ccusage_adapter_common::source::SourceOptions {
                open_claw_path: fixture.root().to_str(),
                ..Default::default()
            },
            Some(&pricing),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert!((entries[0].cost - 0.002).abs() < f64::EPSILON);
        assert_eq!(entries[0].recorded_cost, Some(0.99));
    }

    #[test]
    fn loads_sqlite_usage_when_no_jsonl_sessions_exist() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("agents/main/agent/openclaw-agent.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        for (seq, event_json) in [
            r#"{"id":"evt-1","type":"model_change","modelId":"deepseek-v4-flash","provider":"deepseek"}"#,
            r#"{"id":"evt-2","type":"message","message":{"role":"assistant","usage":{"input":100,"output":50,"cost":{"total":1.80}},"timestamp":1769753935279}}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let mut statement = db
                .prepare(
                    "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session-sqlite', ?1, ?2, 1769753935279)",
                )
                .unwrap();
            statement.bind((1, i64::try_from(seq).unwrap())).unwrap();
            statement.bind((2, event_json)).unwrap();
            statement.next().unwrap();
        }
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        let entries = load_fixture(&fixture, &shared);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2026-01-30");
        assert_eq!(entries[0].session_id.as_ref(), "session-sqlite");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert_eq!(entries[0].version.as_deref(), Some("deepseek"));
        assert!((entries[0].cost - 1.80).abs() < f64::EPSILON);
        assert_eq!(entries[0].message_id.as_deref(), Some("evt-2"));
    }

    #[test]
    fn prefers_sqlite_over_migrated_jsonl_duplicates() {
        let line = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20,"cost":{"total":0.50}},"timestamp":1769753935279}}"#;
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!("{line}\n"),
        });
        let db_path = fixture.path("agents/main/agent/openclaw-agent.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session', 0, ?1, 1769753935279)",
            )
            .unwrap();
        statement
            .bind((
                1,
                r#"{"id":"evt-sqlite","type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20,"cost":{"total":0.51}},"timestamp":1769753935279}}"#,
            ))
            .unwrap();
        statement.next().unwrap();

        let entries = load_fixture(&fixture, &SharedArgs::default());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("evt-sqlite"));
        assert!((entries[0].cost - 0.51).abs() < f64::EPSILON);
    }

    fn write_transcript_events(fixture: &ccusage_test_support::Fixture, events: &[&str]) {
        let db_path = fixture.path("agents/main/agent/openclaw-agent.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        for (seq, event_json) in events.iter().enumerate() {
            let mut statement = db
                .prepare(
                    "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session', ?1, ?2, 1769753935279)",
                )
                .unwrap();
            statement.bind((1, i64::try_from(seq).unwrap())).unwrap();
            statement.bind((2, *event_json)).unwrap();
            statement.next().unwrap();
        }
    }

    fn assistant_event(id: Option<&str>, cost: f64) -> String {
        let id = id.map(|id| format!(r#""id":"{id}","#)).unwrap_or_default();
        format!(
            r#"{{{id}"type":"message","message":{{"role":"assistant","model":"gpt-5.2","usage":{{"input":10,"output":20,"cost":{{"total":{cost}}}}},"timestamp":1769753935279}}}}"#
        )
    }

    #[test]
    fn keeps_jsonl_lines_that_differ_only_in_cost_without_a_sqlite_copy() {
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!(
                "{}\n{}\n",
                assistant_event(None, 0.50),
                assistant_event(None, 0.49),
            ),
        });

        let entries = load_fixture(&fixture, &SharedArgs::default());

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.recorded_cost)
                .collect::<Vec<_>>(),
            [Some(0.50), Some(0.49)]
        );
    }

    #[test]
    fn prefers_sqlite_over_every_migrated_jsonl_copy() {
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!(
                "{}\n{}\n",
                assistant_event(None, 0.50),
                assistant_event(None, 0.49),
            ),
        });
        write_transcript_events(&fixture, &[&assistant_event(Some("evt-sqlite"), 0.51)]);

        let entries = load_fixture(&fixture, &SharedArgs::default());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("evt-sqlite"));
    }

    #[test]
    fn keeps_sqlite_events_with_equal_content_and_distinct_ids() {
        let fixture = fs_fixture!({});
        write_transcript_events(
            &fixture,
            &[
                &assistant_event(Some("evt-1"), 0.51),
                &assistant_event(Some("evt-2"), 0.51),
            ],
        );

        let entries = load_fixture(&fixture, &SharedArgs::default());

        let ids = entries
            .iter()
            .map(|entry| entry.message_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(ids, [Some("evt-1"), Some("evt-2")]);
    }

    #[test]
    fn drops_id_less_sqlite_event_matching_an_identified_one_in_either_order() {
        let identified = assistant_event(Some("evt-x"), 0.51);
        let id_less = assistant_event(None, 0.51);
        for events in [[&identified, &id_less], [&id_less, &identified]] {
            let fixture = fs_fixture!({});
            write_transcript_events(&fixture, &events.map(String::as_str));

            let entries = load_fixture(&fixture, &SharedArgs::default());

            let ids = entries
                .iter()
                .map(|entry| entry.message_id.as_deref())
                .collect::<Vec<_>>();
            assert_eq!(ids, [Some("evt-x")]);
        }
    }
}
