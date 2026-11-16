use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use ccusage_core::{CliError, Diagnostics, Skip, SkipKind};
use serde::de::DeserializeOwned;

use crate::read_files_parallel;

/// Why a whole unit (one file or database) was left out.
///
/// Typed decoding errors retain their category and location; erased reader
/// errors convert to [`SkipKind::Unreadable`] without their untrusted text.
#[derive(Debug)]
pub struct UnitError {
    kind: SkipKind,
    detail: String,
}

impl UnitError {
    pub fn new(kind: SkipKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// Classifies a failed SQLite call by its result code, so every database
    /// source names the same cause the same way.
    ///
    /// A statement the database cannot run (`SQLITE_ERROR`, `SQLITE_SCHEMA`)
    /// means its schema is not the one the loader reads; a corrupt file or one
    /// that is not a database at all is invalid data; anything else, such as a
    /// lock or an I/O error, left the database unreadable, or interrupted a
    /// read whose earlier rows are kept.
    pub fn sqlite(stage: SqliteStage, code: Option<isize>) -> Self {
        let kept = stage == SqliteStage::Rows;
        let (kind, detail) = match code.map(|code| code & 0xff) {
            Some(1 | 17) if kept => (
                SkipKind::Invariant,
                "database schema changed while reading; earlier rows were kept",
            ),
            Some(1 | 17) => (SkipKind::Invariant, "database schema is not supported"),
            Some(11 | 26) if kept => (
                SkipKind::InvalidData,
                "database is corrupt or invalid; earlier rows were kept",
            ),
            Some(11 | 26) => (SkipKind::InvalidData, "database is corrupt or invalid"),
            _ if kept => (
                SkipKind::ReadInterrupted,
                "database read stopped early; earlier rows were kept",
            ),
            _ => (SkipKind::Unreadable, "database could not be read"),
        };
        Self::new(kind, detail)
    }

    /// Records this failure of `unit`. A reader that keeps what it read
    /// before the failure calls this itself; [`read_units`] calls it for a
    /// unit that failed as a whole.
    pub fn record(self, unit: &Path, diagnostics: &Diagnostics) {
        diagnostics.record(Skip::unit(unit, self.kind, self.detail));
    }
}

/// How far reading a database got when a SQLite call failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteStage {
    /// Opening the database or preparing a statement, before any row is kept.
    Setup,
    /// Stepping through rows, keeping the ones read before the failure.
    Rows,
}

impl fmt::Display for UnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl From<CliError> for UnitError {
    fn from(_error: CliError) -> Self {
        // The erased error may include input; typed readers retain safe details.
        Self::new(SkipKind::Unreadable, "unit reader failed")
    }
}

impl From<io::Error> for UnitError {
    fn from(error: io::Error) -> Self {
        Self::new(
            SkipKind::Unreadable,
            format!("I/O error ({:?})", error.kind()),
        )
    }
}

impl From<serde_json::Error> for UnitError {
    fn from(error: serde_json::Error) -> Self {
        use serde_json::error::Category;

        let (kind, category) = match error.classify() {
            Category::Io => (SkipKind::Unreadable, "I/O"),
            Category::Syntax => (SkipKind::InvalidData, "syntax"),
            Category::Data => (SkipKind::InvalidData, "data"),
            Category::Eof => (SkipKind::InvalidData, "EOF"),
        };
        Self::new(
            kind,
            format!(
                "JSON {category} error at line {}, column {}",
                error.line(),
                error.column()
            ),
        )
    }
}

/// Reads a whole JSON unit and decodes it without copying input into a string.
///
/// File IO failures are recorded as unreadable units; decoding failures are
/// recorded separately as invalid data, with only the error category and byte
/// location retained for diagnostics.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, UnitError> {
    let content = fs::read(path)?;
    Ok(serde_json::from_slice(&content)?)
}

/// Reads `files` like [`read_files_parallel`], and records a unit whose read
/// fails in `diagnostics` instead of leaving that to each call site.
///
/// Returns one slot per file, in file order; `None` marks a failed unit, so a
/// caller that tracks relationships between files can tell it from one that
/// read as empty. A reader that skips single records while the unit still
/// reads calls [`Diagnostics::record`] itself.
pub fn read_units<T, E, F>(
    files: &[PathBuf],
    single_thread: bool,
    diagnostics: &Diagnostics,
    read: F,
) -> Vec<Option<T>>
where
    T: Send,
    E: Into<UnitError>,
    F: Fn(&Path) -> Result<T, E> + Sync,
{
    read_files_parallel(files, single_thread, |file| match read(file) {
        Ok(value) => Some(value),
        Err(error) => {
            error.into().record(file, diagnostics);
            None
        }
    })
}

