use std::{collections::HashSet, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use ccusage_adapter_common::read_files_parallel;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};

use super::{
    parser::{ZcodeUsageRow, row_to_entry},
    paths::db_paths,
};

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
    version: SchemaVersion,
    has_provider_id: bool,
    has_cache_creation: bool,
    has_cache_read: bool,
    has_computed_total: bool,
    has_session_version: bool,
}

#[derive(Debug, Clone, Copy)]
enum SchemaVersion {
    Legacy,
    SessionVersioned,
}

impl SchemaVersion {
    fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::SessionVersioned => "session-versioned",
        }
    }
}

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("ZCode"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let db_paths = db_paths(shared)?;
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_entries_from_database(db_path, tz.as_ref(), shared, pricing)
    });

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for db_entries in loaded {
        for entry in db_entries {
            let Some(id) = entry.data.message.id.as_deref() else {
                entries.push(entry);
                continue;
            };
            if seen.insert(id.to_string()) {
                entries.push(entry);
            }
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
            format!("Failed to open ZCode database: {}", db_path.display()),
        );
        return Vec::new();
    };

    if let Err(error) = connection.execute("PRAGMA busy_timeout = 5000") {
        debug_log(
            shared,
            format!("Failed to configure ZCode database locking: {error}"),
        );
        return Vec::new();
    }
    if let Err(error) = connection.execute("PRAGMA query_only = ON") {
        debug_log(
            shared,
            format!("Failed to configure ZCode read-only mode: {error}"),
        );
        return Vec::new();
    }

    let Some(schema) = read_schema(&connection, db_path, shared) else {
        return Vec::new();
    };
    debug_log(
        shared,
        format!(
            "Reading ZCode SQLite schema at {} (layout={}, cache_creation={}, cache_read={}, computed_total={}, provider_id={}, session_version={})",
            db_path.display(),
            schema.version.label(),
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
        "CAST(m.provider_id AS TEXT)"
    } else {
        "NULL"
    };
    let version = if schema.has_session_version {
        "CAST(s.version AS TEXT)"
    } else {
        "NULL"
    };
    let query = format!(
        "SELECT m.id, m.session_id, m.started_at, m.model_id, m.input_tokens, m.output_tokens, {cache_creation}, {cache_read}, {computed_total}, {provider_id}, s.directory, {version} FROM model_usage m LEFT JOIN session s ON s.id = m.session_id WHERE m.status = 'completed'"
    );
    let Ok(mut statement) = connection.prepare(&query) else {
        debug_log(
            shared,
            format!("Failed to query ZCode database: {}", db_path.display()),
        );
        return Vec::new();
    };

    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                if let Some(entry) = read_usage_row(&statement).and_then(|row| {
                    row_to_entry(row, tz, shared.mode, pricing, &shared.pricing_overrides)
                }) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(error) => {
                debug_log(
                    shared,
                    format!(
                        "Failed to read ZCode database {}: {error}",
                        db_path.display()
                    ),
                );
                break;
            }
        }
    }
    entries
}

fn read_schema(
    connection: &sqlite::Connection,
    db_path: &Path,
    shared: &SharedArgs,
) -> Option<Schema> {
    let model_columns = match table_columns(connection, "model_usage") {
        Ok(columns) => columns,
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to inspect ZCode model_usage schema at {}: {error}",
                    db_path.display()
                ),
            );
            return None;
        }
    };
    let session_columns = match table_columns(connection, "session") {
        Ok(columns) => columns,
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to inspect ZCode session schema at {}: {error}",
                    db_path.display()
                ),
            );
            return None;
        }
    };
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
        debug_log(
            shared,
            format!(
                "Unsupported ZCode SQLite schema at {}: missing {}",
                db_path.display(),
                details.join("; ")
            ),
        );
        return None;
    }

    Some(Schema {
        version: if session_columns.contains("version") {
            SchemaVersion::SessionVersioned
        } else {
            SchemaVersion::Legacy
        },
        has_provider_id: model_columns.contains("provider_id"),
        has_cache_creation: model_columns.contains("cache_creation_input_tokens"),
        has_cache_read: model_columns.contains("cache_read_input_tokens"),
        has_computed_total: model_columns.contains("computed_total_tokens"),
        has_session_version: session_columns.contains("version"),
    })
}

fn table_columns(connection: &sqlite::Connection, table: &str) -> sqlite::Result<HashSet<String>> {
    // LIMIT 0 obtains column metadata without materializing any row values,
    // including prompt or content fields that may be present in future schemas.
    let query = format!("SELECT * FROM \"{table}\" LIMIT 0");
    let statement = connection.prepare(query)?;
    Ok(statement.column_names().iter().cloned().collect())
}

