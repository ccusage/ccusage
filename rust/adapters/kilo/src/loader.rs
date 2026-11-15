use std::{collections::HashSet, path::Path, path::PathBuf};

use ccusage_adapter_common::{UnitError, read_units, source::LoadContext};
use ccusage_core::{Skip, SkipKind};

use crate::{LoadedEntry, PricingMap, Result};

use super::{
    parser::{KiloMessage, message_value_to_entry},
    paths::{db_path, paths},
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let db_paths: Vec<PathBuf> = paths()?.iter().filter_map(|path| db_path(path)).collect();
    let pricing = cx
        .pricing
        .ok_or_else(|| crate::cli_error("Kilo pricing was not supplied"))?;
    // Load each database in parallel (a fresh read-only connection per DB), then
    // run the sequential id dedup over the original path order so the surviving
    // record per id matches the single-threaded read.
    let loaded = read_units(
        &db_paths,
        cx.shared.single_thread,
        cx.diagnostics,
        |db_path| load_entries_from_database(db_path, cx, pricing),
    );
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for db_entries in loaded.into_iter().flatten() {
        for entry in db_entries {
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
    cx: &LoadContext<'_>,
    pricing: &PricingMap,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let connection =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
            .map_err(|_| UnitError::new(SkipKind::Unreadable, "could not open Kilo database"))?;
    let mut statement = connection
        .prepare("SELECT id, session_id, data FROM message")
        .map_err(|_| UnitError::new(SkipKind::Invariant, "Kilo database has no message table"))?;
    let mut entries = Vec::new();
    let mut row_number = 0_u32;
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                row_number = row_number.saturating_add(1);
                let row = statement
                    .read::<String, _>(0)
                    .and_then(|row_id| {
                        statement
                            .read::<String, _>(1)
                            .map(|session| (row_id, session))
                    })
                    .and_then(|(row_id, session)| {
                        statement
                            .read::<String, _>(2)
                            .map(|data| (row_id, session, data))
                    });
                let Ok((row_id, row_session_id, data)) = row else {
                    cx.diagnostics.record(Skip::record(
                        db_path,
                        row_number,
                        SkipKind::InvalidData,
                        "Kilo message row has invalid fields",
                    ));
                    continue;
                };
                let value = match serde_json::from_str::<KiloMessage>(&data) {
                    Ok(value) => value,
                    Err(error) => {
                        let error = UnitError::from(error);
                        cx.diagnostics.record(Skip::record(
                            db_path,
                            row_number,
                            SkipKind::UndecodedUsage,
                            error,
                        ));
                        continue;
                    }
                };
                if let Some(entry) = message_value_to_entry(
                    &value,
                    &row_id,
                    &row_session_id,
                    db_path,
                    Some(cx.window.timezone()),
                    cx.shared.mode,
                    pricing,
                ) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                cx.diagnostics.record(Skip::unit(
                    db_path,
                    SkipKind::ReadInterrupted,
                    "Kilo database query stopped before all rows were read",
                ));
                break;
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        PricingMap,
        cli::{CostMode, SharedArgs},
    };
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
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();

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
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();

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
        let entries = crate::load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
    }

    #[test]
    fn keeps_healthy_database_entries_when_another_database_has_the_wrong_schema() {
        let broken = fs_fixture!({});
        sqlite::open(broken.path(super::super::paths::KILO_DB_FILE_NAME))
            .unwrap()
            .execute("CREATE TABLE other (id TEXT)")
            .unwrap();
        let healthy = fs_fixture!({});
        create_db_message(
            &healthy.path(super::super::paths::KILO_DB_FILE_NAME),
            "row-1",
            "session-a",
            r#"{"id":"msg-1","role":"assistant","providerID":"openai","modelID":"gpt-5","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"cache":{"read":0,"write":0}}}"#,
        );
        let _cleanup = EnvVarGuard::set(
            super::super::paths::KILO_DATA_DIR_ENV,
            format!("{},{}", broken.root().display(), healthy.root().display()),
        );

        let entries =
            crate::load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
    }

    #[test]
    fn debug_diagnostics_do_not_echo_malformed_message_values() {
        use ccusage_adapter_common::source::{EntrySource, LoadContext, SourceOptions};
        use ccusage_core::{
            Diagnostics, ReportWindow,
            cli::{AgentReportKind, SharedArgs},
        };

        let fixture = fs_fixture!({});
        let db_path = fixture.path(super::super::paths::KILO_DB_FILE_NAME);
        create_db_message(
            &db_path,
            "row-1",
            "session-a",
            r#"{"role":,"id":"SECRET_CANARY"}"#,
        );
        let _cleanup = EnvVarGuard::set(super::super::paths::KILO_DATA_DIR_ENV, fixture.root());
        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        let window = ReportWindow::from_shared(&shared);
        let diagnostics = Diagnostics::new("kilo");
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: Some(&pricing),
            options: SourceOptions::default(),
            diagnostics: &diagnostics,
        };

        let entries = crate::SOURCE.load_entries(&cx).unwrap();
        let warnings = diagnostics.warnings(&shared).join("\n");

        assert!(entries.is_empty());
        assert!(warnings.contains("undecoded usage"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }
}
