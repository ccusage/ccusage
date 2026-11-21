use ccusage_core::cost::PriceTable;
use std::{collections::HashSet, path::PathBuf};

use jiff::tz::TimeZone as JiffTimeZone;

use ccusage_adapter_common::{
    SqliteStage,
    dedupe::dedupe_first_wins,
    source::LoadContext,
    sqlite::{read_count, read_i64, sqlite_units},
    units::UnitError,
};
use ccusage_core::{Diagnostics, SkipKind};

use crate::{LoadedEntry, Result, cli::SharedArgs, debug_log};

use super::{
    parser::{RowRejection, ZcodeUsageRow, row_to_entry},
    paths::db_paths,
};

// NOTE(2026-08-31, ZCode 0.16.3, #1675): ZCode records model calls in model_usage and the project directory in session. Cache counters, provider ID and session version may be absent from compatible databases.
const REQUIRED_MODEL_COLUMNS: &[&str] = &[
    "id",
    "session_id",
    "started_at",
    "model_id",
    "status",
    "input_tokens",
    "output_tokens",
];
const REQUIRED_SESSION_COLUMNS: &[&str] = &["id", "directory"];

#[derive(Debug, Clone, Copy)]
struct Schema {
    has_provider_id: bool,
    has_cache_creation: bool,
    has_cache_read: bool,
    has_computed_total: bool,
    has_session_version: bool,
}

pub(crate) fn load_entries_for_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let db_paths = db_paths(shared)?;
    let loaded = load_entries_from_databases(&db_paths, tz, shared, pricing, cx.diagnostics);
    let mut entries: Vec<_> = dedupe_first_wins(loaded, |(id, _)| id.clone())
        .into_iter()
        .map(|(_, entry)| entry)
        .collect();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

fn load_entries_from_databases(
    db_paths: &[PathBuf],
    tz: &JiffTimeZone,
    shared: &SharedArgs,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<(String, LoadedEntry)> {
    sqlite_units(
        db_paths,
        shared.single_thread,
        diagnostics,
        |connection, db_path| prepare_usage_query(connection, db_path, shared),
        |statement, _, _| {
            row_to_entry(
                read_usage_row(statement),
                tz,
                shared.mode,
                pricing,
                &shared.pricing_overrides,
            )
            .map_err(|rejection| match rejection {
                RowRejection::InvalidIdentity => UnitError::new(
                    SkipKind::InvalidData,
                    "completed ZCode usage row has no id, session id or model",
                ),
                RowRejection::InvalidTimestamp => UnitError::new(
                    SkipKind::InvalidTimestamp,
                    "completed ZCode usage row has an invalid start time",
                ),
            })
        },
    )
}

fn prepare_usage_query<'a>(
    connection: &'a sqlite::Connection,
    db_path: &std::path::Path,
    shared: &SharedArgs,
) -> std::result::Result<sqlite::Statement<'a>, UnitError> {
    let schema = read_schema(connection)?;
    debug_log(
        shared,
        format!(
            "Reading ZCode SQLite schema at {} (cache_creation={}, cache_read={}, computed_total={}, provider_id={}, session_version={})",
            db_path.display(),
            schema.has_cache_creation,
            schema.has_cache_read,
            schema.has_computed_total,
            schema.has_provider_id,
            schema.has_session_version,
        ),
    );

    let cache_creation = if schema.has_cache_creation {
        "m.cache_creation_input_tokens"
    } else {
        "0"
    };
    let cache_read = if schema.has_cache_read {
        "m.cache_read_input_tokens"
    } else {
        "0"
    };
    let computed_total = if schema.has_computed_total {
        "m.computed_total_tokens"
    } else {
        "m.input_tokens + m.output_tokens"
    };
    let provider_id = if schema.has_provider_id {
        "m.provider_id"
    } else {
        "NULL"
    };
    let version = if schema.has_session_version {
        "s.version"
    } else {
        "NULL"
    };
    let query = format!(
        "SELECT m.id, m.session_id, m.started_at, m.model_id, m.input_tokens, m.output_tokens, {cache_creation}, {cache_read}, {computed_total}, {provider_id}, s.directory, {version} FROM model_usage m LEFT JOIN session s ON s.id = m.session_id WHERE m.status = 'completed'"
    );
    connection
        .prepare(&query)
        .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))
}