fn missing_columns(columns: &HashSet<String>, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|column| !columns.contains(**column))
        .map(|column| (*column).to_string())
        .collect()
}

fn read_usage_row(statement: &sqlite::Statement<'_>) -> Option<ZcodeUsageRow> {
    Some(ZcodeUsageRow {
        id: statement.read::<String, _>(0).ok()?,
        session_id: statement.read::<String, _>(1).ok()?,
        started_at: read_timestamp_ms(statement, 2)?,
        model_id: statement.read::<String, _>(3).ok()?,
        input_tokens: read_token_column(statement, 4),
        output_tokens: read_token_column(statement, 5),
        cache_creation_input_tokens: read_token_column(statement, 6),
        cache_read_input_tokens: read_token_column(statement, 7),
        computed_total_tokens: read_token_column(statement, 8),
        provider_id: statement.read::<Option<String>, _>(9).ok().flatten(),
        directory: statement.read::<Option<String>, _>(10).ok().flatten(),
        version: statement.read::<Option<String>, _>(11).ok().flatten(),
    })
}

fn read_token_column(statement: &sqlite::Statement<'_>, index: usize) -> u64 {
    statement
        .read::<i64, _>(index)
        .map(|value| value.max(0) as u64)
        .or_else(|_| {
            statement.read::<f64, _>(index).map(|value| {
                if value.is_finite() && value > 0.0 {
                    value.min(u64::MAX as f64).round() as u64
                } else {
                    0
                }
            })
        })
        .unwrap_or(0)
}

fn read_timestamp_ms(statement: &sqlite::Statement<'_>, index: usize) -> Option<i64> {
    if let Ok(value) = statement.read::<i64, _>(index) {
        return Some(value);
    }
    let value = statement.read::<f64, _>(index).ok()?;
    (value.is_finite() && value >= i64::MIN as f64 && value <= i64::MAX as f64)
        .then_some(value.round() as i64)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::*;
    use crate::{PricingMap, cli::CostMode};

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

    fn load(root: &Path) -> Vec<LoadedEntry> {
        let _guard = EnvVarGuard::set(super::super::paths::ZCODE_HOME_ENV, root);
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        load_entries(&shared, &PricingMap::load_embedded()).unwrap()
    }

    #[test]
    fn loads_completed_rows_with_millisecond_precision() {
        let fixture = fs_fixture!({});
        let _ = fixture.create_dir_all("cli/db");
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
        assert_eq!(entries[0].data.message.id.as_deref(), Some("usage-1"));
        assert_eq!(entries[0].data.timestamp, "2026-08-16T19:37:22.666Z");
        assert_eq!(entries[0].project_path.as_ref(), "/project");
    }

    #[test]
    fn accepts_legacy_schema_without_optional_columns() {
        let fixture = fs_fixture!({});
        let _ = fixture.create_dir_all("cli/db");
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
        assert_eq!(entries[0].data.version, None);
        assert_eq!(entries[0].project_path.as_ref(), "ZCode");
        assert_eq!(entries[0].data.message.usage.input_tokens, 75);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 25);
    }

    #[test]
    fn skips_unrelated_schema_and_deduplicates_multiple_homes() {
        let first = fs_fixture!({});
        let _ = first.create_dir_all("cli/db");
        let first_db = first.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&first_db, false);
        let db = sqlite::open(&first_db).unwrap();
        db.execute("INSERT INTO session VALUES ('session-1', '/project', '0.16.3')")
            .unwrap();
        insert_current_usage(&first_db, "usage-1", "completed");
        drop(db);

        let second = fs_fixture!({});
        let _ = second.create_dir_all("cli/db");
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

        let _guard = EnvVarGuard::set(
            super::super::paths::ZCODE_HOME_ENV,
            format!("{},{}", first.root().display(), second.root().display()),
        );
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("usage-1"));
        assert_eq!(entries[1].data.message.id.as_deref(), Some("usage-2"));
        drop(_guard);

        let invalid = fs_fixture!({});
        let _ = invalid.create_dir_all("cli/db");
        let invalid_db = invalid.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        let db = sqlite::open(&invalid_db).unwrap();
        db.execute("CREATE TABLE unrelated (id TEXT)").unwrap();
        drop(db);
        assert!(load(invalid.root()).is_empty());
    }
}
