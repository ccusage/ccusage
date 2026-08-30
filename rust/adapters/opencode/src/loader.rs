use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        OpenCodeMessage, OpenCodeSessionAggregate, message_value_to_entry,
        session_message_value_to_entry, session_value_to_entry,
    },
    paths::paths,
};
use crate::{
    LoadedEntry, PricingMap, Result,
    cli::{AgentReportKind, CostMode, SharedArgs},
    collect_files_with_extension, date_range_bounds_ms, debug_log, parse_tz, read_files_parallel,
};

pub fn load_entries(shared: &SharedArgs, report_kind: AgentReportKind) -> Result<Vec<LoadedEntry>> {
    let allow_aggregate_fallback = report_kind == AgentReportKind::Session;
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("OpenCode"),
        shared.json,
        || load_entries_inner(shared, allow_aggregate_fallback),
    )
}

fn load_entries_inner(
    shared: &SharedArgs,
    allow_aggregate_fallback: bool,
) -> Result<Vec<LoadedEntry>> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut message_sessions = HashSet::new();
    let mut aggregate_entries = Vec::new();
    for path in paths()? {
        let directory_entries =
            load_entries_from_directory_parts(&path, shared, allow_aggregate_fallback)?;
        for entry in directory_entries.message_entries {
            message_sessions.insert(entry.session_id.to_string());
            if let Some(id) = entry_id(&entry)
                && !seen.insert(id.to_string())
            {
                continue;
            }
            entries.push(entry);
        }
        aggregate_entries.extend(directory_entries.aggregate_entries);
    }
    append_aggregate_entries(
        &mut entries,
        &mut seen,
        &message_sessions,
        aggregate_entries,
    );
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
fn load_entries_from_directory(
    opencode_dir: &Path,
    shared: &SharedArgs,
) -> Result<Vec<LoadedEntry>> {
    let directory_entries = load_entries_from_directory_parts(opencode_dir, shared, false)?;
    let message_sessions = message_session_ids(&directory_entries.message_entries);
    let mut seen = entry_ids(&directory_entries.message_entries);
    let mut entries = directory_entries.message_entries;
    append_aggregate_entries(
        &mut entries,
        &mut seen,
        &message_sessions,
        directory_entries.aggregate_entries,
    );
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
fn load_entries_from_directory_for_report(
    opencode_dir: &Path,
    shared: &SharedArgs,
    report_kind: AgentReportKind,
) -> Result<Vec<LoadedEntry>> {
    let directory_entries = load_entries_from_directory_parts(
        opencode_dir,
        shared,
        report_kind == AgentReportKind::Session,
    )?;
    let message_sessions = message_session_ids(&directory_entries.message_entries);
    let mut seen = entry_ids(&directory_entries.message_entries);
    let mut entries = directory_entries.message_entries;
    append_aggregate_entries(
        &mut entries,
        &mut seen,
        &message_sessions,
        directory_entries.aggregate_entries,
    );
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_entries_from_directory_parts(
    opencode_dir: &Path,
    shared: &SharedArgs,
    allow_aggregate_fallback: bool,
) -> Result<DirectoryLoadResult> {
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
    let window = DateWindow::from_shared(shared, tz.as_ref());
    let mut message_entries = Vec::new();
    let mut seen = HashSet::new();
    let mut aggregate_entries = Vec::new();
    if let Some(db_path) = db_path(opencode_dir) {
        let database_entries = load_entries_from_database(
            &db_path,
            tz.as_ref(),
            shared.mode,
            pricing.as_ref(),
            shared,
            window,
            allow_aggregate_fallback,
        );
        for entry in database_entries.message_entries {
            if let Some(id) = entry_id(&entry)
                && !seen.insert(id.to_string())
            {
                continue;
            }
            message_entries.push(entry);
        }
        aggregate_entries = database_entries.aggregate_entries;
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
        read_message_file(
            file,
            tz.as_ref(),
            shared.mode,
            pricing.as_ref(),
            shared,
            window,
        )
    });
    for entry in loaded.into_iter().flatten() {
        if let Some(id) = entry_id(&entry)
            && !seen.insert(id.to_string())
        {
            continue;
        }
        message_entries.push(entry);
    }

    Ok(DirectoryLoadResult {
        message_entries,
        aggregate_entries,
    })
}

/// Reports whether `opencode_dir` holds any usage source at all: the SQLite
/// database, or at least one message file.
///
/// Detection has to ignore `--since`/`--until` because the loader applies the
/// window while reading, so an out-of-range query returns no entries even on an
/// install full of logs. Stops at the first message file rather than collecting
/// them all, so this stays cheap next to a large legacy dump.
pub(crate) fn has_source(opencode_dir: &Path) -> bool {
    if db_path(opencode_dir).is_some() {
        return true;
    }
    has_json_file(&opencode_dir.join("storage").join("message"))
}

// Mirrors `collect_files_with_extension`: judge entries by `file_type()` so
// symlinks are neither followed nor counted. Following them would let detection
// claim files the collection pass then refuses to read, and a symlinked cycle
// would recurse until the stack gives out.
fn has_json_file(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        let path = entry.path();
        if file_type.is_file() {
            path.extension()
                .is_some_and(|extension| extension == "json")
        } else {
            file_type.is_dir() && has_json_file(&path)
        }
    })
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

struct DatabaseLoadContext<'a> {
    tz: Option<&'a JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&'a PricingMap>,
    window: DateWindow,
    shared: &'a SharedArgs,
    db_path: &'a Path,
}

#[derive(Default)]
struct DatabaseLoadResult {
    message_entries: Vec<LoadedEntry>,
    aggregate_entries: Vec<LoadedEntry>,
}

#[derive(Default)]
struct DirectoryLoadResult {
    message_entries: Vec<LoadedEntry>,
    aggregate_entries: Vec<LoadedEntry>,
}