/// [`read_units`] for readers that yield a unit's records: the records of
/// every unit that read, concatenated in file order.
pub fn read_units_flat<T, I, E, F>(
    files: &[PathBuf],
    single_thread: bool,
    diagnostics: &Diagnostics,
    read: F,
) -> Vec<T>
where
    I: IntoIterator<Item = T> + Send,
    E: Into<UnitError>,
    F: Fn(&Path) -> Result<I, E> + Sync,
{
    read_units(files, single_thread, diagnostics, read)
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use ccusage_core::{Diagnostics, Result, SkipKind, cli_error};
    use ccusage_test_support::Fixture;

    use super::{SqliteStage, UnitError, read_units, read_units_flat};

    fn numbers(path: &Path) -> Result<Vec<u32>> {
        let content = fs::read_to_string(path)?;
        content
            .split_whitespace()
            .map(|word| {
                word.parse::<u32>()
                    .map_err(|error| cli_error(format!("{word}: {error}")))
            })
            .collect()
    }

    #[test]
    fn keeps_one_slot_per_file_and_records_the_failed_ones() {
        let fixture = Fixture::new();
        let files = vec![
            fixture.write_file("a.txt", "1 2"),
            fixture.path("missing.txt"),
            fixture.write_file("empty.txt", ""),
            fixture.write_file("broken.txt", "3 four"),
        ];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(&files, true, &diagnostics, numbers);

        assert_eq!(loaded, [Some(vec![1, 2]), None, Some(Vec::new()), None]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 2);
    }

    #[test]
    fn flat_reads_concatenate_in_file_order_on_any_thread_count() {
        let fixture = Fixture::new();
        let mut files = Vec::new();
        for index in 0..64_u32 {
            if index % 7 == 3 {
                files.push(fixture.path(format!("missing-{index}.txt")));
            } else {
                let padding = " ".repeat((index as usize % 5) * 512);
                files.push(fixture.write_file(
                    format!("file-{index:02}.txt"),
                    format!("{index}{padding} {}", index + 1000),
                ));
            }
        }
        let expected = (0..64_u32)
            .filter(|index| index % 7 != 3)
            .flat_map(|index| [index, index + 1000])
            .collect::<Vec<_>>();

        for single_thread in [true, false] {
            let diagnostics = Diagnostics::new("test");

            let loaded = read_units_flat(&files, single_thread, &diagnostics, numbers);

            assert_eq!(loaded, expected, "single_thread {single_thread}");
            assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 9);
        }
    }

    #[test]
    fn a_reader_chooses_the_kind_of_its_failure() {
        let fixture = Fixture::new();
        let files = vec![fixture.write_file("state.db", "")];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(&files, true, &diagnostics, |_| {
            Err::<(), _>(UnitError::new(SkipKind::Invariant, "no usage table"))
        });

        assert_eq!(loaded, [None]);
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
    }

    #[test]
    fn io_errors_convert_without_a_wrapper() {
        let fixture = Fixture::new();
        let files = vec![fixture.path("missing.txt")];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(&files, true, &diagnostics, |file| {
            fs::metadata(file).map(|metadata| metadata.len())
        });

        assert_eq!(loaded, [None]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn read_json_classifies_read_and_decode_failures_separately() {
        let fixture = Fixture::new();
        let files = vec![
            fixture.path("missing.json"),
            fixture.write_file("broken.json", "{\"value\":}"),
        ];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(
            &files,
            true,
            &diagnostics,
            super::read_json::<serde_json::Value>,
        );

        assert_eq!(loaded, [None, None]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn serde_error_details_keep_only_category_and_location() {
        let error = serde_json::from_str::<u64>(r#""SECRET_CANARY""#).unwrap_err();

        let error = UnitError::from(error);

        assert_eq!(error.kind, SkipKind::InvalidData);
        assert!(error.detail.contains("data"));
        assert!(error.detail.contains("line 1"));
        assert!(error.detail.contains("column"));
        assert!(!error.detail.contains("SECRET_CANARY"));
        assert_eq!(error.to_string(), error.detail.as_str());
    }

    #[test]
    fn sqlite_failures_are_classified_by_primary_result_code() {
        for (code, setup, rows) in [
            (Some(1), SkipKind::Invariant, SkipKind::Invariant),
            (Some(17), SkipKind::Invariant, SkipKind::Invariant),
            (Some(273), SkipKind::Invariant, SkipKind::Invariant),
            (Some(11), SkipKind::InvalidData, SkipKind::InvalidData),
            (Some(26), SkipKind::InvalidData, SkipKind::InvalidData),
            (Some(267), SkipKind::InvalidData, SkipKind::InvalidData),
            (Some(5), SkipKind::Unreadable, SkipKind::ReadInterrupted),
            (Some(6), SkipKind::Unreadable, SkipKind::ReadInterrupted),
            (Some(10), SkipKind::Unreadable, SkipKind::ReadInterrupted),
            (Some(14), SkipKind::Unreadable, SkipKind::ReadInterrupted),
            (Some(261), SkipKind::Unreadable, SkipKind::ReadInterrupted),
            (None, SkipKind::Unreadable, SkipKind::ReadInterrupted),
        ] {
            assert_eq!(
                UnitError::sqlite(SqliteStage::Setup, code).kind,
                setup,
                "setup {code:?}"
            );
            assert_eq!(
                UnitError::sqlite(SqliteStage::Rows, code).kind,
                rows,
                "rows {code:?}"
            );
        }
    }

    #[test]
    fn unstructured_reader_errors_never_echo_input() {
        let error = UnitError::from(cli_error("invalid token SECRET_CANARY"));

        assert_eq!(error.kind, SkipKind::Unreadable);
        assert!(!error.to_string().contains("SECRET_CANARY"));
    }

    #[test]
    fn custom_io_errors_never_echo_input() {
        let error = UnitError::from(std::io::Error::other("SECRET_CANARY"));

        assert_eq!(error.kind, SkipKind::Unreadable);
        assert!(!error.to_string().contains("SECRET_CANARY"));
    }
}
