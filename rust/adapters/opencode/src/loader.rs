use ccusage_adapter_common::{
    SqliteStage, any_file_matching, dir_entries, jsonl, read_units_flat,
    sqlite::{read_count, read_f64, read_i64},
    units::{UnitError, read_filtered_json},
};
use ccusage_core::cost::PriceTable;
use ccusage_core::{Diagnostics, Skip, SkipKind};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use ccusage_adapter_common::source::{ProgressOwner, UsageLoadContext};
use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{
        MissingTimestamp, OpenCodeMessage, OpenCodeSessionAggregate, SessionMessageError,
        SessionMessageKind, SessionMessageRow, message_time_created, message_value_to_entry,
        session_message_value_to_entry_checked, session_value_to_entry,
    },
    paths::paths,
};
use crate::{
    LoadedEntry, Result,
    cli::{AgentReportKind, CostMode, SharedArgs},
    collect_files_with_extension, date_range_bounds_ms, debug_log,
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
    owner.run(
        crate::progress::UsageLoadAgent("OpenCode"),
        shared.json,
        load,
    )
}

fn load_directories(
    directories: &[PathBuf],
    shared: &SharedArgs,
    report_kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Vec<LinkedEntry> {
    let allow_aggregate_fallback = report_kind == AgentReportKind::Session;
    let context = UsageLoadContext::new(shared);
    let tz = context.window().timezone();
    let window = DateWindow::from_shared(shared, tz);
    let mut collector = EntryCollector::default();
    for directory in directories {
        if let Some(db_path) = db_path(directory, diagnostics) {
            load_entries_from_database(
                &DatabaseLoadContext {
                    db_path: &db_path,
                    tz,
                    mode: shared.mode,
                    pricing: context.table(),
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

        // NOTE(2026-06-15, #1332): Legacy OpenCode stores messages as storage/message/<sessionID>/<messageID>.json. A claimed file stem lets the loader skip reading the duplicate.
        files.retain(|file| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !collector.has_claimed(stem))
        });

        let loaded = read_units_flat(&files, shared.single_thread, diagnostics, |file| {
            read_message_file(file, tz, shared.mode, context.table(), window)
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

    // NOTE(2026-11-09, #2050): OpenCode session counters include compaction usage and retain usage after a revert deletes message rows. Use the aggregate to recover missing assistant usage without adding the compaction again.
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

pub(crate) fn has_source(opencode_dir: &Path) -> bool {
    if db_path(opencode_dir, &Diagnostics::new("opencode")).is_some() {
        return true;
    }
    any_file_matching(
        &opencode_dir.join("storage").join("message"),
        &|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        },
        &Diagnostics::new("opencode"),
    )
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
    pricing: PriceTable<'a>,
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
    let connection = match ccusage_adapter_common::sqlite::open_read_only(db_path) {
        Ok(connection) => connection,
        Err(error) => {
            debug_log(
                shared,
                format!("Failed to open OpenCode database: {}", db_path.display()),
            );
            error.record(db_path, diagnostics);
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

    // NOTE(2026-11-19, #2244): OpenCode's own writers copy each message's time.created into time_created, and only its migrations create these indexes. Any other table gives the column no such guarantee, so the payload alone dates its rows.
    fn opencode_time_index(self) -> Option<&'static str> {
        match self {
            Self::Message => Some("message_session_time_created_id_idx"),
            Self::SessionMessage => Some("session_message_time_created_idx"),
            Self::Session | Self::SessionV2 => None,
        }
    }
}

struct TableSchema {
    table: Table,
    columns: HashSet<String>,
    indexed_time: bool,
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
        let indexed_time = columns.contains("time_created")
            && table
                .opencode_time_index()
                .is_some_and(|index| index_exists(connection, table, index))
            && time_created_looks_like_millis(connection, table);
        Some(Self {
            table,
            columns,
            indexed_time,
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
        if self.indexed_time {
            return context.window;
        }
        debug_log(
            context.shared,
            format!(
                "OpenCode {}.time_created is not OpenCode's indexed millisecond time; scanning unfiltered: {}",
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

fn row_diagnostics(statement: &sqlite::Statement<'_>, diagnostics: &Diagnostics) -> Diagnostics {
    diagnostics.for_row(statement.read::<String, _>(0).ok().as_deref())
}

fn scan_rows<T>(
    mut statement: sqlite::Statement<'_>,
    context: &DatabaseLoadContext<'_>,
    table: Table,
    unreadable: &'static str,
    read: impl Fn(&sqlite::Statement<'_>) -> std::result::Result<Option<T>, UnreadableRow>,
    mut visit: impl FnMut(u32, T, &sqlite::Statement<'_>),
) -> u32 {
    let unit = context.db_path;
    let diagnostics = context.diagnostics.for_table(table.name());
    let mut row = 0_u32;
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                row = row.saturating_add(1);
                match read(&statement) {
                    Ok(Some(value)) => visit(row, value, &statement),
                    Ok(None) => {}
                    Err(UnreadableRow) => row_diagnostics(&statement, &diagnostics)
                        .record(Skip::record(unit, row, SkipKind::InvalidData, unreadable)),
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
    let diagnostics = context.diagnostics.for_table(schema.table.name());
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
         },
         statement| {
            if !window.admits_payload(data.as_bytes()) {
                return;
            }
            let Some(value) = jsonl::decode_record::<OpenCodeMessage>(
                data.as_bytes(),
                context.db_path,
                row,
                Some(&id),
                &diagnostics,
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
                Err(MissingTimestamp) => record_missing_timestamp(
                    &row_diagnostics(statement, &diagnostics),
                    context.db_path,
                    row,
                ),
            }
        },
    );
}

fn record_missing_timestamp(diagnostics: &Diagnostics, unit: &Path, row: u32) {
    diagnostics.record(Skip::record(
        unit,
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
    let diagnostics = context.diagnostics.for_table(schema.table.name());
    let rows = scan_rows(
        statement,
        context,
        schema.table,
        COLUMN_READ_FAILED,
        SessionMessageRecord::read,
        |row, record, statement| {
            // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode starts fork session counters at zero, with only fork-local usage. Copied rows must leave the aggregate fallback available.
            if fork_copies
                .get(&record.session_id)
                .is_some_and(|cutoff| record.seq.is_some_and(|seq| seq <= *cutoff))
            {
                return;
            }
            if !window.admits_payload(record.data.as_bytes()) {
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
                    record_missing_timestamp(
                        &row_diagnostics(statement, &diagnostics),
                        context.db_path,
                        row,
                    );
                    return;
                }
                Err(SessionMessageError::Decode(error)) => {
                    jsonl::record_decode_error(
                        data.as_bytes(),
                        error,
                        context.db_path,
                        row,
                        jsonl::Ending::COMPLETE,
                        &row_diagnostics(statement, &diagnostics),
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
    pricing: PriceTable<'_>,
    window: DateWindow,
) -> std::result::Result<Option<LoadedEntry>, UnitError> {
    let value =
        read_filtered_json::<OpenCodeMessage>(path, |content| window.admits_payload(content))?;
    let Some(value) = value else {
        return Ok(None);
    };
    message_value_to_entry(value, None, None, tz, mode, pricing).map_err(|MissingTimestamp| {
        UnitError::new(SkipKind::InvalidTimestamp, MissingTimestamp::REASON)
    })
}

fn index_exists(connection: &sqlite::Connection, table: Table, index: &str) -> bool {
    connection
        .prepare(
            "SELECT 1 FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND name = ?2 LIMIT 1",
        )
        .and_then(|mut statement| {
            statement.bind((1, table.name()))?;
            statement.bind((2, index))?;
            statement.next()
        })
        .is_ok_and(|state| state == sqlite::State::Row)
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

fn fork_copy_cutoff(
    connection: &sqlite::Connection,
    parent_id: &str,
    boundary: &str,
) -> Option<i64> {
    let boundary: serde_json::Value = serde_json::from_str(boundary).ok()?;
    let boundary_type = boundary.get("type")?.as_str()?;
    // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode fork copies get new message IDs and preserve seq. Resolve the boundary ID in the parent session.
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
        // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode seq values can be sparse, and fork-local rows follow the last copy. Use the preceding parent seq so the cutoff keeps fork-local rows.
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
    let sql = match pushdown_condition(window) {
        Some(condition) => format!("{select} WHERE {condition}"),
        None => select,
    };
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}

fn pushdown_condition(window: DateWindow) -> Option<String> {
    let range = match (window.start, window.end) {
        (Some(_), Some(_)) => "time_created >= ?1 AND time_created < ?2",
        (Some(_), None) => "time_created >= ?1",
        (None, Some(_)) => "time_created < ?1",
        (None, None) => return None,
    };
    Some(format!(
        "(time_created IS NULL OR time_created <= 0 OR ({range}))"
    ))
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
    let diagnostics = context.diagnostics.for_table(schema.table.name());
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
        |row, aggregate, statement| match session_value_to_entry(
            aggregate,
            context.tz,
            context.mode,
            context.pricing,
        ) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(MissingTimestamp) => record_missing_timestamp(
                &row_diagnostics(statement, &diagnostics),
                context.db_path,
                row,
            ),
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
    entry.message_id.as_deref().filter(|id| !id.is_empty())
}

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
        let (start, end) = date_range_bounds_ms(shared.since, shared.until, tz);
        Self { start, end }
    }

    fn is_unbounded(self) -> bool {
        self == Self::UNBOUNDED
    }

    fn contains(self, millis: i64) -> bool {
        self.start.is_none_or(|start| millis >= start) && self.end.is_none_or(|end| millis < end)
    }

    fn admits_payload(self, data: &[u8]) -> bool {
        self.is_unbounded() || message_time_created(data).is_none_or(|millis| self.contains(millis))
    }
}

// NOTE(2026-07-27, #1492): OpenCode indexes message IDs with (session_id, time_created, id). Select IDs from the covering index before fetching payload blobs.
fn prepare_message_query(
    connection: &sqlite::Connection,
    window: DateWindow,
) -> std::result::Result<sqlite::Statement<'_>, UnitError> {
    let select = "SELECT id, session_id, data FROM message";
    let sql = match pushdown_condition(window) {
        Some(condition) => {
            format!("{select} WHERE id IN (SELECT id FROM message WHERE {condition})")
        }
        None => select.to_string(),
    };
    let mut statement = connection.prepare(sql).map_err(sqlite_setup_failure)?;
    bind_window(&mut statement, window)?;
    Ok(statement)
}

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

fn load_entries_with_native_sessions_with_progress(
    shared: &SharedArgs,
    kind: AgentReportKind,
    owner: ProgressOwner,
) -> Result<Vec<LinkedEntry>> {
    let diagnostics = Diagnostics::new("opencode");
    let entries =
        load_entries_with_native_sessions_with_diagnostics(shared, kind, owner, &diagnostics)?;
    diagnostics.emit(shared.debug);
    Ok(entries)
}

#[cfg(test)]
mod tests;
