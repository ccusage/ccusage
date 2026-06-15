use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{OpenCodeMessage, message_value_to_entry},
    paths::paths,
};
use crate::{
    LoadedEntry, PricingMap, Result,
    cli::{CostMode, SharedArgs},
    collect_files_with_extension, debug_log, parse_tz, read_files_parallel,
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
    if let Some(db_path) = db_path(opencode_dir) {
        for entry in
            load_entries_from_database(&db_path, tz.as_ref(), shared.mode, pricing.as_ref(), shared)
        {
            if let Some(id) = entry_id(&entry)
                && !seen.insert(id.to_string())
            {
                continue;
            }
            entries.push(entry);
        }
    }

    let messages_dir = opencode_dir.join("storage").join("message");
    let mut files = Vec::new();
    collect_files_with_extension(&messages_dir, "json", &mut files);

    // Skip files the DB pass already covered. Message files are stored as
    // `storage/message/<sessionID>/<messageID>.json`, so the file stem is the
    // message id used for dedup. When the DB already contributed that id, the
    // file would be discarded by the id dedup below anyway — drop it here so we
    // never pay the read. Files whose stem is not a known id (or that have no
    // usable stem) are kept and parsed normally.
    if !seen.is_empty() {
        files.retain(|file| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !seen.contains(stem))
        });
    }

    // Read the surviving files in parallel, then run the sequential id dedup
    // over the results in their original file order so parallelism never changes
    // which duplicate survives.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        read_message_file(file, tz.as_ref(), shared.mode, pricing.as_ref(), shared)
    });
    for entry in loaded.into_iter().flatten() {
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
    let Ok(mut statement) = connection.prepare("SELECT id, session_id, data FROM message") else {
        debug_log(
            shared,
            format!("Failed to read OpenCode database: {}", db_path.display()),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
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
                let Ok(value) = serde_json::from_str::<OpenCodeMessage>(&data) else {
                    continue;
                };
                if let Some(entry) =
                    message_value_to_entry(&value, Some(id), Some(session_id), tz, mode, pricing)
                {
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
    entries
}

fn read_message_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
) -> Option<LoadedEntry> {
    let content = match fs::read(path) {
        Ok(content) => content,
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to read OpenCode message file {}: {error}",
                    path.display()
                ),
            );
            return None;
        }
    };
    let value = serde_json::from_slice::<OpenCodeMessage>(&content).ok()?;
    message_value_to_entry(&value, None, None, tz, mode, pricing)
}

fn entry_id(entry: &LoadedEntry) -> Option<&str> {
    entry.data.message.id.as_deref().filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::load_entries_from_directory;
    use crate::cli::{CostMode, SharedArgs};
    use ccusage_test_support::fs_fixture;

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
    fn loads_message_json_files() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"cache":{"read":10,"write":20}},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

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
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode.db"),
            "db-msg-1",
            "db-session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60,"cache":{"read":12,"write":24}},"cost":0.03}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

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
    fn skips_message_files_already_covered_by_database() {
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
        // Build a directory with many files spread over several sessions, some
        // sharing ids with each other and with the DB, so the file pass has to
        // dedup. Parallel reads must not change which duplicate survives or the
        // final ordering compared to the single-threaded read.
        let fixture = ccusage_test_support::Fixture::new();
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
