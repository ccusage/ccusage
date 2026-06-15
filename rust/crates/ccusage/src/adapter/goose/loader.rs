use std::{collections::HashSet, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{parser::row_to_entry, paths::goose_db_paths};

const GOOSE_SESSION_QUERY: &str = r#"
SELECT
    id,
    model_config_json,
    provider_name,
    created_at,
    total_tokens,
    input_tokens,
    output_tokens,
    accumulated_total_tokens,
    accumulated_input_tokens,
    accumulated_output_tokens
FROM sessions
WHERE model_config_json IS NOT NULL
    AND TRIM(model_config_json) != ''
"#;

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Goose, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let db_paths = goose_db_paths()?;
    // Load each database in parallel (a fresh read-only connection per DB), then
    // run the sequential per-db dedup over the original path order so the
    // surviving session per key matches the single-threaded read.
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_entries_from_db(db_path, tz.as_ref(), pricing, shared).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to load Goose database {}: {error}",
                    db_path.display()
                ),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (db_path, db_entries) in db_paths.iter().zip(loaded) {
        for entry in db_entries {
            let key = format!("{}:{}", db_path.display(), entry.session_id);
            if seen.insert(key) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_entries_from_db(
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
    shared: &SharedArgs,
) -> Result<Vec<LoadedEntry>> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open Goose database: {}", db_path.display()),
        );
        return Ok(Vec::new());
    };
    let Ok(mut statement) = connection.prepare(GOOSE_SESSION_QUERY) else {
        debug_log(
            shared,
            format!("Failed to read Goose database: {}", db_path.display()),
        );
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                if let Some(entry) = row_to_entry(&statement, tz, pricing) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!("Failed to query Goose database: {}", db_path.display()),
                );
                break;
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_test_support::fs_fixture;

    fn create_goose_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            r#"
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    model_config_json TEXT,
    provider_name TEXT,
    created_at TEXT,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    accumulated_total_tokens INTEGER,
    accumulated_input_tokens INTEGER,
    accumulated_output_tokens INTEGER
)
"#,
        )
        .unwrap();
    }

    struct SessionFixture<'a> {
        id: &'a str,
        model_config: &'a str,
        provider: Option<&'a str>,
        created_at: &'a str,
        total: i64,
        input: i64,
        output: i64,
    }

    fn insert_session(path: &Path, fixture: SessionFixture<'_>) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare(
                r#"
INSERT INTO sessions (
    id,
    model_config_json,
    provider_name,
    created_at,
    accumulated_total_tokens,
    accumulated_input_tokens,
    accumulated_output_tokens
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
            )
            .unwrap();
        statement.bind((1, fixture.id)).unwrap();
        statement.bind((2, fixture.model_config)).unwrap();
        statement.bind((3, fixture.provider)).unwrap();
        statement.bind((4, fixture.created_at)).unwrap();
        statement.bind((5, fixture.total)).unwrap();
        statement.bind((6, fixture.input)).unwrap();
        statement.bind((7, fixture.output)).unwrap();
        statement.next().unwrap();
    }

    #[test]
    fn loads_accumulated_tokens_from_goose_sqlite() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path(super::super::paths::GOOSE_DB_FILE_NAME);
        create_goose_db(&db_path);
        insert_session(
            &db_path,
            SessionFixture {
                id: "session-a",
                model_config: r#"{"model_name":"claude-sonnet-4-20250514"}"#,
                provider: Some("anthropic"),
                created_at: "2026-05-01 01:02:03",
                total: 180,
                input: 100,
                output: 50,
            },
        );

        let pricing = PricingMap::load_embedded();
        let entries = load_entries_from_db(
            &db_path,
            Some(&jiff::tz::TimeZone::UTC),
            &pricing,
            &SharedArgs::default(),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-05-01");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 30);
    }
}
