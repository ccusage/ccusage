use ccusage_adapter_common::sqlite::{read_i64, table_exists as shared_table_exists};
use ccusage_core::Diagnostics;
use std::{collections::HashSet, path::Path};

use super::{DatabaseLoadContext, DateWindow, sqlite_setup_failure};
use crate::debug_log;

#[derive(Clone, Copy)]
pub(super) enum Table {
    Message,
    SessionMessage,
    Session,
    SessionV2,
}

impl Table {
    pub(super) fn name(self) -> &'static str {
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

pub(super) struct TableSchema {
    pub(super) table: Table,
    columns: HashSet<String>,
    pub(super) indexed_time: bool,
}

impl TableSchema {
    pub(super) fn probe(
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

    pub(super) fn read(
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

    pub(super) fn has_all(&self, columns: &[&str]) -> bool {
        columns.iter().all(|column| self.columns.contains(*column))
    }

    pub(super) fn declares(&self, column: &str) -> bool {
        self.columns
            .iter()
            .any(|declared| declared.eq_ignore_ascii_case(column))
    }

    pub(super) fn column_or(&self, column: &'static str, fallback: &'static str) -> &'static str {
        if self.columns.contains(column) {
            column
        } else {
            fallback
        }
    }

    pub(super) fn pushdown(&self, context: &DatabaseLoadContext<'_>) -> DateWindow {
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

pub(super) fn table_exists(
    connection: &sqlite::Connection,
    table: Table,
    db_path: &Path,
    diagnostics: &Diagnostics,
) -> bool {
    match shared_table_exists(connection, table.name()) {
        Ok(exists) => exists,
        Err(error) => {
            sqlite_setup_failure(error).record(db_path, diagnostics);
            false
        }
    }
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
