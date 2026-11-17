use ccusage_adapter_common::{
    SqliteStage, dir_entries, jsonl, read_units_flat,
    sqlite::{read_count, read_f64, read_i64},
    units::{UnitError, decode_json},
};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ccusage_adapter_common::source::ProgressOwner;
use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        MissingTimestamp, OpenCodeMessage, OpenCodeSessionAggregate, SessionMessageError,
        SessionMessageKind, SessionMessageRow, message_value_to_entry,
        session_message_value_to_entry_checked, session_value_to_entry,
    },
    paths::paths,
};
use crate::{
    LoadedEntry, PricingMap, Result,
    cli::{AgentReportKind, CostMode, SharedArgs},
    collect_files_with_extension, date_range_bounds_ms, debug_log, resolve_tz,
};

/// An entry and the Claude Code session its `claude-agent` row ran in.
pub type LinkedEntry = (LoadedEntry, Option<Arc<str>>);

/// Loads OpenCode usage entries, each paired with the Claude Code session its
/// `claude-agent` row ran in, so a unified report can skip the turns the
/// Claude source already counts.
pub fn load_entries_with_native_sessions(
    shared: &SharedArgs,
    report_kind: AgentReportKind,
) -> Result<Vec<LinkedEntry>> {
    load_entries_with_native_sessions_with_progress(shared, report_kind, ProgressOwner::Source)
}

pub(super) fn load_entries_with_native_sessions_with_diagnostics(
    shared: &SharedArgs,
    report_kind: AgentReportKind,
    owner: ProgressOwner,
    diagnostics: &Diagnostics,
) -> Result<Vec<LinkedEntry>> {
    let load = || {
        Ok(load_directories(
            &paths()?,
            shared,
            report_kind,
            diagnostics,
        ))
    };
    match owner {
        ProgressOwner::Source => crate::progress::track_usage_load(
            crate::progress::UsageLoadAgent("OpenCode"),
            shared.json,
            load,
        ),
        ProgressOwner::Caller => load(),
    }
}