fn read_schema(connection: &sqlite::Connection) -> std::result::Result<Schema, UnitError> {
    let model_columns = table_columns(connection, "model_usage")?;
    let session_columns = table_columns(connection, "session")?;
    let missing_model = missing_columns(&model_columns, REQUIRED_MODEL_COLUMNS);
    let missing_session = missing_columns(&session_columns, REQUIRED_SESSION_COLUMNS);
    if !missing_model.is_empty() || !missing_session.is_empty() {
        let mut details = Vec::new();
        if !missing_model.is_empty() {
            details.push(format!("model_usage: {}", missing_model.join(", ")));
        }
        if !missing_session.is_empty() {
            details.push(format!("session: {}", missing_session.join(", ")));
        }
        return Err(UnitError::new(
            SkipKind::Invariant,
            format!(
                "database schema is not supported: missing {}",
                details.join("; ")
            ),
        ));
    }

    Ok(Schema {
        has_provider_id: model_columns.contains("provider_id"),
        has_cache_creation: model_columns.contains("cache_creation_input_tokens"),
        has_cache_read: model_columns.contains("cache_read_input_tokens"),
        has_computed_total: model_columns.contains("computed_total_tokens"),
        has_session_version: session_columns.contains("version"),
    })
}

fn table_columns(
    connection: &sqlite::Connection,
    table: &str,
) -> std::result::Result<HashSet<String>, UnitError> {
    let statement = connection
        .prepare(format!("SELECT * FROM \"{table}\" LIMIT 0"))
        .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))?;
    Ok(statement.column_names().iter().cloned().collect())
}

fn missing_columns(columns: &HashSet<String>, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|column| !columns.contains(**column))
        .map(|column| (*column).to_string())
        .collect()
}

fn read_usage_row(statement: &sqlite::Statement<'_>) -> ZcodeUsageRow {
    ZcodeUsageRow {
        id: read_text_column(statement, 0),
        session_id: read_text_column(statement, 1),
        started_at: read_i64(statement, 2),
        model_id: read_text_column(statement, 3),
        input_tokens: read_count(statement, 4),
        output_tokens: read_count(statement, 5),
        cache_creation_input_tokens: read_count(statement, 6),
        cache_read_input_tokens: read_count(statement, 7),
        computed_total_tokens: read_count(statement, 8),
        provider_id: read_text_column(statement, 9),
        directory: read_text_column(statement, 10),
        version: read_text_column(statement, 11),
    }
}

