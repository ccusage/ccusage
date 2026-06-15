use std::{collections::HashSet, path::Path};

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, read_files_parallel};

use super::{
    parser::{HermesEntry, read_session_row, to_loaded_entry},
    paths::hermes_state_db_paths,
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Hermes, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = crate::parse_tz(shared.timezone.as_deref());
    let db_paths = hermes_state_db_paths()?;
    // Load each state database in parallel (a fresh read-only connection per DB),
    // then run the sequential session dedup over the original path order so the
    // surviving session matches the single-threaded read.
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_state_db_entries(db_path, shared)
    });
    let mut entries = Vec::new();
    let mut seen_sessions = HashSet::new();
    for db_entries in loaded {
        for entry in db_entries {
            if !seen_sessions.insert(entry.session_id.clone()) {
                continue;
            }
            entries.push(to_loaded_entry(entry, tz.as_ref(), pricing));
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_state_db_entries(db_path: &Path, shared: &SharedArgs) -> Vec<HermesEntry> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        crate::debug_log(
            shared,
            format!(
                "Failed to open Hermes state database: {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "
            SELECT
                id,
                model,
                billing_provider,
                started_at,
                message_count,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
                estimated_cost_usd,
                actual_cost_usd
            FROM sessions
            WHERE model IS NOT NULL
                AND TRIM(model) != ''
        ",
    ) else {
        crate::debug_log(
            shared,
            format!(
                "Failed to read Hermes state database: {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                if let Some(entry) = read_session_row(&statement) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                crate::debug_log(
                    shared,
                    format!(
                        "Failed to query Hermes state database: {}",
                        db_path.display()
                    ),
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
    use ccusage_test_support::fs_fixture;

    fn create_state_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    source TEXT NOT NULL,
                    model TEXT,
                    started_at REAL NOT NULL,
                    message_count INTEGER DEFAULT 0,
                    input_tokens INTEGER DEFAULT 0,
                    output_tokens INTEGER DEFAULT 0,
                    cache_read_tokens INTEGER DEFAULT 0,
                    cache_write_tokens INTEGER DEFAULT 0,
                    reasoning_tokens INTEGER DEFAULT 0,
                    billing_provider TEXT,
                    estimated_cost_usd REAL,
                    actual_cost_usd REAL
                );
            ",
        )
        .unwrap();
    }

    #[test]
    fn loads_billable_hermes_sessions_from_state_db() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        let db = sqlite::open(&db_path).unwrap();
        let mut statement = db
            .prepare(
                "
                    INSERT INTO sessions (
                        id, source, model, started_at, message_count,
                        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                        billing_provider, estimated_cost_usd, actual_cost_usd
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ",
            )
            .unwrap();
        statement.bind((1, "session-1")).unwrap();
        statement.bind((2, "cli")).unwrap();
        statement.bind((3, "claude-sonnet-4-20250514")).unwrap();
        statement.bind((4, 1_750_000_000.25)).unwrap();
        statement.bind((5, 42_i64)).unwrap();
        statement.bind((6, 1200_i64)).unwrap();
        statement.bind((7, 300_i64)).unwrap();
        statement.bind((8, 50_i64)).unwrap();
        statement.bind((9, 20_i64)).unwrap();
        statement.bind((10, 10_i64)).unwrap();
        statement.bind((11, "anthropic")).unwrap();
        statement.bind((12, 0.12)).unwrap();
        statement.bind((13, 0.34)).unwrap();
        statement.next().unwrap();

        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let tz = crate::parse_tz(shared.timezone.as_deref());
        let entries = load_state_db_entries(&db_path, &shared)
            .into_iter()
            .map(|entry| to_loaded_entry(entry, tz.as_ref(), &pricing))
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2025-06-15");
        assert_eq!(entries[0].session_id.as_ref(), "session-1");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 1200);
        assert_eq!(entries[0].data.message.usage.output_tokens, 300);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert_eq!(entries[0].message_count, Some(42));
        assert_eq!(entries[0].cost, 0.34);
    }
}