fn load_directories(
    directories: &[PathBuf],
    shared: &SharedArgs,
    report_kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Vec<LinkedEntry> {
    let allow_aggregate_fallback = report_kind == AgentReportKind::Session;
    let pricing = if shared.mode == CostMode::Display {
        None
    } else {
        Some(PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    let tz = resolve_tz(shared.timezone.as_deref());
    let window = DateWindow::from_shared(shared, &tz);
    let mut collector = EntryCollector::default();
    for directory in directories {
        if let Some(db_path) = db_path(directory, diagnostics) {
            load_entries_from_database(
                &DatabaseLoadContext {
                    db_path: &db_path,
                    tz: &tz,
                    mode: shared.mode,
                    pricing: pricing.as_ref(),
                    shared,
                    window,
                    diagnostics,
                },
                allow_aggregate_fallback,
                &mut collector,
            );
        }

        let messages_dir = directory.join("storage").join("message");
        let mut files = Vec::new();
        collect_files_with_extension(&messages_dir, "json", &mut files, diagnostics);

        files.retain(|file| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !collector.has_claimed(stem))
        });

        // Read the surviving files in parallel, then run the sequential id dedup
        // over the results in their original file order so parallelism never changes
        // which duplicate survives.
        let loaded = read_units_flat(&files, shared.single_thread, diagnostics, |file| {
            read_message_file(file, &tz, shared.mode, pricing.as_ref(), window)
        });
        for entry in loaded {
            collector.push_message((entry, None));
        }
    }
    collector.finish()
}

#[derive(Default)]
struct EntryCollector {
    claimed_ids: HashSet<String>,
    message_sessions: HashSet<String>,
    messages: Vec<LinkedEntry>,
    compactions: Vec<LinkedEntry>,
    aggregates: Vec<LoadedEntry>,
}

impl EntryCollector {
    fn claim(&mut self, entry: &LoadedEntry) -> bool {
        entry_id(entry).is_none_or(|id| self.claimed_ids.insert(id.to_string()))
    }

    fn has_claimed(&self, id: &str) -> bool {
        self.claimed_ids.contains(id)
    }

    fn push_message(&mut self, linked: LinkedEntry) {
        self.message_sessions
            .insert(linked.0.session_id.to_string());
        if self.claim(&linked.0) {
            self.messages.push(linked);
        }
    }

    fn push_compaction(&mut self, linked: LinkedEntry) {
        self.compactions.push(linked);
    }

    fn push_aggregate(&mut self, entry: LoadedEntry) {
        self.aggregates.push(entry);
    }

    /// Adds session aggregates for sessions without assistant or legacy message
    /// usage, then compaction rows for sessions no aggregate covered. A session
    /// aggregate already includes the session's compaction usage.
    fn finish(mut self) -> Vec<LinkedEntry> {
        let mut entries = std::mem::take(&mut self.messages);
        let mut aggregate_sessions = HashSet::new();
        for entry in std::mem::take(&mut self.aggregates) {
            if self.message_sessions.contains(entry.session_id.as_ref()) || !self.claim(&entry) {
                continue;
            }
            aggregate_sessions.insert(entry.session_id.to_string());
            entries.push((entry, None));
        }
        for linked in std::mem::take(&mut self.compactions) {
            if !aggregate_sessions.contains(linked.0.session_id.as_ref()) && self.claim(&linked.0) {
                entries.push(linked);
            }
        }
        entries.sort_by_key(|(entry, _)| entry.timestamp);
        entries
    }
}

/// Reports whether `opencode_dir` holds any usage source at all: the SQLite
/// database, or at least one message file.
///
/// Detection has to ignore `--since`/`--until` because the loader applies the
/// window while reading, so an out-of-range query returns no entries even on an
/// install full of logs. Stops at the first message file rather than collecting
/// them all, so this stays cheap next to a large legacy dump.
pub(crate) fn has_source(opencode_dir: &Path) -> bool {
    if db_path(opencode_dir, &Diagnostics::new("opencode")).is_some() {
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

fn db_path(opencode_dir: &Path, diagnostics: &Diagnostics) -> Option<PathBuf> {
    let default_path = opencode_dir.join("opencode.db");
    if default_path.is_file() {
        return Some(default_path);
    }
    let mut candidates = dir_entries(opencode_dir, diagnostics)
        .into_iter()
        .map(|(path, _)| path)
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
    tz: &'a JiffTimeZone,
    mode: CostMode,
    pricing: Option<&'a PricingMap>,
    window: DateWindow,
    shared: &'a SharedArgs,
    db_path: &'a Path,
    diagnostics: &'a Diagnostics,
}

fn load_entries_from_database(
    context: &DatabaseLoadContext<'_>,
    allow_aggregate_fallback: bool,
    collector: &mut EntryCollector,
) {
    let &DatabaseLoadContext {
        shared,
        db_path,
        diagnostics,
        ..
    } = context;
    let connection = match sqlite::Connection::open_with_flags(
        db_path,
        sqlite::OpenFlags::new().with_read_only(),
    ) {
        Ok(connection) => connection,
        Err(error) => {
            debug_log(
                shared,
                format!("Failed to open OpenCode database: {}", db_path.display()),
            );
            sqlite_setup_failure(error).record(db_path, diagnostics);
            return;
        }
    };
    if let Err(error) = connection
        .prepare("SELECT name FROM sqlite_master LIMIT 1")
        .and_then(|mut statement| statement.next())
    {
        sqlite_setup_failure(error).record(db_path, diagnostics);
        return;
    }
    let probe = |table| TableSchema::probe(&connection, table, db_path, diagnostics);
    let message = probe(Table::Message);
    let session_message = probe(Table::SessionMessage);
    let session_v2 = probe(Table::SessionV2);

    if let Some(schema) = &message {
        load_message_rows(&connection, schema, context, collector);
    }
    if let Some(schema) = &session_message {
        let fork_copies = session_v2
            .as_ref()
            .map(|session_v2| fork_copy_cutoffs(&connection, session_v2, schema))
            .unwrap_or_default();
        load_session_message_rows(&connection, schema, &fork_copies, context, collector);
    }

    if allow_aggregate_fallback && !has_temporal_filter(shared) {
        // Session aggregates are cumulative and cannot be sliced safely for a
        // bounded report, so only unbounded reports may use this fallback.
        let session = session_message
            .is_some()
            .then(|| probe(Table::Session))
            .flatten();
        for schema in [session_v2.as_ref(), session.as_ref()]
            .into_iter()
            .flatten()
        {
            for entry in load_session_aggregate_entries(&connection, schema, context) {
                collector.push_aggregate(entry);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Table {
    Message,
    SessionMessage,
    Session,
    SessionV2,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::SessionMessage => "session_message",
            Self::Session => "session",
            Self::SessionV2 => "session_v2",
        }
    }
}

struct TableSchema {
    table: Table,
    columns: HashSet<String>,
    millis_scaled: bool,
}

impl TableSchema {
    fn probe(
        connection: &sqlite::Connection,
        table: Table,
        db_path: &Path,
        diagnostics: &Diagnostics,
    ) -> Option<Self> {
        if !table_exists(connection, table, db_path, diagnostics) {
            return None;
        }
        Self::read(connection, table, db_path, diagnostics)
    }

    fn read(
        connection: &sqlite::Connection,
        table: Table,
        db_path: &Path,
        diagnostics: &Diagnostics,
    ) -> Option<Self> {
        let columns = connection
            .prepare(format!("SELECT * FROM {} LIMIT 0", table.name()))
            .and_then(|statement| {
                (0..statement.column_count())
                    .map(|index| statement.column_name(index).map(str::to_string))
                    .collect::<sqlite::Result<HashSet<_>>>()
            });
        let columns = match columns {
            Ok(columns) => columns,
            Err(error) => {
                sqlite_setup_failure(error).record(db_path, diagnostics);
                return None;
            }
        };
        let millis_scaled =
            columns.contains("time_created") && time_created_looks_like_millis(connection, table);
        Some(Self {
            table,
            columns,
            millis_scaled,
        })
    }

    fn has_all(&self, columns: &[&str]) -> bool {
        columns.iter().all(|column| self.columns.contains(*column))
    }

    fn column_or(&self, column: &'static str, fallback: &'static str) -> &'static str {
        if self.columns.contains(column) {
            column
        } else {
            fallback
        }
    }

    fn pushdown(&self, context: &DatabaseLoadContext<'_>) -> DateWindow {
        if context.window.is_unbounded() {
            return DateWindow::UNBOUNDED;
        }
        if self.millis_scaled {
            return context.window.widened_for_pushdown();
        }
        debug_log(
            context.shared,
            format!(
                "OpenCode {}.time_created is missing or not millisecond-scale; scanning unfiltered: {}",
                self.table.name(),
                context.db_path.display()
            ),
        );
        DateWindow::UNBOUNDED
    }
}

struct UnreadableRow;

impl From<sqlite::Error> for UnreadableRow {
    fn from(_: sqlite::Error) -> Self {
        Self
    }
}

fn scan_rows<T>(
    mut statement: sqlite::Statement<'_>,
    context: &DatabaseLoadContext<'_>,
    table: Table,
    unreadable: &'static str,
    read: impl Fn(&sqlite::Statement<'_>) -> std::result::Result<Option<T>, UnreadableRow>,
    mut visit: impl FnMut(u32, T),
) -> u32 {
    let mut row = 0_u32;
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                row = row.saturating_add(1);
                match read(&statement) {
                    Ok(Some(value)) => visit(row, value),
                    Ok(None) => {}
                    Err(UnreadableRow) => context.diagnostics.record(Skip::record(
                        context.db_path,
                        row,
                        SkipKind::InvalidData,
                        unreadable,
                    )),
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(error) => {
                UnitError::sqlite(SqliteStage::Rows, error.code)
                    .record(context.db_path, context.diagnostics);
                debug_log(
                    context.shared,
                    format!(
                        "Failed to query OpenCode {} table: {}",
                        table.name(),
                        context.db_path.display()
                    ),
                );
                break;
            }
        }
    }
    row
}

const COLUMN_READ_FAILED: &str = "database column read failed";

struct MessageRecord {
    id: String,
    session_id: String,
    data: String,
}

fn load_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    let statement = match prepare_message_query(connection, schema.pushdown(context)) {
        Ok(statement) => statement,
        Err(error) => {
            error.record(context.db_path, context.diagnostics);
            debug_log(
                context.shared,
                format!(
                    "Failed to read OpenCode database: {}",
                    context.db_path.display()
                ),
            );
            return;
        }
    };
    let window = context.window;
    scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        |statement| {
            Ok(Some(MessageRecord {
                id: statement.read(0)?,
                session_id: statement.read(1)?,
                data: statement.read(2)?,
            }))
        },
        |row,
         MessageRecord {
             id,
             session_id,
             data,
         }| {
            if !window.is_unbounded()
                && let Some(millis) = extract_message_timestamp(&data)
                && !window.contains(millis)
            {
                return;
            }
            let Some(value) = jsonl::decode_record::<OpenCodeMessage>(
                data.as_bytes(),
                context.db_path,
                row,
                context.diagnostics,
                Some(|_| true),
            ) else {
                return;
            };
            match message_value_to_entry(
                value,
                Some(id),
                Some(session_id),
                context.tz,
                context.mode,
                context.pricing,
            ) {
                Ok(Some(entry)) => collector.push_message((entry, None)),
                Ok(None) => {}
                Err(MissingTimestamp) => record_missing_timestamp(context, row),
            }
        },
    );
}

fn record_missing_timestamp(context: &DatabaseLoadContext<'_>, row: u32) {
    context.diagnostics.record(Skip::record(
        context.db_path,
        row,
        SkipKind::InvalidTimestamp,
        MissingTimestamp::REASON,
    ));
}

struct SessionMessageRecord {
    id: String,
    session_id: String,
    kind: SessionMessageKind,
    data: String,
    created: Option<i64>,
    seq: Option<i64>,
}

impl SessionMessageRecord {
    fn read(statement: &sqlite::Statement<'_>) -> std::result::Result<Option<Self>, UnreadableRow> {
        let id = statement.read(0)?;
        let session_id = statement.read(1)?;
        let row_type: String = statement.read(2)?;
        let Some(kind) = SessionMessageKind::from_row_type(&row_type) else {
            return Ok(None);
        };
        Ok(Some(Self {
            id,
            session_id,
            kind,
            data: statement.read(3)?,
            created: read_i64(statement, 4),
            seq: statement.read::<Option<i64>, _>(5).ok().flatten(),
        }))
    }
}

fn load_session_message_rows(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    fork_copies: &HashMap<String, i64>,
    context: &DatabaseLoadContext<'_>,
    collector: &mut EntryCollector,
) {
    let statement = match prepare_session_message_query(connection, schema, context) {
        Ok(statement) => statement,
        Err(error) => {
            error.record(context.db_path, context.diagnostics);
            return;
        }
    };
    let window = context.window;
    let mut entries = 0;
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        SessionMessageRecord::read,
        |row, record| {
            // Copies neither count nor mark the fork as covered:
            // the fork's `session_v2` counters start at zero and
            // only hold fork-local usage, so they stay a valid
            // fallback when every fork-local row is gone.
            if fork_copies
                .get(&record.session_id)
                .is_some_and(|cutoff| record.seq.is_some_and(|seq| seq <= *cutoff))
            {
                return;
            }
            if !window.is_unbounded()
                && let Some(millis) = extract_message_timestamp(&record.data)
                && !window.contains(millis)
            {
                return;
            }
            let SessionMessageRecord {
                id,
                session_id,
                kind,
                data,
                created,
                ..
            } = record;
            let linked = match session_message_value_to_entry_checked(
                SessionMessageRow {
                    kind,
                    data: &data,
                    id,
                    session_id,
                    created,
                },
                context.tz,
                context.mode,
                context.pricing,
            ) {
                Ok(Some(linked)) => linked,
                Ok(None) => return,
                Err(SessionMessageError::MissingTimestamp) => {
                    record_missing_timestamp(context, row);
                    return;
                }
                Err(SessionMessageError::Decode(error)) => {
                    jsonl::record_decode_error(
                        data.as_bytes(),
                        error,
                        context.db_path,
                        row,
                        false,
                        context.diagnostics,
                        Some(|_: &serde_json::Value| true),
                    );
                    return;
                }
            };
            entries += 1;
            if kind == SessionMessageKind::Compaction {
                collector.push_compaction(linked);
            } else {
                collector.push_message(linked);
            }
        },
    );
    if rows > 0 && entries == 0 {
        debug_log(
            context.shared,
            format!(
                "OpenCode v2 rows produced no usage entries: {rows} rows in {}",
                context.db_path.display()
            ),
        );
    }
}

fn read_message_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    window: DateWindow,
) -> std::result::Result<Option<LoadedEntry>, UnitError> {
    let content = fs::read(path)?;
    // Skip out-of-range entries before the full parse. Extraction works on the
    // raw text and fails open (non-UTF-8 or missing timestamp -> full parse).
    if !window.is_unbounded()
        && let Ok(text) = std::str::from_utf8(&content)
        && let Some(millis) = extract_message_timestamp(text)
        && !window.contains(millis)
    {
        return Ok(None);
    }
    let value = decode_json::<OpenCodeMessage>(&content)?;
    message_value_to_entry(value, None, None, tz, mode, pricing).map_err(|MissingTimestamp| {
        UnitError::new(SkipKind::InvalidTimestamp, MissingTimestamp::REASON)
    })
}