fn load_entries_from_database(
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
    window: DateWindow,
    allow_aggregate_fallback: bool,
) -> DatabaseLoadResult {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open OpenCode database: {}", db_path.display()),
        );
        return DatabaseLoadResult::default();
    };
    let mut message_entries = Vec::new();

    let mut seen_message_ids = HashSet::new();
    let mut message_sessions = HashSet::new();

    if table_exists(&connection, "message") {
        // Push the window into SQL only while a sample of `time_created` still
        // looks millisecond-scaled. The payload check remains authoritative.
        let pushdown =
            if window.is_unbounded() || time_created_looks_like_millis(&connection, "message") {
                window.widened_for_pushdown()
            } else {
                debug_log(
                    shared,
                    format!(
                        "OpenCode time_created is not millisecond-scale; scanning unfiltered: {}",
                        db_path.display()
                    ),
                );
                DateWindow::UNBOUNDED
            };
        let statement = prepare_message_query(&connection, pushdown).or_else(|| {
            // A pre-SQLite-era schema has no `time_created` column, so the
            // filtered query cannot prepare. Scan unfiltered instead.
            debug_log(
                shared,
                format!(
                    "Failed to prepare filtered OpenCode query; scanning unfiltered: {}",
                    db_path.display()
                ),
            );
            prepare_message_query(&connection, DateWindow::UNBOUNDED)
        });
        if let Some(mut statement) = statement {
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
                        if !window.is_unbounded()
                            && let Some(millis) = extract_message_timestamp(&data)
                            && !window.contains(millis)
                        {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<OpenCodeMessage>(&data) else {
                            continue;
                        };
                        if let Some(entry) = message_value_to_entry(
                            &value,
                            Some(id),
                            Some(session_id),
                            tz,
                            mode,
                            pricing,
                        ) {
                            let session_key = entry.session_id.to_string();
                            message_sessions.insert(session_key);
                            push_unique_entry(&mut message_entries, &mut seen_message_ids, entry);
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
        } else {
            debug_log(
                shared,
                format!("Failed to read OpenCode database: {}", db_path.display()),
            );
        }
    }

    let has_session_messages = table_exists(&connection, "session_message");
    if has_session_messages {
        let pushdown = if window.is_unbounded()
            || time_created_looks_like_millis(&connection, "session_message")
        {
            window.widened_for_pushdown()
        } else {
            DateWindow::UNBOUNDED
        };
        let statement = prepare_session_message_query(&connection, pushdown).or_else(|| {
            debug_log(
                shared,
                format!(
                    "Failed to prepare filtered OpenCode v2 query; scanning unfiltered: {}",
                    db_path.display()
                ),
            );
            prepare_session_message_query(&connection, DateWindow::UNBOUNDED)
        });
        let mut v2_rows = 0;
        let mut v2_entries = 0;
        if let Some(mut statement) = statement {
            loop {
                match statement.next() {
                    Ok(sqlite::State::Row) => {
                        v2_rows += 1;
                        let Ok(id) = statement.read::<String, _>(0) else {
                            continue;
                        };
                        let Ok(session_id) = statement.read::<String, _>(1) else {
                            continue;
                        };
                        let Ok(message_type) = statement.read::<String, _>(2) else {
                            continue;
                        };
                        if message_type != "assistant" {
                            continue;
                        }
                        let Ok(data) = statement.read::<String, _>(3) else {
                            continue;
                        };
                        let created = statement
                            .read::<i64, _>(4)
                            .ok()
                            .or_else(|| statement.read::<f64, _>(4).ok().map(|value| value as i64))
                            .unwrap_or(0);
                        if !window.is_unbounded()
                            && let Some(millis) = extract_message_timestamp(&data)
                            && !window.contains(millis)
                        {
                            continue;
                        }
                        let Some(entry) = session_message_value_to_entry(
                            &data, id, session_id, created, tz, mode, pricing,
                        ) else {
                            continue;
                        };
                        let session_key = entry.session_id.to_string();
                        v2_entries += 1;
                        message_sessions.insert(session_key);
                        push_unique_entry(&mut message_entries, &mut seen_message_ids, entry);
                    }
                    Ok(sqlite::State::Done) => break,
                    Err(_) => {
                        debug_log(
                            shared,
                            format!(
                                "Failed to query OpenCode v2 database: {}",
                                db_path.display()
                            ),
                        );
                        break;
                    }
                }
            }
        }
        if v2_rows > 0 && v2_entries == 0 {
            debug_log(
                shared,
                format!(
                    "OpenCode v2 rows produced no usage entries: {v2_rows} rows in {}",
                    db_path.display()
                ),
            );
        }
    }

    let mut aggregate_entries = Vec::new();
    if allow_aggregate_fallback && !has_temporal_filter(shared) {
        // Session aggregates are cumulative and cannot be sliced safely for a
        // bounded report, so only unbounded reports may use this fallback.
        let mut aggregate_tables = Vec::new();
        if table_exists(&connection, "session_v2") {
            aggregate_tables.push("session_v2");
        }
        if has_session_messages && table_exists(&connection, "session") {
            aggregate_tables.push("session");
        }
        let aggregate_context = DatabaseLoadContext {
            tz,
            mode,
            pricing,
            window,
            shared,
            db_path,
        };
        for table in aggregate_tables {
            for entry in load_session_aggregate_entries(&connection, table, &aggregate_context) {
                if message_sessions.contains(entry.session_id.as_ref()) {
                    continue;
                }
                push_unique_entry(&mut aggregate_entries, &mut seen_message_ids, entry);
            }
        }
    }

    DatabaseLoadResult {
        message_entries,
        aggregate_entries,
    }
}

fn read_message_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
    window: DateWindow,
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
    // Skip out-of-range entries before the full parse. Extraction works on the
    // raw text and fails open (non-UTF-8 or missing timestamp -> full parse).
    if !window.is_unbounded()
        && let Ok(text) = std::str::from_utf8(&content)
        && let Some(millis) = extract_message_timestamp(text)
        && !window.contains(millis)
    {
        return None;
    }
    let value = serde_json::from_slice::<OpenCodeMessage>(&content).ok()?;
    message_value_to_entry(&value, None, None, tz, mode, pricing)
}

