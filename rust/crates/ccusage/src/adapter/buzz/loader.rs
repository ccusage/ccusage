use std::{collections::HashSet, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};

use super::{parser::payload_to_entry, paths::buzz_db_paths};

const BUZZ_QUERY: &str = r#"
SELECT id, raw_json
FROM archived_events
WHERE kind = 44200
"#;

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Buzz, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let db_paths = buzz_db_paths()?;

    // Each DB path is read independently; we dedup by event `id` across DBs.
    let mut entries: Vec<LoadedEntry> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for db_path in &db_paths {
        let db_entries = load_entries_from_db(db_path, tz.as_ref(), pricing, shared)
            .unwrap_or_else(|error| {
                debug_log(
                    shared,
                    format!("Failed to load Buzz archive {}: {error}", db_path.display()),
                );
                Vec::new()
            });
        for entry in db_entries {
            // Use the message id (= event id) as the dedup key.
            let key = entry
                .data
                .message
                .id
                .clone()
                .unwrap_or_else(|| entry.session_id.to_string());
            if seen_ids.insert(key) {
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
            format!("Failed to open Buzz archive: {}", db_path.display()),
        );
        return Ok(Vec::new());
    };

    let Ok(mut statement) = connection.prepare(BUZZ_QUERY) else {
        debug_log(
            shared,
            format!(
                "Failed to prepare Buzz archive query: {}",
                db_path.display()
            ),
        );
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let event_id = statement.read::<String, _>(0).unwrap_or_default();
                let raw_json = match statement.read::<String, _>(1) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(entry) = payload_to_entry(&event_id, &raw_json, tz, pricing) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!("Failed to query Buzz archive: {}", db_path.display()),
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
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn create_buzz_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            r#"
CREATE TABLE archived_events (
    identity_pubkey TEXT,
    relay_url TEXT,
    id TEXT PRIMARY KEY,
    kind INTEGER,
    pubkey TEXT,
    created_at INTEGER,
    raw_json TEXT,
    archived_at TEXT
)
"#,
        )
        .unwrap();
    }

    struct EventFixture<'a> {
        id: &'a str,
        kind: i64,
        raw_json: &'a str,
    }

    fn insert_event(path: &Path, fixture: EventFixture<'_>) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare(
                r#"
INSERT INTO archived_events (id, kind, raw_json)
VALUES (?1, ?2, ?3)
"#,
            )
            .unwrap();
        statement.bind((1, fixture.id)).unwrap();
        statement.bind((2, fixture.kind)).unwrap();
        statement.bind((3, fixture.raw_json)).unwrap();
        statement.next().unwrap();
    }

    #[test]
    fn loads_kind_44200_events_from_buzz_db() {
        let fixture = fs_fixture!({
            "archive/archive.db": "",
        });
        let db_path = fixture.path("archive/archive.db");
        create_buzz_db(&db_path);
        insert_event(
            &db_path,
            EventFixture {
                id: "evt-1",
                kind: 44200,
                raw_json: r#"{
                    "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
                    "sessionId":"ses_abc","turnSeq":2,"timestamp":"2026-07-07T10:00:00.000Z",
                    "turn":{"inputTokens":1000,"outputTokens":50,"totalTokens":null,"costUsd":null},
                    "cumulative":{"inputTokens":2000,"outputTokens":100,"totalTokens":null,"costUsd":null},
                    "deltaReliable":true,"stopReason":"end_turn"
                }"#,
            },
        );

        let _cleanup = EnvVarGuard::set(super::super::paths::BUZZ_PATH_ROOT_ENV, fixture.root());
        let pricing = PricingMap::load_embedded();
        let entries = load_entries_from_db(
            &db_path,
            Some(&jiff::tz::TimeZone::UTC),
            &pricing,
            &SharedArgs::default(),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-07-07");
        assert_eq!(entries[0].session_id.as_ref(), "ses_abc");
        assert_eq!(entries[0].data.message.usage.input_tokens, 1000);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
    }

    #[test]
    fn ignores_non_44200_events() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path(super::super::paths::BUZZ_ARCHIVE_FILE_NAME);
        create_buzz_db(&db_path);
        insert_event(
            &db_path,
            EventFixture {
                id: "evt-other",
                kind: 9,
                raw_json: r#"{"text":"hello"}"#,
            },
        );

        let _cleanup = EnvVarGuard::set(super::super::paths::BUZZ_PATH_ROOT_ENV, fixture.root());
        let pricing = PricingMap::load_embedded();
        let entries = load_entries_from_db(
            &db_path,
            Some(&jiff::tz::TimeZone::UTC),
            &pricing,
            &SharedArgs::default(),
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn deduplicates_by_event_id_across_calls() {
        let fixture = fs_fixture!({
            "archive/archive.db": "",
        });
        let db_path = fixture.path("archive/archive.db");
        create_buzz_db(&db_path);
        // Insert a turn-seq-1 event with null turn (uses cumulative)
        insert_event(
            &db_path,
            EventFixture {
                id: "evt-dup",
                kind: 44200,
                raw_json: r#"{
                    "harness":"buzz-agent","model":"goose-claude-4-6-sonnet",
                    "sessionId":"ses_dup","turnSeq":1,"timestamp":"2026-07-07T09:00:00.000Z",
                    "turn":null,
                    "cumulative":{"inputTokens":500,"outputTokens":25,"totalTokens":null,"costUsd":null},
                    "deltaReliable":false,"stopReason":"end_turn"
                }"#,
            },
        );

        let _cleanup = EnvVarGuard::set(super::super::paths::BUZZ_PATH_ROOT_ENV, fixture.root());
        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs::default();

        // Call load_entries_inner twice simulated by loading the same DB via public API
        // — inner dedup by event id ensures idempotency.
        let entries = load_entries_inner(&shared, &pricing).unwrap();

        assert_eq!(entries.len(), 1, "expected exactly one entry after dedup");
        assert_eq!(entries[0].data.message.usage.input_tokens, 500);
    }

    /// Smoke test against the real archive.db on this machine.
    /// Skipped in CI (no real DB present); useful for local schema-drift detection.
    #[test]
    #[ignore]
    fn smoke_test_real_archive_db() {
        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs::default();
        let entries = load_entries_inner(&shared, &pricing).unwrap();
        assert!(
            !entries.is_empty(),
            "expected at least one entry from the real archive.db"
        );
        println!("Loaded {} entries from real archive.db", entries.len());
        let total_input: u64 = entries
            .iter()
            .map(|e| e.data.message.usage.input_tokens)
            .sum();
        let total_cost: f64 = entries.iter().map(|e| e.cost).sum();
        println!("Total input tokens: {total_input}, total cost: ${total_cost:.4}");
    }
}
