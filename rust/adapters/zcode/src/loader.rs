use std::{collections::HashSet, path::Path, path::PathBuf};

use ccusage_adapter_common::read_files_parallel;
use jiff::tz::TimeZone as JiffTimeZone;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};

use super::{
    parser::{ZcodeUsageRow, row_to_entry},
    paths::{db_path, paths},
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("ZCode"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let db_paths: Vec<PathBuf> = paths()?.iter().filter_map(|path| db_path(path)).collect();
    // Load each database in parallel (a fresh read-only connection per DB), then
    // run the sequential id dedup over the original path order so the surviving
    // record per id matches the single-threaded read.
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_entries_from_database(db_path, tz.as_ref(), shared, pricing)
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for db_entries in loaded {
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
    let Ok(mut statement) = connection.prepare(
        "SELECT m.id, m.session_id, m.started_at, m.model_id, \
         m.input_tokens, m.output_tokens, m.cache_creation_input_tokens, \
         m.cache_read_input_tokens, \
         m.computed_total_tokens, s.directory \
         FROM model_usage m LEFT JOIN session s ON s.id = m.session_id \
         WHERE m.status = 'completed'",
    ) else {
        debug_log(
            shared,
            format!("Failed to read ZCode database: {}", db_path.display()),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let row = read_usage_row(&statement);
                if let Some(entry) = row.and_then(|row| row_to_entry(row, tz, shared.mode, pricing))
                {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!("Failed to query ZCode database: {}", db_path.display()),
                );
                break;
            }
        }
    }
    entries
}

fn read_usage_row(statement: &sqlite::Statement<'_>) -> Option<ZcodeUsageRow> {
    Some(ZcodeUsageRow {
        id: statement.read::<String, _>(0).ok()?,
        session_id: statement.read::<String, _>(1).ok()?,
        started_at: statement.read::<i64, _>(2).ok()?,
        model_id: statement.read::<String, _>(3).ok()?,
        input_tokens: read_token_column(statement, 4),
        output_tokens: read_token_column(statement, 5),
        cache_creation_input_tokens: read_token_column(statement, 6),
        cache_read_input_tokens: read_token_column(statement, 7),
        computed_total_tokens: read_token_column(statement, 8),
        directory: statement.read::<Option<String>, _>(9).ok().flatten(),
    })
}

fn read_token_column(statement: &sqlite::Statement<'_>, index: usize) -> u64 {
    statement
        .read::<i64, _>(index)
        .map_or(0, |value| value.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{PricingMap, cli::CostMode};
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn create_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "CREATE TABLE model_usage (
                id TEXT PRIMARY KEY, session_id TEXT, started_at INTEGER, model_id TEXT,
                status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                reasoning_tokens INTEGER, cache_creation_input_tokens INTEGER,
                cache_read_input_tokens INTEGER, computed_total_tokens INTEGER
            )",
        )
        .unwrap();
        db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT)")
            .unwrap();
    }

    struct UsageCounts {
        input_tokens: i64,
        output_tokens: i64,
        cache_creation: i64,
        cache_read: i64,
        computed_total: i64,
    }

    fn insert_usage(
        path: &Path,
        id: &str,
        session_id: &str,
        started_at: i64,
        status: &str,
        counts: UsageCounts,
    ) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO model_usage (id, session_id, started_at, model_id, status,
                 input_tokens, output_tokens, reasoning_tokens,
                 cache_creation_input_tokens, cache_read_input_tokens, computed_total_tokens)
                 VALUES (?1, ?2, ?3, 'GLM-5.3', ?4, ?5, ?6, 0, ?7, ?8, ?9)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, started_at)).unwrap();
        statement.bind((4, status)).unwrap();
        statement.bind((5, counts.input_tokens)).unwrap();
        statement.bind((6, counts.output_tokens)).unwrap();
        statement.bind((7, counts.cache_creation)).unwrap();
        statement.bind((8, counts.cache_read)).unwrap();
        statement.bind((9, counts.computed_total)).unwrap();
        statement.next().unwrap();
    }

    fn insert_session(path: &Path, id: &str, directory: &str) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare("INSERT INTO session (id, directory) VALUES (?1, ?2)")
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, directory)).unwrap();
        statement.next().unwrap();
    }

    fn fixture_with_db() -> (ccusage_test_support::Fixture, std::path::PathBuf) {
        let fixture = fs_fixture!({});
        let _ = fixture.create_dir_all("cli/db");
        let db_file = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        create_db(&db_file);
        (fixture, db_file)
    }

    fn load(fixture_root: &Path, mode: CostMode) -> Vec<LoadedEntry> {
        let _cleanup = EnvVarGuard::set(super::super::paths::ZCODE_HOME_ENV, fixture_root);
        let shared = SharedArgs {
            mode,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        load_entries(&shared, &PricingMap::load_embedded()).unwrap()
    }

    #[test]
    fn carves_cache_reads_out_of_input_tokens_and_prices_the_model() {
        let (fixture, db_file) = fixture_with_db();
        insert_session(&db_file, "session-a", "/Users/aaron/code/proj");
        // input_tokens includes the cache-read slice; computed = input + output.
        insert_usage(
            &db_file,
            "usage-1",
            "session-a",
            1_786_909_042_666,
            "completed",
            UsageCounts {
                input_tokens: 16_507,
                output_tokens: 59,
                cache_creation: 0,
                cache_read: 11_712,
                computed_total: 16_566,
            },
        );

        let entries = load(fixture.root(), CostMode::Auto);

        assert_eq!(entries.len(), 1);
        let usage = entries[0].data.message.usage;
        assert_eq!(usage.input_tokens, 16_507 - 11_712);
        assert_eq!(usage.output_tokens, 59);
        assert_eq!(usage.cache_read_input_tokens, 11_712);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(entries[0].date, "2026-08-16");
        assert_eq!(entries[0].model.as_deref(), Some("GLM-5.3"));
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].project_path.as_ref(), "/Users/aaron/code/proj");
        assert_eq!(entries[0].data.version, None);
        // glm-5.3 is in the embedded pricing snapshot, so the cost derives
        // from it instead of reporting a missing-pricing model.
        assert!(entries[0].cost > 0.0);
        assert!(entries[0].missing_pricing_model.is_none());
    }

    #[test]
    fn carves_cache_creation_out_of_input_tokens() {
        let (fixture, db_file) = fixture_with_db();
        insert_usage(
            &db_file,
            "usage-1",
            "session-a",
            1_786_909_042_666,
            "completed",
            UsageCounts {
                input_tokens: 1_000,
                output_tokens: 300,
                cache_creation: 100,
                cache_read: 200,
                computed_total: 1_300,
            },
        );

        let entries = load(fixture.root(), CostMode::Auto);

        assert_eq!(entries.len(), 1);
        let usage = entries[0].data.message.usage;
        assert_eq!(usage.input_tokens, 700);
        assert_eq!(usage.cache_creation_input_tokens, 100);
        assert_eq!(usage.cache_read_input_tokens, 200);
        assert_eq!(crate::total_usage_tokens(usage), 1_300);
    }

    #[test]
    fn loads_only_completed_requests() {
        let (fixture, db_file) = fixture_with_db();
        insert_usage(
            &db_file,
            "usage-1",
            "session-a",
            1_786_909_042_666,
            "completed",
            UsageCounts {
                input_tokens: 100,
                output_tokens: 10,
                cache_creation: 0,
                cache_read: 0,
                computed_total: 110,
            },
        );
        insert_usage(
            &db_file,
            "usage-2",
            "session-a",
            1_786_909_043_000,
            "error",
            UsageCounts {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation: 0,
                cache_read: 0,
                computed_total: 0,
            },
        );
        insert_usage(
            &db_file,
            "usage-3",
            "session-a",
            1_786_909_044_000,
            "cancelled",
            UsageCounts {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation: 0,
                cache_read: 0,
                computed_total: 0,
            },
        );

        let entries = load(fixture.root(), CostMode::Auto);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("usage-1"));
    }

    #[test]
    fn clamps_cache_reads_to_input_tokens() {
        let (fixture, db_file) = fixture_with_db();
        insert_usage(
            &db_file,
            "usage-1",
            "session-a",
            1_786_909_042_666,
            "completed",
            UsageCounts {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation: 0,
                cache_read: 40,
                computed_total: 15,
            },
        );

        let entries = load(fixture.root(), CostMode::Auto);

        assert_eq!(entries.len(), 1);
        let usage = entries[0].data.message.usage;
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 10);
        assert_eq!(crate::total_usage_tokens(usage), 15);
    }

    #[test]
    fn missing_database_returns_no_entries() {
        let fixture = fs_fixture!({});
        let entries = load(fixture.root(), CostMode::Auto);

        assert!(entries.is_empty());
    }

    #[test]
    fn unexpected_schema_returns_no_entries() {
        let fixture = fs_fixture!({});
        let _ = fixture.create_dir_all("cli/db");
        let db_file = fixture.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        let db = sqlite::open(&db_file).unwrap();
        db.execute("CREATE TABLE unrelated (id TEXT)").unwrap();

        let entries = load(fixture.root(), CostMode::Auto);

        assert!(entries.is_empty());
    }

    #[test]
    fn deduplicates_usage_rows_across_zcode_homes() {
        let (first, db_file) = fixture_with_db();
        insert_usage(
            &db_file,
            "usage-1",
            "session-a",
            1,
            "completed",
            UsageCounts {
                input_tokens: 100,
                output_tokens: 10,
                cache_creation: 0,
                cache_read: 0,
                computed_total: 110,
            },
        );
        let second = fs_fixture!({});
        let _ = second.create_dir_all("cli/db");
        let second_db = second.path(super::super::paths::ZCODE_DB_RELATIVE_PATH);
        std::fs::copy(&db_file, &second_db).unwrap();
        insert_usage(
            &second_db,
            "usage-2",
            "session-a",
            2,
            "completed",
            UsageCounts {
                input_tokens: 50,
                output_tokens: 5,
                cache_creation: 0,
                cache_read: 0,
                computed_total: 55,
            },
        );

        let _cleanup = EnvVarGuard::set(
            super::super::paths::ZCODE_HOME_ENV,
            format!("{},{}", first.root().display(), second.root().display()),
        );
        let shared = SharedArgs {
            mode: CostMode::Auto,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        // The first root wins for the shared id.
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.data.message.id.as_deref() == Some("usage-1"))
                .unwrap()
                .data
                .message
                .usage
                .input_tokens,
            100
        );
    }
}
