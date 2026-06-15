use std::{collections::HashSet, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};

use super::{
    parser::{KiloMessage, message_value_to_entry},
    paths::{db_path, paths},
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Kilo, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths()? {
        let Some(db_path) = db_path(&path) else {
            continue;
        };
        for entry in load_entries_from_database(&db_path, tz.as_ref(), shared, pricing) {
            if let Some(id) = entry.data.message.id.as_deref()
                && !seen.insert(id.to_string())
            {
                continue;
            }
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_entries_from_database(
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Vec<LoadedEntry> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open Kilo database: {}", db_path.display()),
        );
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare("SELECT id, session_id, data FROM message") else {
        debug_log(
            shared,
            format!("Failed to read Kilo database: {}", db_path.display()),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(row_id) = statement.read::<String, _>(0) else {
                    continue;
                };
                let Ok(row_session_id) = statement.read::<String, _>(1) else {
                    continue;
                };
                let Ok(data) = statement.read::<String, _>(2) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<KiloMessage>(&data) else {
                    continue;
                };
                if let Some(entry) = message_value_to_entry(
                    &value,
                    &row_id,
                    &row_session_id,
                    db_path,
                    tz,
                    shared.mode,
                    pricing,
                ) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!("Failed to query Kilo database: {}", db_path.display()),
                );
                break;
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{PricingMap, cli::CostMode};
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn create_db_message(path: &Path, id: &str, session_id: &str, data: &str) {
        let db = sqlite::open(path).unwrap();
        db.execute("CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)")
            .unwrap();
        let mut statement = db
            .prepare("INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)")
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, data)).unwrap();
        statement.next().unwrap();
    }

    #[test]
    fn loads_kilo_messages_from_sqlite() {
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path(super::super::paths::KILO_DB_FILE_NAME),
            "row-1",
            "session-a",
            r#"{"id":"msg-1","role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"reasoning":5,"cache":{"read":10,"write":20}},"cost":0.02,"agent":"build"}"#,
        );
        let _cleanup = EnvVarGuard::set(super::super::paths::KILO_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 5);
        assert_eq!(entries[0].cost, 0.02);
    }

    #[test]
    fn ignores_kilo_messages_without_timestamps() {
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path(super::super::paths::KILO_DB_FILE_NAME),
            "row-1",
            "session-a",
            r#"{"role":"assistant","providerID":"openai","modelID":"gpt-5","tokens":{"input":1,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        let _cleanup = EnvVarGuard::set(super::super::paths::KILO_DATA_DIR_ENV, fixture.root());
        let shared = SharedArgs::default();
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn deduplicates_kilo_messages_across_data_dirs() {
        let first = fs_fixture!({});
        let second = fs_fixture!({});
        for (fixture, input) in [(&first, 10), (&second, 20)] {
            create_db_message(
                &fixture.path(super::super::paths::KILO_DB_FILE_NAME),
                "row-1",
                "session-a",
                &format!(
                    r#"{{"id":"embedded-msg-1","role":"assistant","providerID":"openai","modelID":"gpt-5","time":{{"created":1767312000000}},"tokens":{{"input":{input},"output":1,"cache":{{"read":0,"write":0}}}}}}"#
                ),
            );
        }
        let _cleanup = EnvVarGuard::set(
            super::super::paths::KILO_DATA_DIR_ENV,
            format!("{},{}", first.root().display(), second.root().display()),
        );
        let shared = SharedArgs::default();
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
    }
}