fn table_exists(
    connection: &sqlite::Connection,
    table: Table,
    db_path: &Path,
    diagnostics: &Diagnostics,
) -> bool {
    let exists = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
        .and_then(|mut statement| {
            statement.bind((1, table.name()))?;
            statement.next()
        });
    match exists {
        Ok(sqlite::State::Row) => true,
        Ok(sqlite::State::Done) => false,
        Err(error) => {
            sqlite_setup_failure(error).record(db_path, diagnostics);
            false
        }
    }
}

/// Fork sessions whose leading `seq` range is a copy of the parent history.
///
/// Returns the largest `seq` that is still copied history for each fork
/// session, so the v2 row scan can drop inherited usage while
/// keeping what the fork generated itself. `session_v2` carries the fork
/// link (`fork_session_id`) plus the boundary message; `session_message`
/// carries the `seq` the fork kept when it copied the parent's rows.
fn fork_copy_cutoffs(
    connection: &sqlite::Connection,
    session_v2: &TableSchema,
    session_message: &TableSchema,
) -> HashMap<String, i64> {
    let mut cutoffs = HashMap::new();
    if !session_v2.has_all(&["id", "fork_session_id", "fork_boundary"])
        || !session_message.has_all(&["id", "session_id", "seq"])
    {
        return cutoffs;
    }
    let Ok(mut forks) = connection.prepare(
        "SELECT id, fork_session_id, fork_boundary FROM session_v2 \
         WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL",
    ) else {
        return cutoffs;
    };
    while matches!(forks.next(), Ok(sqlite::State::Row)) {
        let Ok(fork_id) = forks.read::<String, _>(0) else {
            continue;
        };
        let Ok(parent_id) = forks.read::<String, _>(1) else {
            continue;
        };
        let Ok(boundary) = forks.read::<String, _>(2) else {
            continue;
        };
        if fork_id.is_empty() || parent_id.is_empty() {
            continue;
        }
        if let Some(cutoff) = fork_copy_cutoff(connection, &parent_id, &boundary) {
            cutoffs.insert(fork_id, cutoff);
        }
    }
    cutoffs
}

/// Largest copied `seq` for one fork: the parent's boundary `seq` for a
/// `through` fork, or the parent's last `seq` below the boundary for a
/// `before` fork.
///
/// Returns `None` when the boundary cannot be resolved — a missing parent
/// row, a missing boundary row, or an unparsable boundary payload — so the
/// fork keeps all of its rows instead of being dropped on a guess.
fn fork_copy_cutoff(
    connection: &sqlite::Connection,
    parent_id: &str,
    boundary: &str,
) -> Option<i64> {
    let boundary: serde_json::Value = serde_json::from_str(boundary).ok()?;
    let boundary_type = boundary.get("type")?.as_str()?;
    // The boundary names the parent's message id, which the fork copy does
    // not reuse (every copy gets a new id), so the lookup has to run against
    // the parent session's rows and then compare by the preserved `seq`.
    let parent_message_id = boundary.get("messageID")?.as_str()?;
    let Ok(mut statement) = connection
        .prepare("SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2 LIMIT 1")
    else {
        return None;
    };
    statement.bind((1, parent_id)).ok()?;
    statement.bind((2, parent_message_id)).ok()?;
    let boundary_seq = match statement.next() {
        Ok(sqlite::State::Row) => statement.read::<Option<i64>, _>(0).ok().flatten()?,
        _ => return None,
    };
    match boundary_type {
        "through" => Some(boundary_seq),
        // `seq` is sparse and the fork's own rows continue right after the
        // last copied row, so `boundary_seq - 1` could swallow them.
        "before" => last_parent_seq_before(connection, parent_id, boundary_seq),
        _ => None,
    }
}

fn last_parent_seq_before(
    connection: &sqlite::Connection,
    parent_id: &str,
    boundary_seq: i64,
) -> Option<i64> {
    let Ok(mut statement) = connection
        .prepare("SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2")
    else {
        return None;
    };
    statement.bind((1, parent_id)).ok()?;
    statement.bind((2, boundary_seq)).ok()?;
    match statement.next() {
        // `MAX` over no rows is NULL: nothing was copied, so nothing is skipped.
        Ok(sqlite::State::Row) => statement.read::<Option<i64>, _>(0).ok()?,
        _ => None,
    }
}