fn push_unique_entry(
    entries: &mut Vec<LoadedEntry>,
    seen_message_ids: &mut HashSet<String>,
    entry: LoadedEntry,
) -> bool {
    if let Some(id) = entry_id(&entry)
        && !seen_message_ids.insert(id.to_string())
    {
        return false;
    }
    entries.push(entry);
    true
}

#[cfg(test)]
fn entry_ids(entries: &[LoadedEntry]) -> HashSet<String> {
    entries
        .iter()
        .filter_map(entry_id)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
fn message_session_ids(entries: &[LoadedEntry]) -> HashSet<String> {
    entries
        .iter()
        .map(|entry| entry.session_id.to_string())
        .collect()
}

fn append_aggregate_entries(
    entries: &mut Vec<LoadedEntry>,
    seen: &mut HashSet<String>,
    message_sessions: &HashSet<String>,
    aggregate_entries: Vec<LoadedEntry>,
) {
    for entry in aggregate_entries {
        if message_sessions.contains(entry.session_id.as_ref()) {
            continue;
        }
        if let Some(id) = entry_id(&entry)
            && !seen.insert(id.to_string())
        {
            continue;
        }
        entries.push(entry);
    }
}

fn table_exists(connection: &sqlite::Connection, table: &str) -> bool {
    let Ok(mut statement) = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
    else {
        return false;
    };
    if statement.bind((1, table)).is_err() {
        return false;
    }
    matches!(statement.next(), Ok(sqlite::State::Row))
}

fn table_columns(connection: &sqlite::Connection, table: &str) -> HashSet<String> {
    let sql = match table {
        "message" => "SELECT * FROM message LIMIT 0",
        "session" => "SELECT * FROM session LIMIT 0",
        "session_v2" => "SELECT * FROM session_v2 LIMIT 0",
        "session_message" => "SELECT * FROM session_message LIMIT 0",
        _ => return HashSet::new(),
    };
    let Ok(statement) = connection.prepare(sql) else {
        return HashSet::new();
    };
    let mut columns = HashSet::new();
    for index in 0..statement.column_count() {
        if let Ok(name) = statement.column_name(index) {
            columns.insert(name.to_string());
        }
    }
    columns
}

fn prepare_session_message_query(
    connection: &sqlite::Connection,
    window: DateWindow,
) -> Option<sqlite::Statement<'_>> {
    let columns = table_columns(connection, "session_message");
    if !["id", "session_id", "type", "data"]
        .into_iter()
        .all(|column| columns.contains(column))
    {
        return None;
    }
    let has_time_created = columns.contains("time_created");
    let time_created = if has_time_created {
        "time_created"
    } else {
        "NULL"
    };
    let sql = if has_time_created {
        match (window.start, window.end) {
            (Some(_), Some(_)) => format!(
                "SELECT id, session_id, type, data, {time_created} FROM session_message \
                 WHERE time_created >= ?1 AND time_created < ?2"
            ),
            (Some(_), None) => format!(
                "SELECT id, session_id, type, data, {time_created} FROM session_message \
                 WHERE time_created >= ?1"
            ),
            (None, Some(_)) => format!(
                "SELECT id, session_id, type, data, {time_created} FROM session_message \
                 WHERE time_created < ?1"
            ),
            (None, None) => {
                format!("SELECT id, session_id, type, data, {time_created} FROM session_message")
            }
        }
    } else {
        "SELECT id, session_id, type, data, NULL FROM session_message".to_string()
    };
    let mut statement = connection.prepare(&sql).ok()?;
    if has_time_created {
        for (index, bound) in [window.start, window.end].into_iter().flatten().enumerate() {
            statement.bind((index + 1, bound)).ok()?;
        }
    }
    Some(statement)
}

fn prepare_session_aggregate_query<'a>(
    connection: &'a sqlite::Connection,
    table: &str,
) -> Option<sqlite::Statement<'a>> {
    let table = match table {
        "session" => "session",
        "session_v2" => "session_v2",
        _ => return None,
    };
    let columns = table_columns(connection, table);
    if ![
        "id",
        "time_created",
        "cost",
        "tokens_input",
        "tokens_output",
        "tokens_cache_read",
        "tokens_cache_write",
    ]
    .into_iter()
    .all(|column| columns.contains(column))
    {
        return None;
    }
    let reasoning = if columns.contains("tokens_reasoning") {
        "tokens_reasoning"
    } else {
        "0"
    };
    let model = if columns.contains("model") {
        "model"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT id, time_created, cost, tokens_input, tokens_output, \
         tokens_cache_read, tokens_cache_write, {reasoning}, {model} FROM {table}"
    );
    connection.prepare(&sql).ok()
}

fn load_session_aggregate_entries(
    connection: &sqlite::Connection,
    table: &str,
    context: &DatabaseLoadContext<'_>,
) -> Vec<LoadedEntry> {
    let Some(mut statement) = prepare_session_aggregate_query(connection, table) else {
        debug_log(
            context.shared,
            format!(
                "Failed to read OpenCode {table} table: {}",
                context.db_path.display()
            ),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(session_id) = statement.read::<String, _>(0) else {
                    continue;
                };
                let Some(created) = read_f64(&statement, 1)
                    .filter(|value| value.is_finite())
                    .map(|value| value.trunc() as i64)
                else {
                    continue;
                };
                if !context.window.is_unbounded() && !context.window.contains(created) {
                    continue;
                }
                let cost = read_f64(&statement, 2);
                let input_tokens = read_u64(&statement, 3);
                let output_tokens = read_u64(&statement, 4);
                let cache_read_tokens = read_u64(&statement, 5);
                let cache_write_tokens = read_u64(&statement, 6);
                let reasoning_tokens = read_u64(&statement, 7);
                let model = statement.read::<String, _>(8).ok();
                let (model, provider) = parse_session_model(model.as_deref())
                    .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
                if let Some(entry) = session_value_to_entry(
                    OpenCodeSessionAggregate {
                        session_id,
                        created,
                        model,
                        provider,
                        input_tokens,
                        output_tokens,
                        reasoning_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        cost,
                    },
                    context.tz,
                    context.mode,
                    context.pricing,
                ) {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    context.shared,
                    format!(
                        "Failed to query OpenCode {table} table: {}",
                        context.db_path.display()
                    ),
                );
                break;
            }
        }
    }
    entries
}

