use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use ccusage_core::{CliError, Diagnostics, Skip, SkipKind};
use serde::de::DeserializeOwned;

use crate::{Split, map_parallel};

const SQLITE_ERROR: isize = 1;
const SQLITE_CORRUPT: isize = 11;
const SQLITE_SCHEMA: isize = 17;
const SQLITE_NOTADB: isize = 26;

#[derive(Debug)]
pub struct UnitError {
    fault: Fault,
    detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fault {
    Skip(SkipKind),
    Unwritten,
    Unfinished,
}

impl UnitError {
    pub fn new(kind: SkipKind, detail: impl Into<String>) -> Self {
        Self {
            fault: Fault::Skip(kind),
            detail: detail.into(),
        }
    }

    /// Marks a unit as incomplete. Reports classify it as invalid data after the
    /// in-progress window expires.
    pub fn unfinished(detail: impl Into<String>) -> Self {
        Self {
            fault: Fault::Unfinished,
            detail: detail.into(),
        }
    }

    pub fn sqlite(stage: SqliteStage, code: Option<isize>) -> Self {
        let kept = stage == SqliteStage::Rows;
        // NOTE(2026-11-16): SQLite stores the primary result code in the low byte of an extended code. Mask off the high bits to classify both kinds of code consistently.
        let (kind, detail) = match code.map(|code| code & 0xff) {
            Some(SQLITE_ERROR | SQLITE_SCHEMA) if kept => (
                SkipKind::Invariant,
                "database schema changed while reading; earlier rows were kept",
            ),
            Some(SQLITE_ERROR | SQLITE_SCHEMA) => return Self::unsupported_schema(),
            Some(SQLITE_CORRUPT | SQLITE_NOTADB) if kept => (
                SkipKind::InvalidData,
                "database is corrupt or invalid; earlier rows were kept",
            ),
            Some(SQLITE_CORRUPT | SQLITE_NOTADB) => {
                (SkipKind::InvalidData, "database is corrupt or invalid")
            }
            _ if kept => (
                SkipKind::ReadInterrupted,
                "database read stopped early; earlier rows were kept",
            ),
            _ => (SkipKind::Unreadable, "database could not be read"),
        };
        Self::new(kind, detail)
    }

    pub fn unsupported_schema() -> Self {
        Self::new(SkipKind::Invariant, "database schema is not supported")
    }

    #[cold]
    pub fn record(self, unit: &Path, diagnostics: &Diagnostics) {
        let state = match self.fault {
            Fault::Unwritten => UNWRITTEN,
            Fault::Unfinished if being_written(unit) => "incomplete, read as still being written",
            Fault::Unfinished | Fault::Skip(_) => match fs::metadata(unit) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    "removed after discovery, read as absent"
                }
                Ok(metadata) if metadata.is_file() && metadata.len() == 0 => UNWRITTEN,
                _ => return diagnostics.record(Skip::unit(unit, self.kind(), self.detail)),
            },
        };
        diagnostics.debug_note(
            unit,
            format!("{}: {}: {state}", diagnostics.source(), unit.display()),
        );
    }

    fn kind(&self) -> SkipKind {
        match self.fault {
            Fault::Skip(kind) => kind,
            Fault::Unwritten | Fault::Unfinished => SkipKind::InvalidData,
        }
    }

    #[cfg(feature = "sqlite")]
    pub(crate) fn record_row(self, unit: &Path, row: u32, diagnostics: &Diagnostics) {
        diagnostics.record(Skip::record(unit, row, self.kind(), self.detail));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteStage {
    Setup,
    Rows,
}

impl fmt::Display for UnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl From<CliError> for UnitError {
    fn from(_error: CliError) -> Self {
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

const UNWRITTEN: &str = "empty, read as not yet written";

const IN_PROGRESS_WINDOW: Duration = Duration::from_secs(60);

#[cold]
pub(crate) fn being_written(unit: &Path) -> bool {
    fs::metadata(unit)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            let nanos = |duration: Duration| {
                i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos())
            };
            let modified = match modified.duration_since(SystemTime::UNIX_EPOCH) {
                Ok(duration) => nanos(duration),
                Err(error) => -nanos(error.duration()),
            };
            i128::from(ccusage_core::utc_now().as_millis()) * 1_000_000 - modified
                <= nanos(IN_PROGRESS_WINDOW)
        })
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, UnitError> {
    decode_json(&fs::read(path)?)
}

// NOTE(2026-11-16): Whole-document sources such as Cline can rewrite a JSON file in place. A read can therefore see an empty or truncated document while the writer is active.
pub fn decode_json<T: DeserializeOwned>(content: &[u8]) -> Result<T, UnitError> {
    use serde_json::error::Category;

    serde_json::from_slice(content).map_err(|error| {
        let (fault, category) = match error.classify() {
            Category::Eof if content.iter().all(u8::is_ascii_whitespace) => {
                (Fault::Unwritten, "EOF")
            }
            Category::Eof => {
                return UnitError {
                    fault: Fault::Unfinished,
                    detail: format!(
                        "JSON document is truncated at line {}, column {}",
                        error.line(),
                        error.column()
                    ),
                };
            }
            Category::Io => (Fault::Skip(SkipKind::Unreadable), "I/O"),
            Category::Syntax => (Fault::Skip(SkipKind::InvalidData), "syntax"),
            Category::Data => (Fault::Skip(SkipKind::InvalidData), "data"),
        };
        UnitError {
            fault,
            detail: format!(
                "JSON {category} error at line {}, column {}",
                error.line(),
                error.column()
            ),
        }
    })
}

/// Returns one slot per input, in input order. A failed unit records a diagnostic and leaves an empty slot.
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
    read_typed_units(files, single_thread, diagnostics, |file| {
        read(file.as_path())
    })
}

