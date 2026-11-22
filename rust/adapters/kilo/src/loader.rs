use ccusage_core::cost::PriceTable;
use std::path::PathBuf;

use ccusage_adapter_common::{
    SqliteStage, UnitError, dedupe::dedupe_first_wins, jsonl, source::LoadContext,
    sqlite::sqlite_units,
};
use ccusage_core::SkipKind;

use crate::{LoadedEntry, Result};

use super::{
    parser::{KiloMessage, MissingTimestamp, message_value_to_entry},
    paths::{db_path, paths},
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let db_paths: Vec<PathBuf> = paths(cx.diagnostics, cx.shared.debug)
        .iter()
        .filter_map(|path| db_path(path))
        .collect();
    let pricing = cx.pricing.table(cx.shared.mode);
    let loaded = load_entries_from_databases(&db_paths, cx, pricing);
    let mut entries = dedupe_first_wins(loaded, |(id, _)| id.clone())
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

fn load_entries_from_databases(
    db_paths: &[PathBuf],
    cx: &LoadContext<'_>,
    pricing: PriceTable<'_>,
) -> Vec<(String, LoadedEntry)> {
    sqlite_units(
        db_paths,
        cx.shared.single_thread,
        cx.diagnostics,
        |connection, _| {
            connection
                .prepare("SELECT id, session_id, data FROM message")
                .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))
        },
        |statement, db_path, row_number| {
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
            let (row_id, row_session_id, data) = row.map_err(|_| {
                UnitError::new(SkipKind::InvalidData, "Kilo message row has invalid fields")
            })?;
            let Some(value) = jsonl::decode_record::<KiloMessage>(
                data.as_bytes(),
                db_path,
                row_number,
                None,
                cx.diagnostics,
                Some(|value| {
                    value.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                        && value.get("tokens").is_some()
                }),
            ) else {
                return Ok(None);
            };
            message_value_to_entry(
                &value,
                &row_id,
                &row_session_id,
                db_path,
                cx.window.timezone(),
                cx.shared.mode,
                pricing,
            )
            .map_err(|MissingTimestamp| {
                UnitError::new(
                    SkipKind::InvalidTimestamp,
                    "Kilo usage record has no valid creation time",
                )
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::cli::{CostMode, SharedArgs};
    use ccusage_test_support::{Environment, fs_fixture};

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
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path(super::super::paths::KILO_DB_FILE_NAME),
            "row-1",
            "session-a",
            r#"{"id":"msg-1","role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"reasoning":5,"cache":{"read":10,"write":20}},"cost":0.02,"agent":"build"}"#,
        );
        Environment::with_var(super::super::paths::KILO_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-01-02");
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(
                entries[0].model.as_deref(),
                Some("claude-sonnet-4-20250514")
            );
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
            assert_eq!(entries[0].extra_total_tokens, 5);
            assert_eq!(entries[0].cost, 0.02);
        })
    }

    #[test]
    fn ignores_kilo_messages_without_timestamps() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path(super::super::paths::KILO_DB_FILE_NAME),
            "row-1",
            "session-a",
            r#"{"role":"assistant","providerID":"openai","modelID":"gpt-5","tokens":{"input":1,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        Environment::with_var(super::super::paths::KILO_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs::default();
            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                ccusage_adapter_common::source::SourceOptions::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap();

            assert!(entries.is_empty());
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
                1
            );
        })
    }

    #[test]
    fn deduplicates_kilo_messages_across_data_dirs() {
        ccusage_test_support::install_pricing_snapshots();
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
        Environment::with_var(
            super::super::paths::KILO_DATA_DIR_ENV,
            format!("{},{}", first.root().display(), second.root().display()),
        )
        .run(|| {
            let shared = SharedArgs::default();
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].usage.input_tokens, 10);
        })
    }

    #[test]
    fn keeps_healthy_database_entries_when_another_database_has_the_wrong_schema() {
        ccusage_test_support::install_pricing_snapshots();
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
        Environment::with_var(
            super::super::paths::KILO_DATA_DIR_ENV,
            format!("{},{}", broken.root().display(), healthy.root().display()),
        )
        .run(|| {
            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                ccusage_adapter_common::source::SourceOptions::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::Invariant),
                1,
                "the database without a message table is reported"
            );
        })
    }

    #[test]
    fn classifies_database_failures_by_their_sqlite_cause() {
        use ccusage_adapter_common::source::{LoadContext, SourceOptions};
        use ccusage_core::{
            Diagnostics, ReportWindow, SkipKind,
            cli::{AgentReportKind, SharedArgs},
        };

        let fixture = fs_fixture!({"not-a-database.db": "not a SQLite database"});
        let wrong_schema = fixture.path("wrong-schema.db");
        sqlite::open(&wrong_schema)
            .unwrap()
            .execute("CREATE TABLE other (id TEXT)")
            .unwrap();
        let shared = SharedArgs::default();
        let window = ReportWindow::from_shared(&shared);
        let pricing = ccusage_core::PricingMap::default();

        for (path, kind) in [
            (wrong_schema, SkipKind::Invariant),
            (fixture.path("not-a-database.db"), SkipKind::InvalidData),
        ] {
            let diagnostics = Diagnostics::new("kilo");
            let cx = LoadContext {
                shared: &shared,
                kind: AgentReportKind::Daily,
                window: &window,
                pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
                options: SourceOptions::default(),
                diagnostics: &diagnostics,
            };
            let entries = super::load_entries_from_databases(
                std::slice::from_ref(&path),
                &cx,
                ccusage_core::cost::PriceTable::for_map(cx.shared.mode, &pricing),
            );
            assert!(entries.is_empty());

            assert_eq!(diagnostics.skipped(kind), 1, "{}", path.display());
        }
    }

    #[test]
    fn debug_diagnostics_do_not_echo_malformed_message_values() {
        ccusage_test_support::install_pricing_snapshots();
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
        Environment::with_var(super::super::paths::KILO_DATA_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::load_embedded();
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
                pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
                options: SourceOptions::default(),
                diagnostics: &diagnostics,
            };

            let entries = crate::SOURCE.load_entries(&cx).unwrap();
            let warnings = diagnostics.warnings(&shared).join("\n");

            assert!(entries.is_empty());
            assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
            assert!(warnings.contains("invalid data"));
            assert!(!warnings.contains("SECRET_CANARY"));
        })
    }
}
