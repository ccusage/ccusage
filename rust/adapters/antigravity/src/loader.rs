use std::collections::HashSet;

use ccusage_adapter_common::read_files_parallel;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, parse_tz};

use super::{
    parser::{event_to_loaded, parse_sqlite_file},
    paths::conversation_db_paths,
};

/// Loads Antigravity generation metadata from all discovered conversation databases.
pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Antigravity"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let timezone = parse_tz(shared.timezone.as_deref());
    let database_paths = conversation_db_paths()?;
    let parsed = read_files_parallel(&database_paths, shared.single_thread, parse_sqlite_file);
    let mut events = Vec::new();
    let mut response_ids = HashSet::new();
    for database_events in parsed {
        for event in database_events? {
            if let Some(response_id) = event.message_id.as_deref()
                && !response_ids.insert(response_id.to_string())
            {
                continue;
            }
            events.push(event);
        }
    }
    events.sort_by_key(|event| event.timestamp);
    Ok(events
        .into_iter()
        .map(|event| event_to_loaded(event, timezone.as_ref(), shared.mode, pricing))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, Fixture};

    use super::*;
    use crate::parser::test_support::{create_database, metadata_blob};

    fn shared(single_thread: bool) -> SharedArgs {
        SharedArgs {
            mode: crate::cli::CostMode::Calculate,
            single_thread,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        }
    }

    fn load_from_fixture(fixture: &Fixture, single_thread: bool) -> Vec<LoadedEntry> {
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);
        load_entries(&shared(single_thread), &PricingMap::load_embedded()).unwrap()
    }

    #[test]
    fn loads_real_schema_rows_with_continuations_and_token_buckets() {
        let fixture = Fixture::new();
        let db_path = fixture.path("conversations/session.db");
        create_database(
            &db_path,
            &[
                (
                    2,
                    metadata_blob(
                        None,
                        (200, 300, 0, 75, 25),
                        (1_778_000_001, 0),
                        "response-2",
                    ),
                ),
                (
                    1,
                    metadata_blob(
                        Some("Gemini 3 Pro"),
                        (1_000, 400, 500, 150, 50),
                        (1_778_000_000, 123_000_000),
                        "response-1",
                    ),
                ),
            ],
        );

        let entries = load_from_fixture(&fixture, true);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3-pro"));
        assert_eq!(entries[1].model.as_deref(), Some("gemini-3-pro"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1_400);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 500);
        assert_eq!(entries[0].data.message.usage.output_tokens, 150);
        assert_eq!(entries[0].extra_total_tokens, 50);
        assert_eq!(entries[1].data.message.usage.input_tokens, 500);
        assert_eq!(entries[1].data.message.usage.output_tokens, 75);
        assert_eq!(entries[1].extra_total_tokens, 25);
        assert!(entries[0].cost > 0.0);
    }

    #[test]
    fn parallel_and_single_threaded_database_reads_match() {
        let fixture = Fixture::new();
        create_database(
            &fixture.path("one/conversations/one.db"),
            &[(
                1,
                metadata_blob(
                    Some("gemini-3-pro"),
                    (10, 20, 30, 40, 5),
                    (1_778_000_000, 0),
                    "one",
                ),
            )],
        );
        create_database(
            &fixture.path("two/conversations/two.db"),
            &[(
                1,
                metadata_blob(
                    Some("gemini-3-pro"),
                    (11, 21, 31, 41, 6),
                    (1_778_000_002, 0),
                    "two",
                ),
            )],
        );
        let override_value = format!(
            "{},{}",
            fixture.path("one").display(),
            fixture.path("two").display()
        );
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        let sequential = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();
        let parallel = load_entries(&shared(false), &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            sequential
                .iter()
                .map(|entry| entry.data.message.id.clone())
                .collect::<Vec<_>>(),
            parallel
                .iter()
                .map(|entry| entry.data.message.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(sequential.len(), 2);
    }

    #[test]
    fn deduplicates_response_ids_across_databases() {
        let fixture = Fixture::new();
        create_database(
            &fixture.path("first/conversations/first.db"),
            &[(
                1,
                metadata_blob(
                    Some("gemini-3-pro"),
                    (10, 20, 30, 40, 5),
                    (1_778_000_000, 0),
                    "shared-response",
                ),
            )],
        );
        create_database(
            &fixture.path("second/conversations/second.db"),
            &[(
                1,
                metadata_blob(
                    Some("gemini-3-pro"),
                    (100, 200, 300, 400, 50),
                    (1_778_000_001, 0),
                    "shared-response",
                ),
            )],
        );
        let override_value = format!(
            "{},{}",
            fixture.path("first").display(),
            fixture.path("second").display()
        );
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        let entries = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("shared-response")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 30);
    }

    #[test]
    fn propagates_database_open_and_query_errors() {
        let fixture = Fixture::new();
        let not_a_database = fixture.write_file("conversations/not-a-db.db", "not sqlite");
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(not_a_database.to_string_lossy().as_ref()));
        assert!(message.contains("open") || message.contains("database"));
    }

    #[test]
    fn propagates_missing_generation_table_errors() {
        let fixture = Fixture::new();
        let db_path = fixture.path("conversations/missing-table.db");
        let _ = fixture.create_dir_all("conversations");
        let connection = sqlite::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE other (id INTEGER)")
            .unwrap();
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();

        assert!(error.to_string().contains("gen_metadata"));
    }

    #[test]
    fn propagates_malformed_metadata_errors() {
        let fixture = Fixture::new();
        create_database(
            &fixture.path("conversations/malformed.db"),
            &[(1, vec![0x0a, 0x01, 0x80])],
        );
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();

        assert!(error.to_string().contains("parse Antigravity metadata"));
    }
}