fn read_text_column(statement: &sqlite::Statement<'_>, index: usize) -> Option<String> {
    statement.read::<Option<String>, _>(index).ok().flatten()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ccusage_adapter_common::source::{EntrySource, LoadContext, SourceOptions};
    use ccusage_core::{ReportWindow, cli::AgentReportKind};
    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;
    use crate::cli::CostMode;

    fn create_db(path: &Path, legacy: bool) {
        let db = sqlite::open(path).unwrap();
        if legacy {
            db.execute(
                "CREATE TABLE model_usage (
                    id TEXT PRIMARY KEY, session_id TEXT, started_at INTEGER, model_id TEXT,
                    status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_input_tokens INTEGER, computed_total_tokens INTEGER
                )",
            )
            .unwrap();
            db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT)")
                .unwrap();
        } else {
            db.execute(
                "CREATE TABLE model_usage (
                    id TEXT PRIMARY KEY, session_id TEXT, started_at INTEGER, model_id TEXT,
                    provider_id TEXT, status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                    cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                    computed_total_tokens INTEGER
                )",
            )
            .unwrap();
            db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, version TEXT)")
                .unwrap();
        }
    }

    fn insert_current_usage(path: &Path, id: &str, status: &str) {
        let db = sqlite::open(path).unwrap();
        insert_current_usage_on_connection(&db, id, status);
    }

    fn insert_current_usage_on_connection(db: &sqlite::Connection, id: &str, status: &str) {
        let mut statement = db
            .prepare(
                "INSERT INTO model_usage
                 (id, session_id, started_at, model_id, provider_id, status, input_tokens,
                  output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
                  computed_total_tokens)
                 VALUES (?1, 'session-1', 1786909042666, 'GLM-5.3',
                         'builtin:zai-coding-plan', ?2, 100, 10, 15, 25, 110)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, status)).unwrap();
        statement.next().unwrap();
    }

    fn insert_usage_record(
        db: &sqlite::Connection,
        id: &str,
        session_id: &str,
        started_at: i64,
        model_id: &str,
        usage: [i64; 5],
    ) {
        let [
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            computed_total_tokens,
        ] = usage;
        let mut statement = db
            .prepare(
                "INSERT INTO model_usage
                 (id, session_id, started_at, model_id, provider_id, status, input_tokens,
                  output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
                  computed_total_tokens)
                 VALUES (?1, ?2, ?3, ?4, 'builtin:zai-coding-plan', 'completed',
                         ?5, ?6, ?7, ?8, ?9)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, started_at)).unwrap();
        statement.bind((4, model_id)).unwrap();
        statement.bind((5, input_tokens)).unwrap();
        statement.bind((6, output_tokens)).unwrap();
        statement.bind((7, cache_creation_input_tokens)).unwrap();
        statement.bind((8, cache_read_input_tokens)).unwrap();
        statement.bind((9, computed_total_tokens)).unwrap();
        statement.next().unwrap();
    }

    fn journal_mode(db: &sqlite::Connection) -> String {
        let mut statement = db.prepare("PRAGMA journal_mode").unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);
        statement.read::<String, _>(0).unwrap()
    }

    fn load_with_mode(root: &Path, mode: CostMode) -> Vec<LoadedEntry> {
        Environment::with_var(super::super::paths::ZCODE_HOME_ENV, root).run(|| {
            let shared = SharedArgs {
                mode,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap()
        })
    }

    fn load(root: &Path) -> Vec<LoadedEntry> {
        load_with_mode(root, CostMode::Display)
    }

    #[test]
    fn loads_completed_rows_with_millisecond_precision() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        fixture.create_dir_all("cli/db");
        let db_path = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_path, false);
        let db = sqlite::open(&db_path).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage(&db_path, "usage-1", "completed");
        insert_current_usage(&db_path, "usage-2", "running");
        drop(db);
        let entries = load(fixture.root());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("usage-1"));
        assert_eq!(
            entries[0].timestamp,
            ccusage_core::parse_ts_timestamp("2026-08-16T19:37:22.666Z").unwrap()
        );
        assert_eq!(entries[0].project_path.as_ref(), "/project");
    }

    #[test]
    fn keeps_healthy_databases_when_another_database_has_no_supported_schema() {
        ccusage_test_support::install_pricing_snapshots();
        let broken = fs_fixture!({
            "cli/db/db.sqlite": "not a ZCode database",
        });
        let healthy = fs_fixture!({});
        healthy.create_dir_all("cli/db");
        let database = healthy.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&database, false);
        let db = sqlite::open(&database).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage(&database, "usage-1", "completed");
        drop(db);
        Environment::with_var(
            super::super::paths::ZCODE_HOME_ENV,
            format!("{},{}", broken.root().display(), healthy.root().display()),
        )
        .run(|| {
            let shared = SharedArgs {
                single_thread: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                ccusage_adapter_common::source::SourceOptions::default(),
                Some(&ccusage_core::PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].message_id.as_deref(), Some("usage-1"));
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        })
    }

    #[test]
    fn separates_invalid_databases_from_unsupported_schemas() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({"broken.db": "SECRET_CANARY"});
        sqlite::open(fixture.path("unsupported.db"))
            .unwrap()
            .execute("CREATE TABLE other (id TEXT)")
            .unwrap();
        let diagnostics = Diagnostics::new("zcode");
        let shared = SharedArgs::default();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let loaded = load_entries_from_databases(
            &[fixture.path("broken.db"), fixture.path("unsupported.db")],
            &jiff::tz::TimeZone::UTC,
            &shared,
            PriceTable::for_map(shared.mode, &pricing),
            &diagnostics,
        );

        assert!(loaded.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    }

    #[test]
    fn names_the_missing_columns_of_an_incomplete_schema() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        let path = fixture.path("incomplete.db");
        let db = sqlite::open(&path).unwrap();
        db.execute("CREATE TABLE model_usage (id TEXT, session_id TEXT)")
            .unwrap();
        db.execute("CREATE TABLE session (id TEXT)").unwrap();
        drop(db);

        let diagnostics = Diagnostics::new("zcode");
        load_entries_from_databases(
            std::slice::from_ref(&path),
            &jiff::tz::TimeZone::UTC,
            &SharedArgs::default(),
            PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
            &diagnostics,
        );

        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(
            warnings.contains(
                "database schema is not supported: missing model_usage: started_at, model_id, \
                 status, input_tokens, output_tokens; session: directory"
            ),
            "{warnings}"
        );
    }

    #[test]
    fn diagnoses_rejected_completed_usage_and_keeps_healthy_rows() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        let path = fixture.path("usage.db");
        create_db(&path, false);
        insert_current_usage(&path, "healthy", "completed");
        insert_current_usage(&path, "broken", "completed");
        sqlite::open(&path)
            .unwrap()
            .execute("UPDATE model_usage SET model_id = NULL WHERE id = 'broken'")
            .unwrap();
        let diagnostics = Diagnostics::new("zcode");
        let entries = load_entries_from_databases(
            std::slice::from_ref(&path),
            &jiff::tz::TimeZone::UTC,
            &SharedArgs::default(),
            PriceTable::Auto(&ccusage_core::PricingMap::load_embedded()),
            &diagnostics,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn diagnoses_malformed_usage_identity_only_when_the_row_has_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        fixture.create_dir_all("cli/db");
        let db_path = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_path, false);
        let db = sqlite::open(&db_path).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage_on_connection(&db, "healthy", "completed");
        insert_usage_record(
            &db,
            "",
            "session-1",
            1_786_909_042_667,
            "GLM-5.3",
            [100, 10, 15, 25, 110],
        );
        insert_usage_record(
            &db,
            "blank-session",
            "",
            1_786_909_042_668,
            "GLM-5.3",
            [100, 10, 15, 25, 110],
        );
        insert_usage_record(
            &db,
            "blank-model",
            "session-1",
            1_786_909_042_669,
            "",
            [100, 10, 15, 25, 110],
        );
        insert_usage_record(
            &db,
            "bad-timestamp",
            "session-1",
            0,
            "GLM-5.3",
            [100, 10, 15, 25, 110],
        );
        insert_usage_record(&db, "zero-token", "session-1", 0, "", [0; 5]);
        db.execute(
            "INSERT INTO model_usage
             (id, session_id, started_at, model_id, status, input_tokens, output_tokens)
             VALUES ('null-model', 'session-1', 1786909042670, NULL, 'completed', 100, 10),
                    ('zero-token-null-identity', NULL, NULL, NULL, 'completed', 0, 0)",
        )
        .unwrap();
        drop(db);

        Environment::with_var(super::super::paths::ZCODE_HOME_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                single_thread: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let window = ReportWindow::from_shared(&shared);
            let pricing = ccusage_core::PricingMap::load_embedded();
            let diagnostics = Diagnostics::new("zcode");
            let cx = LoadContext {
                shared: &shared,
                kind: AgentReportKind::Daily,
                window: &window,
                pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
                options: SourceOptions::default(),
                diagnostics: &diagnostics,
            };

            let entries = crate::SOURCE.load_entries(&cx).unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].message_id.as_deref(), Some("healthy"));
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 4);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
            let warnings = diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                })
                .join("\n");
            assert!(!warnings.contains("blank-session"));
            assert!(!warnings.contains("blank-model"));
            assert!(!warnings.contains("bad-timestamp"));
        })
    }

    #[test]
    fn preserves_opaque_provider_for_loader_pricing() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        fixture.create_dir_all("cli/db");
        let db_path = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_path, false);
        let db = sqlite::open(&db_path).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        db.execute(
            "INSERT INTO model_usage
             VALUES ('usage-opaque-provider', 'session-1', 1786909042666, 'GLM-5.3',
                     '847d13c9-0568-4f2f-818e-8bd498e5d920', 'completed', 100, 10, 15, 25, 110)",
        )
        .unwrap();
        drop(db);

        let entries = load_with_mode(fixture.root(), CostMode::Calculate);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.0);
        assert_eq!(entries[0].missing_pricing_model.as_deref(), Some("GLM-5.3"));
    }

    #[test]
    fn accepts_legacy_schema_without_optional_columns() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        fixture.create_dir_all("cli/db");
        let db_path = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_path, true);
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "INSERT INTO model_usage
             VALUES ('usage-legacy', 'session-legacy', 1786909042666, 'custom-model',
                     'completed', 100, 10, 25, 110)",
        )
        .unwrap();
        db.execute("INSERT INTO session VALUES ('session-legacy', '')")
            .unwrap();
        drop(db);

        let entries = load(fixture.root());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, None);
        assert_eq!(entries[0].project_path.as_ref(), "ZCode");
        assert_eq!(entries[0].usage.input_tokens, 75);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 25);
    }

    #[test]
    fn reads_from_wal_database_without_changing_journal_mode() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        fixture.create_dir_all("cli/db");
        let db_path = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_path, false);

        let db = sqlite::open(&db_path).unwrap();
        db.execute("PRAGMA journal_mode = WAL").unwrap();
        db.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        assert_eq!(journal_mode(&db), "wal");
        db.execute("BEGIN").unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage_on_connection(&db, "usage-1", "completed");
        db.execute("COMMIT").unwrap();
        assert!(db_path.with_extension("sqlite-wal").is_file());

        let entries = load(fixture.root());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("usage-1"));
        assert_eq!(journal_mode(&db), "wal");
    }

    #[test]
    fn skips_unrelated_schema_and_deduplicates_multiple_homes() {
        ccusage_test_support::install_pricing_snapshots();
        let first = fs_fixture!({});
        first.create_dir_all("cli/db");
        let first_db = first.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&first_db, false);
        let db = sqlite::open(&first_db).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage(&first_db, "usage-1", "completed");
        drop(db);

        let second = fs_fixture!({});
        second.create_dir_all("cli/db");
        let second_db = second.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        std::fs::copy(&first_db, &second_db).unwrap();
        let db = sqlite::open(&second_db).unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO model_usage
                 (id, session_id, started_at, model_id, provider_id, status, input_tokens,
                  output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
                  computed_total_tokens)
                 VALUES ('usage-2', 'session-1', 1786909043666, 'GLM-5.3',
                         'builtin:zai-coding-plan', 'completed', 50, 5, 0, 0, 55)",
            )
            .unwrap();
        statement.next().unwrap();
        drop(statement);
        drop(db);

        Environment::with_var(
            super::super::paths::ZCODE_HOME_ENV,
            format!("{},{}", first.root().display(), second.root().display()),
        )
        .run(|| {
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
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].message_id.as_deref(), Some("usage-1"));
            assert_eq!(entries[1].message_id.as_deref(), Some("usage-2"));
        });

        let invalid = fs_fixture!({});
        invalid.create_dir_all("cli/db");
        let invalid_db = invalid.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        let db = sqlite::open(&invalid_db).unwrap();
        db.execute("CREATE TABLE unrelated (id TEXT)").unwrap();
        drop(db);
        assert!(load(invalid.root()).is_empty());
    }
}