pub fn read_typed_units<U, T, E, F>(
    units: &[U],
    single_thread: bool,
    diagnostics: &Diagnostics,
    read: F,
) -> Vec<Option<T>>
where
    U: AsRef<Path> + Sync,
    T: Send,
    E: Into<UnitError>,
    F: Fn(&U) -> Result<T, E> + Sync,
{
    map_parallel(
        units,
        single_thread,
        Split::BySize(U::as_ref),
        |unit| match read(unit) {
            Ok(value) => Some(value),
            Err(error) => {
                error.into().record(unit.as_ref(), diagnostics);
                None
            }
        },
    )
}

pub fn read_typed_units_flat<U, T, I, E, F>(
    units: &[U],
    single_thread: bool,
    diagnostics: &Diagnostics,
    read: F,
) -> Vec<T>
where
    U: AsRef<Path> + Sync,
    I: IntoIterator<Item = T> + Send,
    E: Into<UnitError>,
    F: Fn(&U) -> Result<I, E> + Sync,
{
    read_typed_units(units, single_thread, diagnostics, read)
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

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
pub(crate) mod tests {
    use std::{
        fs,
        path::Path,
        time::{Duration, SystemTime},
    };

    use ccusage_core::{Diagnostics, Result, SkipKind, cli::SharedArgs, cli_error};
    use ccusage_test_support::Fixture;

    use super::{
        Fault, IN_PROGRESS_WINDOW, SqliteStage, UnitError, decode_json, read_units, read_units_flat,
    };

    pub(crate) fn age_past_write_window(path: &Path) {
        let modified = SystemTime::now() - IN_PROGRESS_WINDOW - Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn typed_units_keep_metadata_order_and_failed_slots() {
        struct Unit {
            path: std::path::PathBuf,
            id: u32,
        }
        impl AsRef<Path> for Unit {
            fn as_ref(&self) -> &Path {
                &self.path
            }
        }
        let fixture = Fixture::new();
        let units = [
            Unit {
                path: fixture.write_file("a", "1"),
                id: 7,
            },
            Unit {
                path: fixture.write_file("b", "2"),
                id: 3,
            },
            Unit {
                path: fixture.write_file("c", "3"),
                id: 9,
            },
        ];
        for single_thread in [true, false] {
            let diagnostics = Diagnostics::new("test");
            let loaded = super::read_typed_units(&units, single_thread, &diagnostics, |unit| {
                if unit.id == 3 {
                    Err(UnitError::new(SkipKind::InvalidData, "rejected"))
                } else {
                    Ok(unit.id)
                }
            });
            assert_eq!(loaded, [Some(7), None, Some(9)]);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }

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
            fixture.create_dir_all("directory.txt"),
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
                files.push(fixture.create_dir_all(format!("directory-{index}.txt")));
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
        let files = vec![fixture.write_file("state.db", "SQLite format 3")];
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
        let files = vec![fixture.create_dir_all("directory.txt")];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(&files, true, &diagnostics, |file| fs::read(file));

        assert_eq!(loaded, [None]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn a_unit_removed_after_discovery_or_not_yet_written_is_noted_not_skipped() {
        let fixture = Fixture::new();
        let missing = fixture.path("missing.json");
        let empty = fixture.write_file("empty.json", "");
        let files = vec![missing.clone(), empty.clone()];
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(
            &files,
            true,
            &diagnostics,
            super::read_json::<serde_json::Value>,
        );

        assert_eq!(loaded, [None, None]);
        for kind in [SkipKind::Unreadable, SkipKind::InvalidData] {
            assert_eq!(diagnostics.skipped(kind), 0, "{kind:?}");
        }
        let debug = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        assert_eq!(
            diagnostics.warnings(&debug),
            [
                format!("test: {}: empty, read as not yet written", empty.display()),
                format!(
                    "test: {}: removed after discovery, read as absent",
                    missing.display()
                ),
            ]
        );
    }

    #[test]
    fn read_json_classifies_read_and_decode_failures_separately() {
        let fixture = Fixture::new();
        let files = vec![
            fixture.create_dir_all("directory.json"),
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
    fn a_document_cut_off_by_the_end_of_the_file_is_noted_as_still_being_written() {
        let fixture = Fixture::new();
        let cut = fixture.write_file("cut.json", r#"{"usage":{"inp"#);
        let files = vec![
            cut.clone(),
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
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
        let debug = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        assert_eq!(
            diagnostics.warnings(&debug)[0],
            format!(
                "test: {}: incomplete, read as still being written",
                cut.display()
            )
        );
    }

    #[test]
    fn a_document_left_cut_off_past_the_write_window_is_skipped_as_truncated() {
        let fixture = Fixture::new();
        let cut = fixture.write_file("cut.json", r#"{"usage":{"inp"#);
        age_past_write_window(&cut);
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(
            std::slice::from_ref(&cut),
            true,
            &diagnostics,
            super::read_json::<serde_json::Value>,
        );

        assert_eq!(loaded, [None]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        let debug = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        let warnings = diagnostics.warnings(&debug).join("\n");
        assert!(
            warnings.contains(&format!(
                "{}: JSON document is truncated at line 1, column 14",
                cut.display()
            )),
            "{warnings}"
        );
        assert!(!warnings.contains("still being written"), "{warnings}");
    }

    #[test]
    fn an_unfinished_unit_is_noted_while_written_and_skipped_once_left_alone() {
        let fixture = Fixture::new();
        let fresh = fixture.write_file("fresh.zst", "frame");
        let stale = fixture.write_file("stale.zst", "frame");
        age_past_write_window(&stale);
        let diagnostics = Diagnostics::new("test");

        for unit in [&fresh, &stale] {
            UnitError::unfinished("stream ends inside a frame").record(unit, &diagnostics);
        }

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        let debug = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        let warnings = diagnostics.warnings(&debug).join("\n");
        assert!(
            warnings.contains(&format!(
                "test: {}: incomplete, read as still being written",
                fresh.display()
            )),
            "{warnings}"
        );
        assert!(
            warnings.contains(&format!("{}: stream ends inside a frame", stale.display())),
            "{warnings}"
        );
    }

    #[test]
    fn a_whitespace_only_document_is_noted_like_an_empty_file() {
        let fixture = Fixture::new();
        let blank = fixture.write_file("blank.json", " \n\t\r\n");
        let diagnostics = Diagnostics::new("test");

        let loaded = read_units(
            std::slice::from_ref(&blank),
            true,
            &diagnostics,
            super::read_json::<serde_json::Value>,
        );

        assert_eq!(loaded, [None]);
        let debug = SharedArgs {
            debug: true,
            ..SharedArgs::default()
        };
        assert_eq!(
            diagnostics.warnings(&debug),
            [format!(
                "test: {}: empty, read as not yet written",
                blank.display()
            )]
        );
    }

    #[test]
    fn serde_error_details_keep_only_category_and_location() {
        let error = decode_json::<u64>(br#""SECRET_CANARY""#).unwrap_err();

        assert_eq!(error.fault, Fault::Skip(SkipKind::InvalidData));
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
                UnitError::sqlite(SqliteStage::Setup, code).fault,
                Fault::Skip(setup),
                "setup {code:?}"
            );
            assert_eq!(
                UnitError::sqlite(SqliteStage::Rows, code).fault,
                Fault::Skip(rows),
                "rows {code:?}"
            );
        }
    }

    #[test]
    fn unstructured_reader_errors_never_echo_input() {
        let error = UnitError::from(cli_error("invalid token SECRET_CANARY"));

        assert_eq!(error.fault, Fault::Skip(SkipKind::Unreadable));
        assert!(!error.to_string().contains("SECRET_CANARY"));
    }

    #[test]
    fn custom_io_errors_never_echo_input() {
        let error = UnitError::from(std::io::Error::other("SECRET_CANARY"));

        assert_eq!(error.fault, Fault::Skip(SkipKind::Unreadable));
        assert!(!error.to_string().contains("SECRET_CANARY"));
    }
}
