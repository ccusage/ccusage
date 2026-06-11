use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{message_to_entry, reprice},
    paths::paths,
};
use crate::{
    LoadedEntry, OpenCodeMessage, PricingMap, Result,
    cache::{self, OpenCodeRow},
    cli::{CostMode, SharedArgs},
    collect_files_with_extension, debug_log, parse_tz,
};

pub(crate) fn load_entries(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent::OpenCode,
        shared.json,
        || load_entries_inner(shared),
    )
}

fn load_entries_inner(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths()? {
        for entry in load_entries_from_directory(&path, shared)? {
            if let Some(id) = entry_id(&entry)
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

pub(crate) fn load_entries_from_directory(
    opencode_dir: &Path,
    shared: &SharedArgs,
) -> Result<Vec<LoadedEntry>> {
    let pricing = if shared.mode == CostMode::Display {
        None
    } else {
        Some(PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    // Load current DB rows (empty when the database file is absent), then always
    // pass them through the ledger under a dedicated namespace so spend is
    // retained even after the whole `opencode.db` is deleted.
    let db_entries = match db_path(opencode_dir) {
        Some(db_path) => {
            load_entries_from_database(&db_path, tz.as_ref(), shared.mode, pricing.as_ref(), shared)
        }
        None => Vec::new(),
    };
    for entry in cache::retain_via_ledger("opencode-db", db_entries, shared.live_only) {
        if let Some(id) = entry_id(&entry)
            && !seen.insert(id.to_string())
        {
            continue;
        }
        entries.push(entry);
    }

    let messages_dir = opencode_dir.join("storage").join("message");
    let mut files = Vec::new();
    collect_files_with_extension(&messages_dir, "json", &mut files);
    let json_entries = cache::load_with_cache(
        "opencode",
        &files,
        shared.single_thread,
        shared.live_only,
        cache::Freshness::FileStat,
        |path| read_message_file(path, tz.as_ref(), shared.mode, pricing.as_ref(), shared),
        |e| reprice(e, shared.mode, pricing.as_ref()),
    )?;
    for entry in json_entries {
        if let Some(id) = entry_id(&entry)
            && !seen.insert(id.to_string())
        {
            continue;
        }
        entries.push(entry);
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn db_path(opencode_dir: &Path) -> Option<PathBuf> {
    let default_path = opencode_dir.join("opencode.db");
    if default_path.is_file() {
        return Some(default_path);
    }
    let mut candidates = fs::read_dir(opencode_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_channel_db_name)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_channel_db_name(name: &str) -> bool {
    name.starts_with("opencode-")
        && name.ends_with(".db")
        && name["opencode-".len()..name.len() - ".db".len()]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Load all OpenCode message-database rows, reusing the per-database row cache so
/// rows whose `data` content hash is unchanged skip the expensive JSON parse.
///
/// The output is built only from rows seen in *this* scan, so the result is byte
/// identical to a cold (no-cache) run: rows deleted from the database fall out
/// and are never resurrected, and reuse is keyed on an exact `id` match with
/// `content_hash` validation so changed content correctly invalidates the cache.
fn load_entries_from_database(
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
) -> Vec<LoadedEntry> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open OpenCode database: {}", db_path.display()),
        );
        return Vec::new();
    };

    let cache_key = cache::file_metadata(db_path).map(|meta| meta.cache_key);
    // Index the previous run's rows by id for O(1) reuse lookups.
    let cached: HashMap<String, OpenCodeRow> = cache_key
        .as_deref()
        .and_then(cache::load_opencode_row_cache)
        .into_iter()
        .flatten()
        .map(|row| (row.id.clone(), row))
        .collect();

    let mut statement = match connection.prepare("SELECT id, session_id, data FROM message") {
        Ok(statement) => statement,
        Err(_) => {
            debug_log(
                shared,
                format!("Failed to read OpenCode database: {}", db_path.display()),
            );
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    let mut fresh_rows = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(id) = statement.read::<String, _>(0) else {
                    continue;
                };
                let Ok(session_id) = statement.read::<String, _>(1) else {
                    continue;
                };
                let Ok(data) = statement.read::<String, _>(2) else {
                    continue;
                };

                let mut hasher = rustc_hash::FxHasher::default();
                data.hash(&mut hasher);
                let content_hash = hasher.finish();

                // Reuse the cached entry on exact (id, content_hash) match.
                if let Some(row) = cached.get(&id)
                    && row.content_hash == content_hash
                {
                    entries.push(crate::LoadedEntry::from(row.entry.clone()));
                    fresh_rows.push(row.clone());
                    continue;
                }

                let msg = match serde_json::from_str::<OpenCodeMessage>(&data) {
                    Ok(msg) => msg,
                    Err(error) => {
                        debug_log(
                            shared,
                            format!(
                                "Failed to read OpenCode database message {}: {error}",
                                db_path.display()
                            ),
                        );
                        continue;
                    }
                };
                if let Some(entry) =
                    message_to_entry(&msg, Some(id.clone()), Some(session_id), tz, mode, pricing)
                {
                    fresh_rows.push(OpenCodeRow {
                        id,
                        content_hash,
                        entry: crate::cache::CachedEntry::from(&entry),
                    });
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!("Failed to query OpenCode database: {}", db_path.display()),
                );
                break;
            }
        }
    }

    // Rebuild the cache from exactly the rows seen this run.
    if let Some(cache_key) = cache_key {
        cache::save_opencode_row_cache(&cache_key, &fresh_rows);
    }

    // Reprice every entry (cache hits carry cost 0 from `CachedEntry`; fresh
    // parses are repriced idempotently) so cost reflects the current pricing/mode
    // without reparsing. Ledger-frozen entries are merged later and untouched.
    for entry in &mut entries {
        reprice(entry, mode, pricing);
    }

    entries
}

fn read_message_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
) -> crate::Result<Vec<LoadedEntry>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to read OpenCode message file {}: {error}",
                    path.display()
                ),
            );
            return Ok(Vec::new());
        }
    };
    let msg = match serde_json::from_str::<OpenCodeMessage>(&content) {
        Ok(msg) => msg,
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to read OpenCode message file {}: {error}",
                    path.display()
                ),
            );
            return Ok(Vec::new());
        }
    };
    Ok(message_to_entry(&msg, None, None, tz, mode, pricing)
        .into_iter()
        .collect())
}