fn prepare_session_message_query<'a>(
    connection: &'a sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
) -> std::result::Result<sqlite::Statement<'a>, UnitError> {
    if !schema.has_all(&["id", "session_id", "type", "data"]) {
        return Err(UnitError::unsupported_schema());
    }
    let window = schema.pushdown(context);
    let select = format!(
        "SELECT id, session_id, type, data, {}, {} FROM session_message",
        schema.column_or("time_created", "NULL"),
        schema.column_or("seq", "NULL"),
    );
    let sql = match (window.start, window.end) {
        (Some(_), Some(_)) => format!("{select} WHERE time_created >= ?1 AND time_created < ?2"),
        (Some(_), None) => format!("{select} WHERE time_created >= ?1"),
        (None, Some(_)) => format!("{select} WHERE time_created < ?1"),
        (None, None) => select,
    };
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}

fn bind_window(
    statement: &mut sqlite::Statement<'_>,
    window: DateWindow,
) -> std::result::Result<(), UnitError> {
    for (index, bound) in [window.start, window.end].into_iter().flatten().enumerate() {
        statement
            .bind((index + 1, bound))
            .map_err(sqlite_setup_failure)?;
    }
    Ok(())
}

fn sqlite_setup_failure(error: sqlite::Error) -> UnitError {
    UnitError::sqlite(SqliteStage::Setup, error.code)
}

