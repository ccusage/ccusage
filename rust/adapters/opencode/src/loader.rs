use ccusage_adapter_common::{SqliteStage, any_file_matching, units::UnitError};
use ccusage_core::cost::PriceTable;
use ccusage_core::{Diagnostics, Skip, SkipKind};
use std::{collections::HashSet, path::Path, sync::Arc};

use ccusage_adapter_common::source::ProgressOwner;
use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{MissingTimestamp, message_time_created},
    paths::paths,
};
use crate::{
    LoadedEntry, Result,
    cli::{AgentReportKind, CostMode, SharedArgs},
    date_range_bounds_ms, debug_log,
};

mod files;
mod forks;
mod messages;
mod schema;
mod session_aggregate;
mod session_messages;

use files::{db_path, load_directories};
use forks::fork_copy_cutoffs;
use messages::load_message_rows;
use schema::{Table, TableSchema};
use session_aggregate::load_session_aggregate_entries;
use session_messages::load_session_message_rows;

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
                record_rows_failure(sqlite_rows_failure(error), context, table);
                break;
            }
        }
    }
    row
}

fn sqlite_rows_failure(error: sqlite::Error) -> UnitError {
    UnitError::sqlite(SqliteStage::Rows, error.code)
}

fn record_rows_failure(error: UnitError, context: &DatabaseLoadContext<'_>, table: Table) {
    error.record(context.db_path, context.diagnostics);
    debug_log(
        context.shared,
        format!(
            "Failed to query OpenCode {} table: {}",
            table.name(),
            context.db_path.display()
        ),
    );
}

const COLUMN_READ_FAILED: &str = "database column read failed";

fn record_missing_timestamp(diagnostics: &Diagnostics, unit: &Path, row: u32) {
    diagnostics.record(Skip::record(
        unit,
        row,
        SkipKind::InvalidTimestamp,
        MissingTimestamp::REASON,
    ));
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