fn parse_session_model(value: Option<&str>) -> Option<(String, String)> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(model) = json
            .as_str()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return Some((model.to_string(), "unknown".to_string()));
        }
        if let Some(object) = json.as_object() {
            let model = object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("modelID").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|model| !model.is_empty())?;
            let provider = object
                .get("providerID")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("provider").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .unwrap_or("unknown");
            return Some((model.to_string(), provider.to_string()));
        }
    }
    Some((value.to_string(), "unknown".to_string()))
}

fn read_u64(statement: &sqlite::Statement<'_>, index: usize) -> u64 {
    statement
        .read::<i64, _>(index)
        .ok()
        .and_then(|value| u64::try_from(value.max(0)).ok())
        .or_else(|| {
            statement
                .read::<f64, _>(index)
                .ok()
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| value.trunc() as u64)
        })
        .unwrap_or(0)
}

fn read_f64(statement: &sqlite::Statement<'_>, index: usize) -> Option<f64> {
    statement
        .read::<f64, _>(index)
        .ok()
        .filter(|value| value.is_finite())
        .or_else(|| {
            statement
                .read::<i64, _>(index)
                .ok()
                .map(|value| value as f64)
        })
}

fn entry_id(entry: &LoadedEntry) -> Option<&str> {
    entry.data.message.id.as_deref().filter(|id| !id.is_empty())
}

/// Pulls `time.created` millis from raw JSON to skip rows before a full parse.
///
/// Only the canonical `"time": { ... "created": <digits> ... }` shape is
/// recognized, and the search for `created` never leaves that object, so a
/// `time` object belonging to something else in the payload cannot contribute a
/// number. Everything else returns `None` and the caller falls back to a full
/// parse: a scan that gives up costs time, whereas a scan that guesses wrong
/// would silently drop an in-range entry.
fn extract_message_timestamp(data: &str) -> Option<i64> {
    const TIME_KEY: &str = "\"time\":";
    const CREATED_KEY: &str = "\"created\":";

    let time_object = data[data.find(TIME_KEY)? + TIME_KEY.len()..]
        .trim_start()
        .strip_prefix('{')?;
    let time_object = &time_object[..time_object.find('}')?];
    let after_key = time_object[time_object.find(CREATED_KEY)? + CREATED_KEY.len()..].trim_start();
    let end = after_key
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(after_key.len());
    after_key[..end].parse::<i64>().ok()
}

/// Half-open millisecond window derived from `--since`/`--until`, used to skip
/// rows and files before they are parsed.
///
/// A `None` bound is not narrowed, either because the option was absent or
/// because it is not a full date. The window is deliberately equivalent to the
/// authoritative `date_within_range` check applied to loaded entries: both
/// resolve the bounds in the reporting timezone, so pre-filtering can never drop
/// an entry the report would have kept.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DateWindow {
    start: Option<i64>,
    end: Option<i64>,
}

fn has_temporal_filter(shared: &SharedArgs) -> bool {
    shared.since.is_some() || shared.until.is_some() || shared.last.is_some()
}

impl DateWindow {
    const UNBOUNDED: Self = Self {
        start: None,
        end: None,
    };

    fn from_shared(shared: &SharedArgs, tz: Option<&JiffTimeZone>) -> Self {
        let (start, end) =
            date_range_bounds_ms(shared.since.as_deref(), shared.until.as_deref(), tz);
        Self { start, end }
    }

    fn is_unbounded(self) -> bool {
        self == Self::UNBOUNDED
    }

    /// Same window, widened by a day on each side for the SQL push-down.
    ///
    /// SQL filters on `message.time_created` while the report filters on the
    /// payload's `time.created`. They hold the same value on every OpenCode
    /// build checked here, but the column is only a proxy for the payload, so
    /// the pushed-down window is kept loose: a column drifting from its payload
    /// by up to a day costs a few extra rows to scan instead of excluding a row
    /// the report wanted. Drift past a day is still excluded — ruling that out
    /// would take the full-column scan this push-down exists to avoid. The exact
    /// window is applied per row either way.
    fn widened_for_pushdown(self) -> Self {
        Self {
            start: self.start.map(|start| start - crate::MILLIS_PER_DAY),
            end: self.end.map(|end| end + crate::MILLIS_PER_DAY),
        }
    }

    fn contains(self, millis: i64) -> bool {
        self.start.is_none_or(|start| millis >= start) && self.end.is_none_or(|end| millis < end)
    }
}