fn load_session_aggregate_entries(
    connection: &sqlite::Connection,
    schema: &TableSchema,
    context: &DatabaseLoadContext<'_>,
) -> Vec<LoadedEntry> {
    if !schema.has_all(&[
        "id",
        "time_created",
        "cost",
        "tokens_input",
        "tokens_output",
        "tokens_cache_read",
        "tokens_cache_write",
    ]) {
        return Vec::new();
    }
    let table = schema.table.name();
    let sql = format!(
        "SELECT id, time_created, cost, tokens_input, tokens_output, \
         tokens_cache_read, tokens_cache_write, {}, {} FROM {table}",
        schema.column_or("tokens_reasoning", "0"),
        schema.column_or("model", "NULL"),
    );
    let statement = match connection.prepare(sql) {
        Ok(statement) => statement,
        Err(error) => {
            debug_log(
                context.shared,
                format!(
                    "Failed to read OpenCode {table} table: {}",
                    context.db_path.display()
                ),
            );
            sqlite_setup_failure(error).record(context.db_path, context.diagnostics);
            return Vec::new();
        }
    };
    let mut entries = Vec::new();
    scan_rows(
        statement,
        context,
        schema.table,
        "invalid aggregate row",
        |statement| {
            let session_id = statement.read(0)?;
            let created = read_i64(statement, 1);
            let model = statement.read::<String, _>(8).ok();
            let (model, provider) = parse_session_model(model.as_deref())
                .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
            Ok(Some(OpenCodeSessionAggregate {
                session_id,
                created,
                model,
                provider,
                input_tokens: read_count(statement, 3),
                output_tokens: read_count(statement, 4),
                reasoning_tokens: read_count(statement, 7),
                cache_read_tokens: read_count(statement, 5),
                cache_write_tokens: read_count(statement, 6),
                cost: read_f64(statement, 2),
            }))
        },
        |row, aggregate| {
            if !context.window.is_unbounded()
                && aggregate
                    .created
                    .is_none_or(|created| !context.window.contains(created))
            {
                return;
            }
            match session_value_to_entry(aggregate, context.tz, context.mode, context.pricing) {
                Ok(Some(entry)) => entries.push(entry),
                Ok(None) => {}
                Err(MissingTimestamp) => record_missing_timestamp(context, row),
            }
        },
    );
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

    fn from_shared(shared: &SharedArgs, tz: &JiffTimeZone) -> Self {
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
/// The bounds are applied through a subquery that selects only `id`. OpenCode's
/// index is `(session_id, time_created, id)`, so a bare `time_created` range
/// cannot seek it and scans the table, reading every `data` blob on the way. The
/// subquery is answered from that index alone, leaving only in-range rows to be
/// fetched by primary key — the difference is what keeps a narrow window off the
/// gigabytes of payload it does not need.
fn prepare_message_query(
    connection: &sqlite::Connection,
    window: DateWindow,
) -> std::result::Result<sqlite::Statement<'_>, UnitError> {
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
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}

/// Smallest value treated as millisecond scale: any Unix timestamp after 1973
/// needs at least 12 digits, while second-scale values stay far below it.
const MIN_MILLIS_SCALE: i64 = 100_000_000_000;

fn time_created_looks_like_millis(connection: &sqlite::Connection, table: Table) -> bool {
    let Ok(mut statement) = connection.prepare(format!(
        "SELECT max(time_created) FROM (SELECT time_created FROM {} LIMIT 8)",
        table.name()
    )) else {
        return false;
    };
    match statement.next() {
        Ok(sqlite::State::Row) => {
            read_i64(&statement, 0).is_some_and(|max| max >= MIN_MILLIS_SCALE)
        }
        _ => false,
    }
}

pub(super) fn load_entries_with_native_sessions_with_progress(
    shared: &SharedArgs,
    kind: AgentReportKind,
    owner: ProgressOwner,
) -> Result<Vec<LinkedEntry>> {
    let diagnostics = Diagnostics::new("opencode");
    let entries =
        load_entries_with_native_sessions_with_diagnostics(shared, kind, owner, &diagnostics)?;
    diagnostics.emit(shared);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{LinkedEntry, load_directories};
    use crate::{
        LoadedEntry,
        cli::{AgentReportKind, CostMode, SharedArgs},
    };
    use ccusage_core::Diagnostics;
    use ccusage_test_support::fs_fixture;

    fn load_linked(
        directories: &[&Path],
        shared: &SharedArgs,
        report_kind: AgentReportKind,
    ) -> Vec<LinkedEntry> {
        let directories = directories
            .iter()
            .map(|directory| directory.to_path_buf())
            .collect::<Vec<_>>();
        load_directories(
            &directories,
            shared,
            report_kind,
            &Diagnostics::new("opencode"),
        )
    }

    fn load_entries_from_directory_for_report(
        directory: &Path,
        shared: &SharedArgs,
        report_kind: AgentReportKind,
    ) -> Vec<LoadedEntry> {
        load_linked(&[directory], shared, report_kind)
            .into_iter()
            .map(|(entry, _)| entry)
            .collect()
    }

    fn load_entries_from_directory(directory: &Path, shared: &SharedArgs) -> Vec<LoadedEntry> {
        load_entries_from_directory_for_report(directory, shared, AgentReportKind::Daily)
    }

    #[test]
    fn reports_failed_schema_probes_without_database_details() {
        use ccusage_core::{Diagnostics, SkipKind};

        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("opencode.db");
        let connection = sqlite::open(&path).unwrap();
        connection
            .execute("CREATE TABLE session_message (id TEXT, session_id TEXT, seq INTEGER)")
            .unwrap();
        let diagnostics = Diagnostics::new("opencode");
        assert!(super::table_exists(
            &connection,
            super::Table::SessionMessage,
            &path,
            &diagnostics,
        ));
        connection.execute("DROP TABLE session_message").unwrap();
        assert!(
            super::TableSchema::read(
                &connection,
                super::Table::SessionMessage,
                &path,
                &diagnostics
            )
            .is_none()
        );
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(warnings.contains("database schema is not supported"));
        assert!(!warnings.contains("no such table"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_message_directory_is_reported() {
        use std::{fs, os::unix::fs::PermissionsExt};

        use ccusage_core::{Diagnostics, SkipKind};

        let fixture = ccusage_test_support::fs_fixture!({
            "storage/message/blocked/m.json": "{}",
        });
        let blocked = fixture.path("storage/message/blocked");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::read_dir(&blocked).is_err();
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("opencode");
        load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Daily,
            &diagnostics,
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            return;
        }

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_data_directory_that_cannot_be_listed_for_databases_is_reported() {
        use std::{fs, os::unix::fs::PermissionsExt};

        use ccusage_core::{Diagnostics, SkipKind};

        let fixture = ccusage_test_support::fs_fixture!({
            "data/storage/message/s/m.json": "{}",
        });
        let data = fixture.path("data");
        fs::set_permissions(&data, fs::Permissions::from_mode(0o300)).unwrap();
        let denied = fs::read_dir(&data).is_err();
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("opencode");
        load_directories(
            std::slice::from_ref(&data),
            &shared,
            AgentReportKind::Daily,
            &diagnostics,
        );
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            return;
        }

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn specialized_diagnostics_keep_healthy_database_rows() {
        let fixture = ccusage_test_support::Fixture::new();
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "good",
            "s",
            r#"{"modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#,
        );
        create_db_message_with_time(&db, "bad", "s", 1767312000000, "{SECRET_CANARY");
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            debug: true,
            ..Default::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("opencode");
        let loaded = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Daily,
            &diagnostics,
        );
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.data.message.usage.input_tokens, 10);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }

    #[test]
    fn names_a_session_message_table_without_usage_columns_an_unsupported_schema() {
        use ccusage_core::SkipKind;

        let fixture = ccusage_test_support::Fixture::new();
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "good",
            "s",
            r#"{"modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#,
        );
        sqlite::open(&db)
            .unwrap()
            .execute("CREATE TABLE session_message (id TEXT, session_id TEXT, seq INTEGER)")
            .unwrap();
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            debug: true,
            ..Default::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("opencode");

        let loaded = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Daily,
            &diagnostics,
        );

        assert_eq!(loaded.len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
        assert!(
            diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("opencode.db: database schema is not supported")
        );
    }

    #[test]
    fn specialized_diagnostics_leave_optional_sources_and_nonusage_quiet() {
        let fixture = ccusage_test_support::Fixture::new();
        sqlite::open(fixture.path("opencode.db"))
            .unwrap()
            .execute("CREATE TABLE session_v2 (id TEXT PRIMARY KEY)")
            .unwrap();
        let _ = fixture.write_file(
            "storage/message/user.json",
            r#"{"role":"user","content":"SECRET_CANARY"}"#,
        );
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            debug: true,
            ..Default::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("opencode");
        let loaded = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Session,
            &diagnostics,
        );
        assert!(loaded.is_empty());
        assert!(diagnostics.warnings(&shared).is_empty());
    }

    #[test]
    fn reports_aggregate_schema_read_failures() {
        use super::{
            DatabaseLoadContext, DateWindow, Table, TableSchema, load_session_aggregate_entries,
        };
        use ccusage_core::{Diagnostics, SkipKind};

        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("opencode.db");
        create_db_session_aggregate_table(&path, "session_v2");
        let connection = sqlite::open(&path).unwrap();
        let diagnostics = Diagnostics::new("opencode");
        let schema =
            TableSchema::probe(&connection, Table::SessionV2, &path, &diagnostics).unwrap();
        connection.execute("DROP TABLE session_v2").unwrap();
        let shared = SharedArgs::default();
        let context = DatabaseLoadContext {
            tz: &jiff::tz::TimeZone::system(),
            mode: CostMode::Display,
            pricing: None,
            window: DateWindow::UNBOUNDED,
            shared: &shared,
            db_path: &path,
            diagnostics: &diagnostics,
        };

        assert!(load_session_aggregate_entries(&connection, &schema, &context).is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    }

    #[test]
    fn skips_usage_without_a_creation_time_in_every_format() {
        const UNTIMED: &str = r#"{"modelID":"gpt-test","providerID":"openai","model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":10,"output":1},"cost":0.01}"#;
        let fixture = fs_fixture!({
            "storage/message/file-session/file-untimed.json": r#"{"id":"file-untimed","sessionID":"file-session","modelID":"gpt-test","providerID":"openai","time":{"created":0},"tokens":{"input":10,"output":1},"cost":0.01}"#,
        });
        let db = fixture.path("opencode.db");
        create_db_message(
            &db,
            "timed",
            "timed-session",
            &assistant_payload(10, 1, 0.01).replace(
                r#""model":{"id":"gpt-test","providerID":"openai"},"#,
                r#""modelID":"gpt-test","providerID":"openai","#,
            ),
        );
        create_db_message_with_time(&db, "db-untimed", "db-session", 0, UNTIMED);
        create_db_session_message_table(&db, false);
        for id in ["v2-user-1", "v2-user-2"] {
            insert_db_session_message(&db, id, "v2-session", "user", 0, "{}", false);
        }
        insert_db_session_message(
            &db,
            "v2-untimed",
            "v2-session",
            "assistant",
            0,
            UNTIMED,
            false,
        );
        create_db_session_aggregate_table(&db, "session_v2");
        insert_db_session_aggregate(
            &db,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "aggregate-session",
                time_created: 0,
                model: r#"{"id":"gpt-test","providerID":"openai"}"#,
                cost: 0.5,
                input_tokens: 200,
                output_tokens: 100,
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
        let diagnostics = Diagnostics::new("opencode");

        let entries = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Session,
            &diagnostics,
        );

        let ids = entries
            .iter()
            .map(|(entry, _)| entry.data.message.id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(ids, [Some("timed")]);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            4
        );
    }

    #[test]
    fn specialized_diagnostics_distinguish_v2_usage_from_suppressed_rows() {
        let fixture = ccusage_test_support::Fixture::new();
        let db = fixture.path("opencode.db");
        create_db_session_message_table(&db, true);
        for (id, kind, data) in [
            (
                "good",
                "assistant",
                r#"{"modelID":"claude-opus-5","providerID":"anthropic","tokens":{"input":10,"output":1}}"#,
            ),
            ("bad", "assistant", "{SECRET_CANARY"),
            ("user", "user", "{SECRET_CANARY"),
            (
                "failed",
                "compaction",
                r#"{"status":"failed","tokens":{"input":10}}"#,
            ),
            ("pending", "assistant", r#"{"tokens":null}"#),
        ] {
            insert_db_session_message(&db, id, "s", kind, 1767312000000, data, true);
        }
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            debug: true,
            ..Default::default()
        };
        let diagnostics = ccusage_core::Diagnostics::new("opencode");
        let loaded = load_directories(
            &[fixture.root().to_path_buf()],
            &shared,
            AgentReportKind::Daily,
            &diagnostics,
        );
        assert_eq!(loaded.len(), 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
        assert!(
            !diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }

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
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT)"
        } else {
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0, data TEXT)"
        };
        db.execute(schema).unwrap();
    }

    fn set_db_session_message_seq(path: &Path, id: &str, seq: i64) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare("UPDATE session_message SET seq = ?1 WHERE id = ?2")
            .unwrap();
        statement.bind((1, seq)).unwrap();
        statement.bind((2, id)).unwrap();
        statement.next().unwrap();
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
        create_db_session_aggregate_table_with_fork_columns(path, table, false);
    }

    fn create_db_session_aggregate_table_with_fork_columns(
        path: &Path,
        table: &str,
        with_fork_columns: bool,
    ) {
        let fork_columns = if with_fork_columns {
            ", fork_session_id TEXT, fork_boundary TEXT"
        } else {
            ""
        };
        let schema = match table {
            "session_v2" => {
                format!(
                    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT{fork_columns})"
                )
            }
            "session" => {
                format!(
                    "CREATE TABLE session (id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT{fork_columns})"
                )
            }
            _ => panic!("unsupported test session table: {table}"),
        };
        sqlite::open(path)
            .unwrap()
            .execute(schema.as_str())
            .unwrap();
    }

    fn insert_db_session_fork(
        path: &Path,
        table: &str,
        fork_id: &str,
        parent_id: &str,
        boundary: &str,
    ) {
        let sql = match table {
            "session_v2" => {
                "INSERT INTO session_v2 (id, fork_session_id, fork_boundary) VALUES (?1, ?2, ?3)"
            }
            "session" => {
                "INSERT INTO session (id, fork_session_id, fork_boundary) VALUES (?1, ?2, ?3)"
            }
            _ => panic!("unsupported test session table: {table}"),
        };
        let db = sqlite::open(path).unwrap();
        let mut statement = db.prepare(sql).unwrap();
        statement.bind((1, fork_id)).unwrap();
        statement.bind((2, parent_id)).unwrap();
        statement.bind((3, boundary)).unwrap();
        statement.next().unwrap();
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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        );

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
        );

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
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "legacy-session");
        assert_eq!(entries[0].data.message.usage.input_tokens, 120);
        assert_eq!(entries[0].cost, 0.03);
    }

    const COMPACTION_COMPLETED: &str = r#"{"status":"completed","reason":"auto","summary":"s","model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":100,"output":20,"reasoning":7,"cache":{"read":30,"write":5}},"cost":0.02}"#;

    fn insert_assistant_and_compaction(db_path: &Path, session_id: &str, prefix: &str) {
        insert_db_session_message(
            db_path,
            &format!("{prefix}-assistant"),
            session_id,
            "assistant",
            1_767_312_000_000,
            assistant_payload(10, 2, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(db_path, &format!("{prefix}-assistant"), 0);
        insert_db_session_message(
            db_path,
            &format!("{prefix}-compaction"),
            session_id,
            "compaction",
            1_767_312_000_001,
            COMPACTION_COMPLETED,
            true,
        );
        set_db_session_message_seq(db_path, &format!("{prefix}-compaction"), 1);
    }

    fn total_tokens(entries: &[crate::LoadedEntry]) -> u64 {
        entries
            .iter()
            .map(|entry| {
                let usage = &entry.data.message.usage;
                usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_creation_input_tokens
                    + usage.cache_read_input_tokens
                    + entry.extra_total_tokens
            })
            .sum()
    }

    #[test]
    fn a_message_wins_over_a_compaction_with_its_id_in_an_earlier_directory() {
        let fixture = fs_fixture!({});
        let first = fixture.path("first/opencode.db");
        let second = fixture.path("second/opencode.db");
        std::fs::create_dir_all(fixture.path("first")).unwrap();
        std::fs::create_dir_all(fixture.path("second")).unwrap();
        create_db_session_message_table(&first, true);
        insert_db_session_message(
            &first,
            "shared-id",
            "v2-session",
            "compaction",
            1_767_312_000_000,
            COMPACTION_COMPLETED,
            true,
        );
        create_db_session_message_table(&second, true);
        insert_db_session_message(
            &second,
            "shared-id",
            "v2-session",
            "assistant",
            1_767_312_000_000,
            assistant_payload(10, 2, 0.01).as_str(),
            true,
        );

        let entries = load_linked(
            &[&fixture.path("first"), &fixture.path("second")],
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Daily,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.data.message.usage.input_tokens, 10);
    }

    #[test]
    fn counts_completed_v2_compaction_usage_alongside_assistant_usage() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_assistant_and_compaction(&db_path, "v2-session", "msg");
        // A running compaction carries no usage and a failed one has no model,
        // so neither can be attributed.
        insert_db_session_message(
            &db_path,
            "msg-compaction-running",
            "v2-session",
            "compaction",
            1_767_312_000_002,
            r#"{"status":"running","reason":"auto","summary":"","time":{"created":1767312000000}}"#,
            true,
        );
        insert_db_session_message(
            &db_path,
            "msg-compaction-failed",
            "v2-session",
            "compaction",
            1_767_312_000_003,
            r#"{"status":"failed","reason":"auto","error":{"type":"unknown","message":"x"},"time":{"created":1767312000000},"tokens":{"input":50,"output":0},"cost":0.5}"#,
            true,
        );
        // Only the status keeps these out: each has a model and usage.
        for (id, status) in [
            ("msg-compaction-failed-with-model", "failed"),
            ("msg-compaction-running-with-usage", "running"),
            ("msg-compaction-without-status", ""),
        ] {
            let status = if status.is_empty() {
                String::new()
            } else {
                format!(r#""status":"{status}","#)
            };
            insert_db_session_message(
                &db_path,
                id,
                "v2-session",
                "compaction",
                1_767_312_000_004,
                format!(
                    r#"{{{status}"reason":"auto","model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":1767312000000}},"tokens":{{"input":40,"output":4}},"cost":0.4}}"#
                )
                .as_str(),
                true,
            );
        }

        let entries = load_entries_from_directory(
            fixture.root(),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
        );

        let mut ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.data.message.id.as_deref())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["msg-assistant", "msg-compaction"]);
        assert_eq!(total_tokens(&entries), 174);
        let cost: f64 = entries.iter().map(|entry| entry.cost).sum();
        assert!((cost - 0.03).abs() < 1e-9);
    }

    #[test]
    fn prefers_session_aggregate_over_a_lone_v2_compaction_row() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        // The assistant rows are gone (for example after a revert), but the
        // session counters still hold them alongside the compaction usage.
        insert_db_session_message(
            &db_path,
            "msg-compaction",
            "v2-session",
            "compaction",
            1_767_312_000_001,
            COMPACTION_COMPLETED,
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
                cost: 0.03,
                input_tokens: 110,
                output_tokens: 22,
                cache_read_tokens: 30,
                cache_write_tokens: 5,
                reasoning_tokens: 7,
            },
        );
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let session_entries = load_entries_from_directory_for_report(
            fixture.root(),
            &shared,
            AgentReportKind::Session,
        );
        assert_eq!(session_entries.len(), 1);
        assert_ne!(
            session_entries[0].data.message.id.as_deref(),
            Some("msg-compaction")
        );
        assert_eq!(total_tokens(&session_entries), 174);
        assert!((session_entries[0].cost - 0.03).abs() < 1e-9);

        // Period reports never use cumulative session counters, so the
        // compaction row is the only usage they can attribute.
        let daily_entries = load_entries_from_directory(fixture.root(), &shared);
        assert_eq!(daily_entries.len(), 1);
        assert_eq!(
            daily_entries[0].data.message.id.as_deref(),
            Some("msg-compaction")
        );
        assert_eq!(total_tokens(&daily_entries), 162);
    }

    #[test]
    fn excludes_copied_v2_compaction_rows_from_forks() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_assistant_and_compaction(&db_path, "parent", "parent");
        insert_assistant_and_compaction(&db_path, "fork", "fork-copy");
        insert_db_session_message(
            &db_path,
            "fork-own",
            "fork",
            "assistant",
            1_767_312_000_002,
            assistant_payload(3, 1, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(&db_path, "fork-own", 2);
        create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
        insert_db_session_fork(
            &db_path,
            "session_v2",
            "fork",
            "parent",
            r#"{"type":"through","messageID":"parent-compaction"}"#,
        );

        let entries = fork_entries(&db_path);

        assert_eq!(total_tokens(&entries), 178);
        let fork_ids: Vec<_> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .filter_map(|entry| entry.data.message.id.as_deref())
            .collect();
        assert_eq!(fork_ids, vec!["fork-own"]);
    }

    fn assistant_payload(input: u64, output: u64, cost: f64) -> String {
        format!(
            r#"{{"model":{{"id":"gpt-test","providerID":"openai"}},"time":{{"created":1767312000000}},"tokens":{{"input":{input},"output":{output}}},"cost":{cost}}}"#,
        )
    }

    fn insert_fork_history(
        db_path: &Path,
        parent_id: &str,
        fork_id: &str,
        boundary_type: &str,
        boundary_message_id: &str,
    ) {
        // Parent history: two assistant turns, then the boundary assistant
        // turn, then a later assistant turn the fork never copies.
        for (id, seq, input) in [
            ("parent-msg-1", 0, 100),
            ("parent-msg-2", 1, 200),
            (boundary_message_id, 2, 300),
            ("parent-msg-4", 3, 400),
        ] {
            insert_db_session_message(
                db_path,
                id,
                parent_id,
                "assistant",
                1_767_312_000_000 + seq,
                assistant_payload(input, 10, 0.01).as_str(),
                true,
            );
            set_db_session_message_seq(db_path, id, seq);
        }
        // Fork copies: same `seq`, new ids, and the fork's own turn at seq 4.
        // The boundary copy keeps usage so `through` vs `before` is observable.
        for (copy_id, seq, input) in [
            ("fork-copy-1", 0, 100),
            ("fork-copy-2", 1, 200),
            ("fork-copy-boundary", 2, 300),
        ] {
            insert_db_session_message(
                db_path,
                copy_id,
                fork_id,
                "assistant",
                1_767_312_000_000 + seq,
                assistant_payload(input, 10, 0.01).as_str(),
                true,
            );
            set_db_session_message_seq(db_path, copy_id, seq);
        }
        insert_db_session_message(
            db_path,
            "fork-own",
            fork_id,
            "assistant",
            1_767_312_000_004,
            assistant_payload(40, 4, 0.02).as_str(),
            true,
        );
        set_db_session_message_seq(db_path, "fork-own", 4);
        create_db_session_aggregate_table_with_fork_columns(db_path, "session_v2", true);
        insert_db_session_fork(
            db_path,
            "session_v2",
            fork_id,
            parent_id,
            format!(r#"{{"type":"{boundary_type}","messageID":"{boundary_message_id}"}}"#,)
                .as_str(),
        );
    }

    fn fork_entries(db_path: &Path) -> Vec<crate::LoadedEntry> {
        load_entries_from_directory(
            db_path.parent().expect("fixture db has a parent directory"),
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
        )
    }

    #[test]
    fn excludes_copied_history_through_the_fork_boundary() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");

        let entries = fork_entries(&db_path);

        // Parent keeps its four turns; the fork keeps only its own turn.
        // `through` drops the copied boundary turn (seq 2) with the rest.
        assert_eq!(entries.len(), 5);
        let parent_input: u64 = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "parent")
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum();
        assert_eq!(parent_input, 1000);
        let fork_entries: Vec<_> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .collect();
        assert_eq!(fork_entries.len(), 1);
        assert_eq!(fork_entries[0].data.message.id.as_deref(), Some("fork-own"));
        assert_eq!(fork_entries[0].data.message.usage.input_tokens, 40);
    }

    #[test]
    fn excludes_copied_history_before_the_fork_boundary() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "before", "boundary-msg");

        let entries = fork_entries(&db_path);

        // `before` keeps the copied boundary turn (seq 2) alongside the
        // fork's own turn; everything at or below seq 1 is still dropped.
        let mut fork_ids: Vec<_> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .filter_map(|entry| entry.data.message.id.as_deref())
            .collect();
        fork_ids.sort_unstable();
        assert_eq!(fork_ids, vec!["fork-copy-boundary", "fork-own"]);
    }

    #[test]
    fn keeps_fork_rows_when_the_boundary_cannot_be_resolved() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
        // Point the fork at a boundary message the parent never had.
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "UPDATE session_v2 SET fork_boundary = '{\"type\":\"through\",\"messageID\":\"missing\"}' \
             WHERE id = 'fork'",
        )
        .unwrap();

        let entries = fork_entries(&db_path);

        assert_eq!(entries.len(), 8);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "fork")
                .count(),
            4
        );
    }

    #[test]
    fn keeps_fork_rows_without_fork_metadata() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
        // Drop the fork link: no `session_v2` row means no exclusion.
        let db = sqlite::open(&db_path).unwrap();
        db.execute("DROP TABLE session_v2").unwrap();

        let entries = fork_entries(&db_path);

        assert_eq!(entries.len(), 8);
    }

    #[test]
    fn keeps_fork_rows_when_session_messages_have_no_seq_column() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER, data TEXT)",
            )
            .unwrap();
        for (id, session_id, input) in [
            ("parent-msg", "parent", 100),
            ("fork-copy", "fork", 100),
            ("fork-own", "fork", 40),
        ] {
            insert_db_session_message(
                &db_path,
                id,
                session_id,
                "assistant",
                1_767_312_000_000,
                assistant_payload(input, 10, 0.01).as_str(),
                true,
            );
        }
        create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
        insert_db_session_fork(
            &db_path,
            "session_v2",
            "fork",
            "parent",
            r#"{"type":"through","messageID":"parent-msg"}"#,
        );

        let entries = fork_entries(&db_path);

        // Pre-`seq` schemas cannot tell copies apart, so every row stays.
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn excludes_copied_history_from_a_fork_of_a_fork() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
        // The grandchild copies the fork's rows (including the fork's own
        // turn) and names the fork's own message as its boundary.
        for (copy_id, seq) in [
            ("grand-copy-1", 0),
            ("grand-copy-2", 1),
            ("grand-copy-boundary", 2),
            ("grand-copy-fork-own", 4),
        ] {
            insert_db_session_message(
                &db_path,
                copy_id,
                "grandchild",
                "assistant",
                1_767_312_000_000 + seq,
                assistant_payload(100, 10, 0.01).as_str(),
                true,
            );
            set_db_session_message_seq(&db_path, copy_id, seq);
        }
        insert_db_session_message(
            &db_path,
            "grand-own",
            "grandchild",
            "assistant",
            1_767_312_000_005,
            assistant_payload(7, 1, 0.01).as_str(),
            true,
        );
        set_db_session_message_seq(&db_path, "grand-own", 5);
        insert_db_session_fork(
            &db_path,
            "session_v2",
            "grandchild",
            "fork",
            r#"{"type":"through","messageID":"fork-own"}"#,
        );

        let entries = fork_entries(&db_path);

        let grandchild_ids: Vec<_> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "grandchild")
            .filter_map(|entry| entry.data.message.id.as_deref())
            .collect();
        assert_eq!(grandchild_ids, vec!["grand-own"]);
        // Parent (4) + fork's own turn (1) + grandchild's own turn (1).
        assert_eq!(entries.len(), 6);
    }

    #[test]
    fn falls_back_to_the_fork_local_aggregate_when_only_copies_remain() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        insert_fork_history(&db_path, "parent", "fork", "through", "boundary-msg");
        // The fork's own turn was reverted, leaving only copied rows.
        sqlite::open(&db_path)
            .unwrap()
            .execute("DELETE FROM session_message WHERE id = 'fork-own'")
            .unwrap();
        // OpenCode v2 starts fork counters at zero, so the aggregate holds only
        // the fork-local usage that survived the revert.
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE session_v2 SET time_created = 1767312000000, cost = 0.02, \
                 tokens_input = 40, tokens_output = 4, tokens_cache_read = 0, \
                 tokens_cache_write = 0, tokens_reasoning = 0, \
                 model = '{\"id\":\"gpt-test\",\"providerID\":\"openai\"}' WHERE id = 'fork'",
            )
            .unwrap();
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries_from_directory_for_report(
            fixture.root(),
            &shared,
            AgentReportKind::Session,
        );

        let fork_input: Vec<u64> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .map(|entry| entry.data.message.usage.input_tokens)
            .collect();
        assert_eq!(fork_input, vec![40]);
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn keeps_fork_rows_after_the_last_copy_when_before_seqs_are_sparse() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("opencode.db");
        create_db_session_message_table(&db_path, true);
        create_db_session_aggregate_table_with_fork_columns(&db_path, "session_v2", true);
        // Parent seqs have gaps; the fork copies rows 0 and 10, and its own row
        // continues right after the last copy instead of below the boundary.
        for (session_id, id, seq, input) in [
            ("parent", "parent-a", 0, 100),
            ("parent", "parent-b", 10, 200),
            ("parent", "parent-boundary", 100, 300),
            ("fork", "fork-copy-a", 0, 100),
            ("fork", "fork-copy-b", 10, 200),
            ("fork", "fork-own", 20, 40),
        ] {
            insert_db_session_message(
                &db_path,
                id,
                session_id,
                "assistant",
                1_767_312_000_000 + seq,
                assistant_payload(input, 10, 0.01).as_str(),
                true,
            );
            set_db_session_message_seq(&db_path, id, seq);
        }
        insert_db_session_fork(
            &db_path,
            "session_v2",
            "fork",
            "parent",
            r#"{"type":"before","messageID":"parent-boundary"}"#,
        );

        let entries = fork_entries(&db_path);

        let fork_ids: Vec<_> = entries
            .iter()
            .filter(|entry| entry.session_id.as_ref() == "fork")
            .filter_map(|entry| entry.data.message.id.as_deref())
            .collect();
        assert_eq!(fork_ids, vec!["fork-own"]);
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
        );

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
        );

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
        );

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
        );

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
        );

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
        let entries = load_linked(
            &[aggregate_fixture.root(), message_fixture.root()],
            &SharedArgs {
                mode: CostMode::Display,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
            AgentReportKind::Session,
        );
        let entries = entries.iter().map(|(entry, _)| entry).collect::<Vec<_>>();

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
            let entries =
                load_entries_from_directory_for_report(fixture.root(), &shared, report_kind);
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].data.message.id.as_deref(),
                Some("period-message")
            );
        }

        let session_entries = load_entries_from_directory_for_report(
            fixture.root(),
            &shared,
            AgentReportKind::Session,
        );
        assert_eq!(session_entries.len(), 2);
        assert!(session_entries.iter().any(|entry| {
            entry.data.message.id.as_deref() == Some("session:period-aggregate-session")
        }));
    }

    #[test]
    fn pairs_claude_agent_rows_with_their_claude_session_across_data_dirs() {
        let live = fs_fixture!({});
        let live_db = live.path("opencode.db");
        create_db_session_message_table(&live_db, true);
        for (id, session_id, data) in [
            (
                "agent",
                "agent-session",
                r#"{"time":{"created":1767312000000},"model":{"id":"opus","providerID":"claude-agent"},"content":[{"type":"tool","native":{"sessionID":"claude-a","messageID":"m"}}],"tokens":{"input":100},"cost":0}"#,
            ),
            (
                "plain",
                "plain-session",
                r#"{"time":{"created":1767312000000},"model":{"id":"gpt-test","providerID":"openai"},"content":[{"type":"text","text":"\"native\": not a part"}],"tokens":{"input":5},"cost":0}"#,
            ),
        ] {
            insert_db_session_message(
                &live_db,
                id,
                session_id,
                "assistant",
                1_767_312_000_000,
                data,
                true,
            );
        }
        let archive = fs_fixture!({});
        let archive_db = archive.path("opencode.db");
        create_db_session_aggregate_table(&archive_db, "session_v2");
        insert_db_session_aggregate(
            &archive_db,
            "session_v2",
            &SessionAggregateFixture {
                session_id: "agent-session",
                time_created: 1_767_312_000_000,
                model: r#"{"id":"opus","providerID":"claude-agent"}"#,
                cost: 0.0,
                input_tokens: 9_999,
                output_tokens: 0,
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

        let entries = load_linked(
            &[live.root(), archive.root()],
            &shared,
            AgentReportKind::Session,
        );
        let linked = entries
            .iter()
            .map(|(entry, native_session)| {
                (
                    entry.data.message.id.as_deref().unwrap(),
                    native_session.as_deref(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(linked, [("agent", Some("claude-a")), ("plain", None)]);
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
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "current-session");
        assert_eq!(entries[0].data.message.usage.input_tokens, 200);
        assert_eq!(entries[0].cost, 0.5);
    }

    #[test]
    fn loads_channel_sqlite_database() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({});
        create_db_message(
            &fixture.path("opencode-beta.db"),
            "channel-msg-1",
            "channel-session-a",
            r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":80,"output":40}}"#,
        );

        let entries = load_entries_from_directory(fixture.root(), &SharedArgs::default());

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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

        let single_entries = load_entries_from_directory(fixture.root(), &single);
        let multi_entries = load_entries_from_directory(fixture.root(), &multi);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);
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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        let entries = load_entries_from_directory(fixture.root(), &shared);

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
        assert!(load_entries_from_directory(db_only.root(), &shared).is_empty());
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
