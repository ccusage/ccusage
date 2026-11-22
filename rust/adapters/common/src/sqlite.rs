use std::{path::Path, sync::Once};

use ccusage_core::Diagnostics;
use sqlite::{Connection, OpenFlags, State, Statement, Type};

use crate::{SqliteStage, UnitError, units::read_typed_units_flat};

// NOTE(2026-11-22, SQLite 3.49.1, #2519): Racing first opens crash on arm64 because lazy sqlite3_initialize() can read a null xMutexInit, so initialize once before any worker opens a connection. SQLite fixed this on trunk after 3.54.1 (check-in 37b83ad2f5); drop this once the bundled copy has it. The empty name opens a private temporary database that never touches the disk, because this build omits ":memory:".
fn initialize_sqlite() {
    static INITIALIZED: Once = Once::new();
    INITIALIZED.call_once(|| drop(sqlite::open("")));
}

pub fn open_read_only(path: &Path) -> Result<Connection, UnitError> {
    initialize_sqlite();
    let mut connection = Connection::open_with_flags(path, OpenFlags::new().with_read_only())
        .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))?;
    connection
        .set_busy_timeout(5000)
        .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))?;
    Ok(connection)
}

pub fn table_exists(connection: &Connection, table: &str) -> sqlite::Result<bool> {
    let mut statement = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")?;
    statement.bind((1, table))?;
    Ok(matches!(statement.next()?, State::Row))
}