/// Prepares the `message` scan, narrowed to `window` where its bounds are set.
///
/// Returns `None` when the statement cannot be prepared, which is what a schema
/// without a `time_created` column looks like.
///
/// The bounds are applied through a subquery that selects only `id`. OpenCode's
/// index is `(session_id, time_created, id)`, so a bare `time_created` range
/// cannot seek it and scans the table, reading every `data` blob on the way. The
/// subquery is answered from that index alone, leaving only in-range rows to be
/// fetched by primary key — the difference is what keeps a narrow window off the
/// gigabytes of payload it does not need.
fn prepare_message_query(
    connection: &sqlite::Connection,
    window: DateWindow,
) -> Option<sqlite::Statement<'_>> {
    let sql = match (window.start, window.end) {
        (Some(_), Some(_)) => {
            "SELECT id, session_id, data FROM message WHERE id IN \
             (SELECT id FROM message WHERE time_created >= ?1 AND time_created < ?2)"
        }
        (Some(_), None) => {
            "SELECT id, session_id, data FROM message WHERE id IN \
             (SELECT id FROM message WHERE time_created >= ?1)"
        }
        (None, Some(_)) => {
            "SELECT id, session_id, data FROM message WHERE id IN \
             (SELECT id FROM message WHERE time_created < ?1)"
        }
        (None, None) => "SELECT id, session_id, data FROM message",
    };
    let mut statement = connection.prepare(sql).ok()?;
    for (index, bound) in [window.start, window.end].into_iter().flatten().enumerate() {
        statement.bind((index + 1, bound)).ok()?;
    }
    Some(statement)
}

/// Smallest value treated as millisecond scale: any Unix timestamp after 1973
/// needs at least 12 digits, while second-scale values stay far below it.
const MIN_MILLIS_SCALE: i64 = 100_000_000_000;