fn entry_id(entry: &LoadedEntry) -> Option<&str> {
    entry.data.message.id.as_deref().filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::MutexGuard,
    };

    use super::load_entries_from_directory;
    use crate::cli::{CostMode, SharedArgs};
    use ccusage_test_support::{Fixture, fs_fixture};

    /// Serializes tests that mutate the process-global `XDG_CACHE_HOME`. Shares
    /// the crate-wide lock so opencode tests never race cache tests on the env
    /// var (both now read/write the same `ledger.jsonl`).
    fn test_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    /// Isolate cache I/O in a temp dir so tests never touch the real cache.
    struct CacheEnv {
        dir: PathBuf,
        prev_xdg: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }

    impl CacheEnv {
        fn new(name: &str) -> Self {
            let guard = test_lock();
            let dir = std::env::temp_dir().join(format!("ccusage-opencode-test-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
            unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };
            Self {
                dir,
                prev_xdg,
                _guard: guard,
            }
        }
    }

    impl Drop for CacheEnv {
        fn drop(&mut self) {
            match &self.prev_xdg {
                Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
                None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// Create a message database with the realistic OpenCode schema, carrying
    /// `time_created` / `time_updated` columns.
    fn create_db_message(path: &Path, id: &str, session_id: &str, data: &str) {
        create_db_message_at(path, id, session_id, data, time_created_of(data));
    }

    fn create_db_message_at(path: &Path, id: &str, session_id: &str, data: &str, time: i64) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "CREATE TABLE IF NOT EXISTS message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
        )
        .unwrap();
        insert_message(&db, id, session_id, data, time);
    }

    fn insert_message(db: &sqlite::Connection, id: &str, session_id: &str, data: &str, time: i64) {
        let mut statement = db
            .prepare("INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)")
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, time)).unwrap();
        statement.bind((4, time)).unwrap();
        statement.bind((5, data)).unwrap();
        statement.next().unwrap();
    }

    fn time_created_of(data: &str) -> i64 {
        serde_json::from_str::<serde_json::Value>(data)
            .ok()
            .and_then(|v| v.get("time")?.get("created")?.as_i64())
            .unwrap_or(0)
    }

    fn display_shared() -> SharedArgs {
        SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        }
    }

    #[test]
    fn loads_message_json_files() {
        let _env = CacheEnv::new("loads-json");
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"cache":{"read":10,"write":20}},"cost":0.02}"#,
        });

        let entries = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();

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
        assert_eq!(entries[0].cost, 0.02);
    }

    #[test]
    fn loads_messages_from_sqlite_database() {
        let _env = CacheEnv::new("loads-sqlite");
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode.db"),
            "db-msg-1",
            "db-session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60,"cache":{"read":12,"write":24}},"cost":0.03}"#,
        );

        let entries = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(entries[0].session_id.as_ref(), "db-session-a");
        assert_eq!(entries[0].data.message.id.as_deref(), Some("db-msg-1"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].data.message.usage.output_tokens, 60);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            24
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 12);
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn loads_channel_sqlite_database() {
        let _env = CacheEnv::new("loads-channel");
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode-beta.db"),
            "channel-msg-1",
            "channel-session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":80,"output":40}}"#,
        );

        let entries = load_entries_from_directory(fixture.root(), &SharedArgs::default()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "channel-session-a");
        assert_eq!(entries[0].data.message.usage.input_tokens, 80);
    }

    #[test]
    fn prefers_database_messages_over_duplicate_json_files() {
        let _env = CacheEnv::new("prefers-db");
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"json-session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":0.99}"#,
        });
        create_db_message(
            &fixture.path("opencode.db"),
            "msg-1",
            "db-session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "db-session-a");
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn serves_unchanged_row_from_cache_on_second_load() {
        let _env = CacheEnv::new("row-cache-hit");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        );

        let cold = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        // The cache database holding the row cache must now exist.
        assert!(
            _env.dir.join("ccusage").join("cache.db").exists(),
            "row cache should be written to the cache database"
        );

        let warm = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(cold.len(), warm.len());
        assert_eq!(cold[0].cost, warm[0].cost);
        assert_eq!(
            cold[0].data.message.usage.input_tokens,
            warm[0].data.message.usage.input_tokens
        );
    }

    #[test]
    fn changed_row_content_invalidates_cache() {
        let _env = CacheEnv::new("row-content-change");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":100}}"#,
        );

        let cold = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(cold[0].data.message.usage.input_tokens, 100);

        // Rewrite the row's `data` in place (same id). Only the content hash
        // catches this; the old `(id, time_updated)` key would serve a stale 100.
        let conn = sqlite::open(&db).unwrap();
        conn.execute(
            r#"UPDATE message SET data = '{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":999}}' WHERE id = 'msg-1'"#,
        )
        .unwrap();
        drop(conn);

        let warm = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(
            warm[0].data.message.usage.input_tokens, 999,
            "changed row content must invalidate the cache and reparse"
        );
    }

    #[test]
    fn deleted_database_retains_spend_via_ledger() {
        let _env = CacheEnv::new("db-retain");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":100}}"#,
        );

        // Cold load records the DB row's spend in the ledger.
        let cold = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(cold[0].data.message.usage.input_tokens, 100);

        // Delete the entire database file — simulates a removed opencode store.
        fs::remove_file(&db).unwrap();

        // The spend must still be reported, re-emitted from the ledger.
        let warm = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(
            warm.len(),
            1,
            "deleted DB spend must be retained via the ledger"
        );
        assert_eq!(warm[0].data.message.usage.input_tokens, 100);
    }

    #[test]
    fn deleted_row_spend_retained_via_ledger() {
        let _env = CacheEnv::new("row-cache-delete");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":100}}"#,
        );
        create_db_message(
            &db,
            "msg-2",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":200}}"#,
        );

        let first = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(first.len(), 2);

        // Delete one row, then reload: its spend must persist via the ledger so
        // a removed chat never erases the tokens/cost already incurred.
        let conn = sqlite::open(&db).unwrap();
        conn.execute("DELETE FROM message WHERE id = 'msg-2'")
            .unwrap();
        drop(conn);

        let second = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(second.len(), 2, "deleted row's spend must be retained");
        let deleted = second
            .iter()
            .find(|e| e.data.message.id.as_deref() == Some("msg-2"))
            .expect("deleted row retained from ledger");
        assert_eq!(deleted.data.message.usage.input_tokens, 200);
    }

    #[test]
    fn loads_distinct_rows_with_same_timestamp() {
        let _env = CacheEnv::new("row-same-tick");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        // Two distinct rows sharing an identical time_updated value: content-hash
        // keying must keep both, never collapse them on the shared timestamp.
        create_db_message_at(
            &db,
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":100}}"#,
            1767312000000,
        );
        create_db_message_at(
            &db,
            "msg-2",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":200}}"#,
            1767312000000,
        );

        let first = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(first.len(), 2);
        let second = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn loads_legacy_schema_without_time_updated() {
        let _env = CacheEnv::new("legacy-schema");
        let fixture = fs_fixture!({});
        let db = fixture.path("opencode.db");
        let conn = sqlite::open(&db).unwrap();
        conn.execute("CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)")
            .unwrap();
        let mut statement = conn
            .prepare("INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)")
            .unwrap();
        statement.bind((1, "msg-1")).unwrap();
        statement.bind((2, "session-a")).unwrap();
        statement
            .bind((
                3,
                r#"{"providerID":"anthropic","modelID":"m","time":{"created":1767312000000},"tokens":{"input":100}}"#,
            ))
            .unwrap();
        statement.next().unwrap();
        drop(statement);
        drop(conn);

        let entries = load_entries_from_directory(fixture.root(), &display_shared()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
    }
    #[test]
    fn skips_message_files_already_covered_by_database() {
        let _env = CacheEnv::new("opencode-skips-db-covered");
        // Real OpenCode message files live at
        // `storage/message/<sessionID>/<messageID>.json`, so the file stem is
        // the message id. The DB pass contributes `msg-db`, so the matching
        // file must be dropped (DB wins) while the file that the DB does not
        // cover is still loaded.
        let fixture = fs_fixture!({
            "storage/message/ses_a/msg-db.json": r#"{"id":"msg-db","sessionID":"json-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":0.99}"#,
            "storage/message/ses_a/msg-file.json": r#"{"id":"msg-file","sessionID":"file-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000001},"tokens":{"input":50,"output":25},"cost":0.01}"#,
        });
        create_db_message(
            &fixture.path("opencode.db"),
            "msg-db",
            "db-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(entries.len(), 2);
        // The DB-covered id keeps the DB row, not the file's inflated tokens.
        let db_entry = entries
            .iter()
            .find(|entry| entry.data.message.id.as_deref() == Some("msg-db"))
            .expect("db-covered message present");
        assert_eq!(db_entry.session_id.as_ref(), "db-session");
        assert_eq!(db_entry.data.message.usage.input_tokens, 120);
        // The file the DB does not cover is still read and parsed.
        let file_entry = entries
            .iter()
            .find(|entry| entry.data.message.id.as_deref() == Some("msg-file"))
            .expect("db-uncovered message present");
        assert_eq!(file_entry.session_id.as_ref(), "file-session");
        assert_eq!(file_entry.data.message.usage.input_tokens, 50);
    }

    #[test]
    fn dedup_is_stable_across_thread_counts() {
        let _env = CacheEnv::new("opencode-dedup-thread-stable");
        // Build a directory with many files spread over several sessions, some
        // sharing ids with each other and with the DB, so the file pass has to
        // dedup. Parallel reads must not change which duplicate survives or the
        // final ordering compared to the single-threaded read.
        let fixture = Fixture::new();
        for session in 0..4 {
            for message in 0..15 {
                let id = format!("msg-{session}-{message}");
                let created = 1_767_312_000_000_i64 + i64::from(session * 100 + message);
                let path = format!("storage/message/ses_{session}/{id}.json");
                let data = format!(
                    r#"{{"id":"{id}","sessionID":"ses_{session}","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{{"created":{created}}},"tokens":{{"input":{input},"output":10}}}}"#,
                    input = 100 + message,
                );
                let _ = fixture.write_file(path, data);
            }
        }
        // A duplicate file (same id, later timestamp) to force the file-vs-file
        // dedup path under both thread counts.
        let _ = fixture.write_file(
            "storage/message/ses_dup/msg-0-0.json",
            r#"{"id":"msg-0-0","sessionID":"ses_dup","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312999999},"tokens":{"input":7777,"output":10}}"#,
        );

        create_db_message(
            &fixture.path("opencode.db"),
            "msg-1-1",
            "db-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60}}"#,
        );

        let single = SharedArgs {
            mode: CostMode::Display,
            single_thread: true,
            ..SharedArgs::default()
        };
        let multi = SharedArgs {
            mode: CostMode::Display,
            single_thread: false,
            ..SharedArgs::default()
        };

        let single_entries = load_entries_from_directory(fixture.root(), &single).unwrap();
        let multi_entries = load_entries_from_directory(fixture.root(), &multi).unwrap();

        let project = |entries: &[crate::LoadedEntry]| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry.timestamp.as_millis(),
                        entry.data.message.id.clone(),
                        entry.session_id.to_string(),
                        entry.data.message.usage.input_tokens,
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(project(&single_entries), project(&multi_entries));
    }
}