pub fn sqlite_units<U, T, P, R>(
    units: &[U],
    single_thread: bool,
    diagnostics: &Diagnostics,
    prepare: P,
    read: R,
) -> Vec<T>
where
    U: AsRef<Path> + Sync,
    T: Send,
    P: for<'a> Fn(&'a Connection, &U) -> Result<Statement<'a>, UnitError> + Sync,
    R: Fn(&Statement<'_>, &U, u32) -> Result<Option<T>, UnitError> + Sync,
{
    sqlite_units_with_state(
        units,
        single_thread,
        diagnostics,
        |connection, unit| prepare(connection, unit).map(|statement| (statement, ())),
        |_, statement, unit, row| read(statement, unit, row),
    )
}

pub fn sqlite_units_with_state<U, T, S, P, R>(
    units: &[U],
    single_thread: bool,
    diagnostics: &Diagnostics,
    prepare: P,
    read: R,
) -> Vec<T>
where
    U: AsRef<Path> + Sync,
    T: Send,
    P: for<'a> Fn(&'a Connection, &U) -> Result<(Statement<'a>, S), UnitError> + Sync,
    R: Fn(&mut S, &Statement<'_>, &U, u32) -> Result<Option<T>, UnitError> + Sync,
{
    read_typed_units_flat(units, single_thread, diagnostics, |unit| {
        let connection = open_read_only(unit.as_ref())?;
        let (mut statement, mut state) = prepare(&connection, unit)?;
        let mut records = Vec::new();
        let mut row_number = 0_u32;
        loop {
            match statement.next() {
                Ok(State::Row) => {
                    row_number = row_number.saturating_add(1);
                    match read(&mut state, &statement, unit, row_number) {
                        Ok(Some(record)) => records.push(record),
                        Ok(None) => {}
                        Err(error) => error.record_row(unit.as_ref(), row_number, diagnostics),
                    }
                }
                Ok(State::Done) => break,
                Err(error) => {
                    UnitError::sqlite(SqliteStage::Rows, error.code)
                        .record(unit.as_ref(), diagnostics);
                    break;
                }
            }
        }
        Ok::<_, UnitError>(records)
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Integer(i64),
    Real(f64),
}

impl Number {
    fn as_i64(self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Real(value) => {
                let truncated = value.trunc();
                (truncated >= i64::MIN as f64 && truncated < i64::MAX as f64)
                    .then_some(truncated as i64)
            }
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Real(value) => value,
        }
    }
}

// NOTE(2026-11-17, sqlite 0.37.4): SQLite's numeric reads coerce NULL to zero and clamp out-of-range REAL values in integer reads. Inspect the stored type first to preserve the distinction between absent and zero values.
pub(crate) fn read_number(statement: &Statement<'_>, index: usize) -> Option<Number> {
    match statement.column_type(index).ok()? {
        Type::Integer => statement.read::<i64, _>(index).ok().map(Number::Integer),
        Type::Float | Type::String => statement
            .read::<f64, _>(index)
            .ok()
            .filter(|value| value.is_finite())
            .map(Number::Real),
        Type::Binary | Type::Null => None,
    }
}

pub fn read_i64(statement: &Statement<'_>, index: usize) -> Option<i64> {
    read_number(statement, index)?.as_i64()
}

pub fn read_f64(statement: &Statement<'_>, index: usize) -> Option<f64> {
    read_number(statement, index).map(Number::as_f64)
}

pub fn read_count(statement: &Statement<'_>, index: usize) -> u64 {
    read_i64(statement, index)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Number, read_count, read_f64, read_i64, read_number};

    fn table_probe(table: &str) -> Option<bool> {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path(format!("{table}.db"));
        sqlite::open(&path)
            .unwrap()
            .execute("CREATE TABLE records (value INTEGER); CREATE VIEW record_view AS SELECT value FROM records")
            .unwrap();
        let connection = super::open_read_only(&path).unwrap();
        super::table_exists(&connection, table).ok()
    }

    #[test]
    fn table_exists_sees_tables_but_not_views_or_missing_names() {
        assert_eq!(table_probe("records"), Some(true));
        assert_eq!(table_probe("record_view"), Some(false));
        assert_eq!(table_probe("missing"), Some(false));
    }

    #[test]
    fn table_exists_reports_a_corrupt_database_instead_of_claiming_the_table_is_absent() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.write_file("corrupt.db", "not a SQLite database");
        let connection = super::open_read_only(&path).unwrap();
        assert!(super::table_exists(&connection, "records").is_err());
    }

    #[test]
    fn read_only_database_waits_for_an_exclusive_writer() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("locked.db");
        let writer = sqlite::open(&path).unwrap();
        writer.execute("CREATE TABLE records (value INTEGER); INSERT INTO records VALUES (7); BEGIN EXCLUSIVE").unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = ccusage_core::parallel::spawn_worker(move || {
            started_tx.send(()).unwrap();
            let connection = super::open_read_only(&path).unwrap();
            let result =
                connection
                    .prepare("SELECT value FROM records")
                    .and_then(|mut statement| {
                        statement.next()?;
                        statement.read::<i64, _>(0)
                    });
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        writer.execute("COMMIT").unwrap();
        assert_eq!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            7
        );
        reader.join().unwrap();
    }

    #[test]
    fn sqlite_units_keep_healthy_rows_and_record_rejected_rows() {
        use crate::UnitError;
        use ccusage_core::{Diagnostics, SkipKind};
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("rows.db");
        sqlite::open(&path)
            .unwrap()
            .execute(
                "CREATE TABLE records (value INTEGER); INSERT INTO records VALUES (1), (-1), (3)",
            )
            .unwrap();
        for single_thread in [true, false] {
            let diagnostics = Diagnostics::new("test");
            let loaded = super::sqlite_units(
                std::slice::from_ref(&path),
                single_thread,
                &diagnostics,
                |connection, _| {
                    connection
                        .prepare("SELECT value FROM records ORDER BY rowid")
                        .map_err(|error| UnitError::sqlite(crate::SqliteStage::Setup, error.code))
                },
                |statement, _, _| {
                    let value = read_i64(statement, 0).unwrap();
                    if value < 0 {
                        Err(UnitError::new(SkipKind::InvalidData, "invalid record"))
                    } else {
                        Ok(Some(value))
                    }
                },
            );
            assert_eq!(loaded, [1, 3]);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }

    fn values(
        units: &[std::path::PathBuf],
        sql: &str,
        diagnostics: &ccusage_core::Diagnostics,
    ) -> Vec<i64> {
        let mut loaded = super::sqlite_units(
            units,
            true,
            diagnostics,
            |connection, _| {
                connection.prepare(sql).map_err(|error| {
                    crate::UnitError::sqlite(crate::SqliteStage::Setup, error.code)
                })
            },
            |statement, _, _| Ok(read_i64(statement, 0)),
        );
        loaded.sort_unstable();
        loaded
    }

    #[test]
    fn sqlite_units_keep_healthy_units_beside_a_corrupt_one() {
        use ccusage_core::{Diagnostics, SkipKind};
        let fixture = ccusage_test_support::Fixture::new();
        let healthy = fixture.path("healthy.db");
        sqlite::open(&healthy)
            .unwrap()
            .execute("CREATE TABLE records (value INTEGER); INSERT INTO records VALUES (1), (2)")
            .unwrap();
        let corrupt = fixture.write_file("corrupt.db", "SECRET_CANARY is not a database");
        let diagnostics = Diagnostics::new("test");

        let loaded = values(
            &[healthy, corrupt],
            "SELECT value FROM records",
            &diagnostics,
        );

        assert_eq!(loaded, [1, 2]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&ccusage_core::cli::SharedArgs {
                    debug: true,
                    ..Default::default()
                })
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }

    #[test]
    fn sqlite_units_keep_rows_read_before_a_stepping_failure() {
        use ccusage_core::Diagnostics;
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("rows.db");
        sqlite::open(&path)
            .unwrap()
            .execute(
                "CREATE TABLE records (value INTEGER); INSERT INTO records VALUES (1), (2), (3), (4)",
            )
            .unwrap();
        let diagnostics = Diagnostics::new("test");

        let loaded = values(
            &[path],
            "SELECT CASE WHEN value = 3 THEN abs(-9223372036854775807 - 1) ELSE value END FROM records ORDER BY rowid",
            &diagnostics,
        );

        assert_eq!(loaded, [1, 2]);
        assert!(
            !diagnostics
                .warnings(&ccusage_core::cli::SharedArgs::default())
                .is_empty()
        );
    }

    #[test]
    fn read_only_open_never_creates_or_writes_a_database() {
        let fixture = ccusage_test_support::Fixture::new();
        let missing = fixture.path("missing.db");
        assert!(super::open_read_only(&missing).is_err());
        assert!(!missing.exists());
        let path = fixture.path("existing.db");
        sqlite::open(&path)
            .unwrap()
            .execute("CREATE TABLE records (value INTEGER)")
            .unwrap();
        let connection = super::open_read_only(&path).unwrap();
        assert!(
            connection
                .execute("INSERT INTO records VALUES (1)")
                .is_err()
        );
    }

    fn with_row(values: &str, check: impl FnOnce(&sqlite::Statement<'_>)) {
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection.prepare(format!("SELECT {values}")).unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);
        check(&statement);
    }

    #[test]
    fn reads_each_value_by_its_stored_type() {
        with_row(
            "NULL, 7, 2.5, '12abc', 'x', zeroblob(1), 9e999",
            |statement| {
                assert_eq!(read_number(statement, 0), None);
                assert_eq!(read_number(statement, 1), Some(Number::Integer(7)));
                assert_eq!(read_number(statement, 2), Some(Number::Real(2.5)));
                assert_eq!(read_number(statement, 3), Some(Number::Real(12.0)));
                assert_eq!(read_number(statement, 4), Some(Number::Real(0.0)));
                assert_eq!(read_number(statement, 5), None);
                assert_eq!(read_number(statement, 6), None);
            },
        );
    }

    #[test]
    fn rejects_a_real_outside_the_integer_range_instead_of_clamping_it() {
        with_row("1e300, -1e300, 9e18, 1.9", |statement| {
            assert_eq!(read_i64(statement, 0), None);
            assert_eq!(read_i64(statement, 1), None);
            assert_eq!(read_i64(statement, 2), Some(9_000_000_000_000_000_000));
            assert_eq!(read_i64(statement, 3), Some(1));
            assert_eq!(read_f64(statement, 0), Some(1e300));
        });
    }

    #[test]
    fn counts_missing_negative_and_unreadable_values_as_zero() {
        with_row("NULL, -5, 1e300, 42, 3.7", |statement| {
            assert_eq!(read_count(statement, 0), 0);
            assert_eq!(read_count(statement, 1), 0);
            assert_eq!(read_count(statement, 2), 0);
            assert_eq!(read_count(statement, 3), 42);
            assert_eq!(read_count(statement, 4), 3);
        });
    }
}