/// Reports whether a sample of `message.time_created` looks like Unix
/// milliseconds, the scale the payload's `time.created` uses.
///
/// Sampling keeps this cheap on databases tens of gigabytes in size, and proves
/// nothing about the rows it did not read: a column with mixed scales can still
/// slip through, and matching scales say nothing about matching values. What it
/// does catch is a build that stored seconds, or left the column at zero, where
/// millisecond bounds would otherwise exclude every row — those disable the
/// push-down and leave the payload check to filter.
fn time_created_looks_like_millis(connection: &sqlite::Connection, table: &str) -> bool {
    let sql = match table {
        "message" => "SELECT max(time_created) FROM (SELECT time_created FROM message LIMIT 8)",
        "session_message" => {
            "SELECT max(time_created) FROM (SELECT time_created FROM session_message LIMIT 8)"
        }
        _ => return false,
    };
    if !table_columns(connection, table).contains("time_created") {
        return false;
    }
    let Ok(mut statement) = connection.prepare(sql) else {
        return false;
    };
    match statement.next() {
        Ok(sqlite::State::Row) => statement
            .read::<i64, _>(0)
            .is_ok_and(|max| max >= MIN_MILLIS_SCALE),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::Path};

    use super::{
        load_entries, load_entries_from_directory, load_entries_from_directory_for_report,
    };
    use crate::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    // Mirrors the real OpenCode schema, where `time_created` repeats the
    // payload's `time.created`, so tests exercise the range push-down.
    fn create_db_message(path: &Path, id: &str, session_id: &str, data: &str) {
        let created = serde_json::from_str::<serde_json::Value>(data)
            .ok()
            .and_then(|value| value["time"]["created"].as_i64())
            .expect("test message payload needs time.created");
        create_db_message_with_time(path, id, session_id, created, data);
    }

    // Schema with `time_created` set independently of the payload, for the cases
    // where the column and the payload have to disagree.
    fn create_db_message_with_time(
        path: &Path,
        id: &str,
        session_id: &str,
        time_created_ms: i64,
        data: &str,
    ) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "CREATE TABLE IF NOT EXISTS message \
             (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER NOT NULL DEFAULT 0, data TEXT)",
        )
        .unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
            )
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, time_created_ms)).unwrap();
        statement.bind((4, data)).unwrap();
        statement.next().unwrap();
    }

    // Pre-SQLite-era layout: no `time_created` column, so the filtered query
    // cannot prepare and the loader has to fall back to an unfiltered scan.
    fn create_db_message_legacy_schema(path: &Path, id: &str, session_id: &str, data: &str) {
        let db = sqlite::open(path).unwrap();
        db.execute("CREATE TABLE IF NOT EXISTS message (id TEXT, session_id TEXT, data TEXT)")
            .unwrap();
        let mut statement = db
            .prepare("INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)")
            .unwrap();
        statement.bind((1, id)).unwrap();
        statement.bind((2, session_id)).unwrap();
        statement.bind((3, data)).unwrap();
        statement.next().unwrap();
    }

    fn create_db_session_message_table(path: &Path, with_time_created: bool) {
        let db = sqlite::open(path).unwrap();
        let schema = if with_time_created {
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER, data TEXT)"
        } else {
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, data TEXT)"
        };
        db.execute(schema).unwrap();
    }

    fn insert_db_session_message(
        path: &Path,
        id: &str,
        session_id: &str,
        message_type: &str,
        time_created: i64,
        data: &str,
        with_time_created: bool,
    ) {
        let db = sqlite::open(path).unwrap();
        if with_time_created {
            let mut statement = db
                .prepare(
                    "INSERT INTO session_message (id, session_id, type, time_created, data) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .unwrap();
            statement.bind((1, id)).unwrap();
            statement.bind((2, session_id)).unwrap();
            statement.bind((3, message_type)).unwrap();
            statement.bind((4, time_created)).unwrap();
            statement.bind((5, data)).unwrap();
            statement.next().unwrap();
        } else {
            let mut statement = db
                .prepare(
                    "INSERT INTO session_message (id, session_id, type, data) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .unwrap();
            statement.bind((1, id)).unwrap();
            statement.bind((2, session_id)).unwrap();
            statement.bind((3, message_type)).unwrap();
            statement.bind((4, data)).unwrap();
            statement.next().unwrap();
        }
    }

    fn create_db_session_aggregate_table(path: &Path, table: &str) {
        let schema = match table {
            "session_v2" => {
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT)"
            }
            "session" => {
                "CREATE TABLE session (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT)"
            }
            _ => panic!("unsupported test session table: {table}"),
        };
        sqlite::open(path).unwrap().execute(schema).unwrap();
    }

    struct SessionAggregateFixture<'a> {
        session_id: &'a str,
        time_created: i64,
        model: &'a str,
        cost: f64,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
    }

    fn insert_db_session_aggregate(
        path: &Path,
        table: &str,
        fixture: &SessionAggregateFixture<'_>,
    ) {
        let sql = match table {
            "session_v2" => {
                "INSERT INTO session_v2 (id, time_created, cost, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            }
            "session" => {
                "INSERT INTO session (id, time_created, cost, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            }
            _ => panic!("unsupported test session table: {table}"),
        };
        let db = sqlite::open(path).unwrap();
        let mut statement = db.prepare(sql).unwrap();
        statement.bind((1, fixture.session_id)).unwrap();
        statement.bind((2, fixture.time_created)).unwrap();
        statement.bind((3, fixture.cost)).unwrap();
        statement.bind((4, fixture.input_tokens)).unwrap();
        statement.bind((5, fixture.output_tokens)).unwrap();
        statement.bind((6, fixture.cache_read_tokens)).unwrap();
        statement.bind((7, fixture.cache_write_tokens)).unwrap();
        statement.bind((8, fixture.reasoning_tokens)).unwrap();
        statement.bind((9, fixture.model)).unwrap();
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
    fn loads_nested_v2_assistant_usage_and_ignores_session_aggregate() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_db_session_message(
            &db_path,
            "msg-v2-user",
            "v2-session",
            "user",
            1_767_312_000_000,
            r#"{"text":"hello","time":{"created":1767312000000}}"#,
            true,
        );
        insert_db_session_message(
            &db_path,
            "msg-v2-assistant",
            "v2-session",
            "assistant",
            1_767_312_000_001,
            r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":120,"output":60,"reasoning":10,"cache":{"read":12,"write":24}},"cost":0.03}"#,
            true,
        );
        create_db_session_aggregate_table(&db_path, "session");
        insert_db_session_aggregate(
            &db_path,
            "session",
            &SessionAggregateFixture {
                session_id: "v2-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &shared,
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("msg-v2-assistant")
        );
        assert_eq!(entries[0].session_id.as_ref(), "v2-session");
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].data.message.usage.output_tokens, 60);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            24
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 12);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn loads_v2_messages_without_a_time_created_column() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, false);
        insert_db_session_message(
            &db_path,
            "msg-v2-no-time-column",
            "v2-session",
            "assistant",
            1_767_312_000_000,
            r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":2,"output":3},"cost":0.01}"#,
            false,
        );

        let entries = load_entries_from_directory(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("msg-v2-no-time-column")
        );
    }

    #[test]
    fn deduplicates_legacy_and_v2_rows_by_message_id() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_message(
            &db_path,
            "msg-shared",
            "legacy-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
        );
        create_db_session_message_table(&db_path, true);
        insert_db_session_message(
            &db_path,
            "msg-shared",
            "v2-session",
            "assistant",
            1_767_312_000_000,
            r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":9.99}"#,
            true,
        );

        let entries = load_entries_from_directory(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "legacy-session");
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn suppresses_session_aggregate_when_legacy_json_has_message() {
        let fixture = fs_fixture!({
            "storage/message/legacy-json-session/legacy-json-message.json": r#"{"id":"legacy-json-message","sessionID":"legacy-json-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#,
        });
        let db_path = fixture.path("opencode.db");
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "legacy-json-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "legacy-json-session");
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("legacy-json-message")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn uses_session_v2_aggregate_when_no_message_level_usage_exists() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "aggregate-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 0.25,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_write_tokens: 20,
                reasoning_tokens: 5,
            },
        );

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "aggregate-session");
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("session:aggregate-session")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 5);
        assert_eq!(entries[0].cost, 0.25);
    }

    #[test]
    fn excludes_cumulative_session_aggregate_from_bounded_window() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "spanning-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 0.25,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_write_tokens: 20,
                reasoning_tokens: 5,
            },
        );

        // The cumulative aggregate includes usage outside this window, so it
        // cannot be attributed safely to the requested date range.
        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                since: Some("20260102".to_string()),
                until: Some("20260103".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn keeps_message_rows_but_excludes_aggregate_for_partial_since_bound() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_message(
            &db_path,
            "partial-since-message",
            "partial-since-message-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1},"cost":0.01}"#,
        );
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "partial-since-aggregate-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                since: Some("2026".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("partial-since-message")
        );
    }

    #[test]
    fn keeps_message_rows_but_excludes_aggregate_for_partial_until_bound() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_message(
            &db_path,
            "partial-until-message",
            "partial-until-message-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1},"cost":0.01}"#,
        );
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "partial-until-aggregate-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                until: Some("2026-02".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("partial-until-message")
        );
    }

    #[test]
    fn suppresses_aggregate_when_message_usage_is_in_another_configured_directory() {
        let aggregate_fixture = fs_fixture!({});
        let aggregate_db_path = aggregate_fixture.path("opencode.db");
        create_db_session_aggregate_table(&aggregate_db_path, "session_v2");
        insert_db_session_aggregate(
            &aggregate_db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "cross-directory-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );
        let message_fixture = fs_fixture!({
            "storage/message/cross-directory-session/cross-directory-message.json": r#"{"id":"cross-directory-message","sessionID":"cross-directory-session","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":12,"output":6},"cost":0.03}"#,
        });
        let _guard = EnvVarsGuard::set_many([(
            "OPENCODE_DATA_DIR",
            Some(OsString::from(format!(
                "{},{}",
                aggregate_fixture.root().display(),
                message_fixture.root().display()
            ))),
        )]);

        let entries = load_entries(
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("cross-directory-message")
        );
        assert_eq!(entries[0].cost, 0.03);
    }

    #[test]
    fn only_session_reports_use_cumulative_session_aggregate_fallback() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_message(
            &db_path,
            "period-message",
            "period-message-session",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1},"cost":0.01}"#,
        );
        create_db_session_aggregate_table(&db_path, "session_v2");
        insert_db_session_aggregate(
            &db_path,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "period-aggregate-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 9.99,
                input_tokens: 9_999,
                output_tokens: 9_999,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            },
        );
        let _guard = EnvVarsGuard::set_many([(
            "OPENCODE_DATA_DIR",
            Some(fixture.root().as_os_str().to_os_string()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        for report_kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
        ] {
            let entries = load_entries(&shared, report_kind).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].data.message.id.as_deref(),
                Some("period-message")
            );
        }

        let session_entries = load_entries(&shared, AgentReportKind::Session).unwrap();
        assert_eq!(session_entries.len(), 2);
        assert!(session_entries.iter().any(|entry| {
            entry.data.message.id.as_deref() == Some("session:period-aggregate-session")
        }));
    }

    #[test]
    fn uses_current_session_table_as_v2_aggregate_fallback() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        create_db_session_aggregate_table(&db_path, "session");
        insert_db_session_aggregate(
            &db_path,
            "session",
            &SessionAggregateFixture {
                session_id: "current-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 0.5,
                input_tokens: 200,
                output_tokens: 100,
                cache_read_tokens: 20,
                cache_write_tokens: 40,
                reasoning_tokens: 0,
            },
        );

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "current-session");
        assert_eq!(entries[0].data.message.usage.input_tokens, 200);
        assert_eq!(entries[0].cost, 0.5);
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

    #[test]
    fn since_filter_drops_db_rows_older_than_lower_bound() {
        let fixture = fs_fixture!({});
        // 2025-12-31 00:00 UTC
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-old",
            "session-old",
            1_767_139_200_000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767139200000},"tokens":{"input":1,"output":1}}"#,
        );
        // 2026-01-04 00:00 UTC, in range for since=20260103
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-new",
            "session-new",
            1_767_484_800_000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767484800000},"tokens":{"input":2,"output":2}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260103".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("msg-new"));
    }

    #[test]
    fn until_filter_drops_db_rows_at_or_after_upper_bound() {
        let fixture = fs_fixture!({});
        // 2026-01-02 00:00 UTC, in range for until=20260105
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-early",
            "session-early",
            1_767_312_000_000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
        );
        // 2026-01-11 00:00 UTC, out of range for until=20260105
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-late",
            "session-late",
            1_768_089_600_000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1768089600000},"tokens":{"input":2,"output":2}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            until: Some("20260105".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("msg-early"));
    }

    #[test]
    fn legacy_schema_without_time_created_still_returns_in_range_rows() {
        let fixture = fs_fixture!({});
        create_db_message_legacy_schema(
            &fixture.path("opencode.db"),
            "msg-in-range",
            "session-a",
            // payload date 2026-01-05, inside the requested window
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767571200000},"tokens":{"input":3,"output":3}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260103".to_string()),
            until: Some("20260107".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("msg-in-range"));
    }

    #[test]
    fn legacy_schema_without_time_created_still_drops_out_of_range_rows() {
        let fixture = fs_fixture!({});
        create_db_message_legacy_schema(
            &fixture.path("opencode.db"),
            "msg-out-of-range",
            "session-a",
            // payload date 2026-01-02, before since=20260103
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":3,"output":3}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260103".to_string()),
            until: Some("20260107".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert!(
            entries.is_empty(),
            "fallback scan must still exclude out-of-range rows via the in-loop check"
        );
    }

    #[test]
    fn filters_json_file_entries_by_until() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            until: Some("20260101".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert!(
            entries.is_empty(),
            "message on 2026-01-02 should be excluded by until=20260101"
        );
    }

    #[test]
    fn filters_json_file_entries_by_since() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260103".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert!(
            entries.is_empty(),
            "message on 2026-01-02 should be excluded by since=20260103"
        );
    }

    #[test]
    fn includes_entries_when_since_until_bracket_date() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260101".to_string()),
            until: Some("20260103".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "message on 2026-01-02 should be included when since=20260101 and until=20260103"
        );
    }

    #[test]
    fn includes_entries_when_since_exact_match() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260102".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "message on 2026-01-02 should be included when since=20260102"
        );
    }

    #[test]
    fn includes_entries_when_until_exact_match() {
        let fixture = fs_fixture!({
            "storage/message/message.json": r#"{"id":"msg-1","sessionID":"session-a","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50},"cost":0.02}"#,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            until: Some("20260102".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "message on 2026-01-02 should be included when until=20260102"
        );
    }

    // Real OpenCode message files are pretty-printed (newlines + indentation),
    // unlike the minified fixtures above. Pins `extract_message_timestamp`
    // against the real on-disk shape.
    const PRETTY_PRINTED_MESSAGE: &str = r#"{
  "id": "msg-pretty",
  "sessionID": "session-a",
  "role": "assistant",
  "providerID": "anthropic",
  "modelID": "claude-sonnet-4-20250514",
  "time": {
    "created": 1767312000000,
    "completed": 1767312001000
  },
  "tokens": {
    "input": 100,
    "output": 50
  },
  "cost": 0.02
}"#;

    #[test]
    fn extracts_timestamp_from_pretty_printed_file_when_out_of_range() {
        let fixture = fs_fixture!({
            "storage/message/message.json": PRETTY_PRINTED_MESSAGE,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            until: Some("20260101".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert!(
            entries.is_empty(),
            "pretty-printed message on 2026-01-02 must be excluded by until=20260101"
        );
    }

    #[test]
    fn extracts_timestamp_from_pretty_printed_file_when_in_range() {
        let fixture = fs_fixture!({
            "storage/message/message.json": PRETTY_PRINTED_MESSAGE,
        });

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260101".to_string()),
            until: Some("20260103".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("msg-pretty"));
    }

    // 2026-01-01 12:00 UTC, which is 2026-01-02 in UTC+14 and therefore only
    // kept when the lower bound is resolved in the reporting timezone.
    const NOON_UTC_2026_01_01: &str = r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767268800000},"tokens":{"input":1,"output":1}}"#;

    // 2026-01-02 06:00 UTC, which is still 2026-01-01 in UTC-12.
    const EARLY_UTC_2026_01_02: &str = r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767333600000},"tokens":{"input":1,"output":1}}"#;

    #[test]
    fn since_bound_follows_local_midnight_in_the_reporting_timezone() {
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode.db"),
            "msg-utc-plus-14",
            "session-a",
            NOON_UTC_2026_01_01,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("Pacific/Kiritimati".to_string()),
            since: Some("20260102".to_string()),
            until: Some("20260102".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(
            entries.len(),
            1,
            "a UTC+14 local date of 2026-01-02 must survive since=until=20260102"
        );
        assert_eq!(entries[0].date, "2026-01-02");
    }

    #[test]
    fn until_bound_follows_local_midnight_in_the_reporting_timezone() {
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode.db"),
            "msg-utc-minus-12",
            "session-a",
            EARLY_UTC_2026_01_02,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("Etc/GMT+12".to_string()),
            until: Some("20260101".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(
            entries.len(),
            1,
            "a UTC-12 local date of 2026-01-01 must survive until=20260101"
        );
        assert_eq!(entries[0].date, "2026-01-01");
    }

    #[test]
    fn pushdown_margin_keeps_rows_whose_column_drifts_from_the_payload() {
        let fixture = fs_fixture!({});
        // Payload lands on 2026-01-02, but the column sits 26 hours later, which
        // an exact SQL window would push past its upper bound.
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-drifted",
            "session-a",
            1_767_312_000_000 + 26 * 60 * 60 * 1000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260102".to_string()),
            until: Some("20260102".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(
            entries.len(),
            1,
            "the payload decides the window, so a drifting column must not exclude the row"
        );
        assert_eq!(entries[0].date, "2026-01-02");
    }

    #[test]
    fn second_scale_time_created_disables_the_range_pushdown() {
        let fixture = fs_fixture!({});
        // The payload is on 2026-01-02, but the column holds seconds. Comparing
        // it against millisecond bounds would exclude the row outright.
        create_db_message_with_time(
            &fixture.path("opencode.db"),
            "msg-seconds",
            "session-a",
            1_767_312_000,
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20260101".to_string()),
            until: Some("20260103".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(
            entries.len(),
            1,
            "an unrecognized time_created scale must fall back to scanning, not drop rows"
        );
        assert_eq!(entries[0].data.message.id.as_deref(), Some("msg-seconds"));
    }

    #[test]
    fn non_ascii_date_bounds_leave_filtering_to_the_report() {
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode.db"),
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            // Multi-byte bound: 8 bytes long, but not 8 ASCII digits.
            since: Some("abあcde".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries_from_directory(fixture.root(), &shared).unwrap();

        assert_eq!(
            entries.len(),
            1,
            "a bound with no instant must not pre-filter; the report's string filter decides"
        );
        assert!(!crate::date_within_range(
            &entries[0].date,
            shared.since.as_deref(),
            None
        ));
    }

    #[test]
    fn detects_sources_regardless_of_the_date_window() {
        let db_only = fs_fixture!({});
        create_db_message(
            &db_only.path("opencode.db"),
            "msg-1",
            "session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":1,"output":1}}"#,
        );
        assert!(super::has_source(db_only.root()));

        let files_only = fs_fixture!({
            "storage/message/session-a/msg-1.json": r#"{"id":"msg-1"}"#,
        });
        assert!(super::has_source(files_only.root()));

        let empty = fs_fixture!({});
        assert!(!super::has_source(empty.root()));

        // A window that excludes every entry must not make the source vanish;
        // that is what keeps OpenCode in the aggregate report's detected list.
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            since: Some("20200101".to_string()),
            until: Some("20200102".to_string()),
            ..SharedArgs::default()
        };
        assert!(
            load_entries_from_directory(db_only.root(), &shared)
                .unwrap()
                .is_empty()
        );
        assert!(super::has_source(db_only.root()));
    }

    #[test]
    fn extracts_timestamp_from_minified_and_pretty_printed_payloads() {
        assert_eq!(
            super::extract_message_timestamp(r#"{"time":{"created":1767312000000}}"#),
            Some(1_767_312_000_000)
        );
        assert_eq!(
            super::extract_message_timestamp(PRETTY_PRINTED_MESSAGE),
            Some(1_767_312_000_000)
        );
        // Digits running to the end of the object, without a trailing comma.
        assert_eq!(
            super::extract_message_timestamp(r#"{"time":{"created": 42}}"#),
            Some(42)
        );
    }

    #[test]
    fn ignores_a_time_object_that_is_not_the_messages_own() {
        // The scan must not reach past the first `time` object for a `created`
        // key: guessing wrong here would drop an in-range message, while giving
        // up only costs a full parse.
        assert_eq!(
            super::extract_message_timestamp(
                r#"{"parts":[{"time":{"start":1}}],"time":{"created":1767312000000}}"#
            ),
            None
        );
        // A quoted `"time"` that is a value rather than a key is not followed by
        // a colon, so it cannot start a match either.
        assert_eq!(
            super::extract_message_timestamp(r#"{"unit":"time","created":1767312000000}"#),
            None
        );
    }

    #[test]
    fn declines_to_extract_timestamps_it_cannot_trust() {
        // Quoted numbers, negative values and missing keys all fail open so the
        // full parse decides.
        assert_eq!(
            super::extract_message_timestamp(r#"{"time":{"created":"1767312000000"}}"#),
            None
        );
        assert_eq!(
            super::extract_message_timestamp(r#"{"time":{"created":-5}}"#),
            None
        );
        assert_eq!(
            super::extract_message_timestamp(r#"{"time":{"completed":1767312000000}}"#),
            None
        );
        assert_eq!(super::extract_message_timestamp(r#"{"id":"msg-1"}"#), None);
    }
}
