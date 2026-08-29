use std::{
    env, fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[cfg(test)]
use std::{
    fs::{File, FileTimes},
    time::Duration,
};

use jiff::{civil::Date, tz::TimeZone as JiffTimeZone};

use crate::{
    Result, cli::SharedArgs, cli_error, date_range_bounds_ms, fast::FxHashSet, home, parse_tz,
};

pub(super) fn codex_usage_sources() -> Result<Vec<CodexUsageSource>> {
    Ok(codex_usage_sources_from_homes(codex_home_paths()?))
}

#[cfg(test)]
fn codex_usage_paths_from_homes(homes: Vec<PathBuf>) -> Vec<PathBuf> {
    codex_usage_sources_from_homes(homes)
        .into_iter()
        .map(|source| source.dir)
        .collect()
}

fn codex_usage_sources_from_homes(homes: Vec<PathBuf>) -> Vec<CodexUsageSource> {
    let mut paths = Vec::new();
    let mut seen = FxHashSet::default();
    for path in homes {
        let sessions = path.join("sessions");
        let archived_sessions = path.join("archived_sessions");
        let mut found_usage_dir = false;
        if sessions.is_dir() {
            if seen.insert(sessions.clone()) {
                paths.push(CodexUsageSource {
                    dir: sessions,
                    dedupe_scope: path.clone(),
                });
            }
            found_usage_dir = true;
        }
        if archived_sessions.is_dir() {
            if seen.insert(archived_sessions.clone()) {
                paths.push(CodexUsageSource {
                    dir: archived_sessions,
                    dedupe_scope: path.clone(),
                });
            }
            found_usage_dir = true;
        }
        if !found_usage_dir && seen.insert(path.clone()) {
            paths.push(CodexUsageSource {
                dir: path.clone(),
                dedupe_scope: path,
            });
        }
    }
    paths
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodexUsageSource {
    pub(super) dir: PathBuf,
    dedupe_scope: PathBuf,
}

#[cfg(test)]
impl CodexUsageSource {
    pub(super) fn new_for_test(dir: PathBuf, dedupe_scope: PathBuf) -> Self {
        Self { dir, dedupe_scope }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CodexUsageFileGroup {
    pub(super) dir: PathBuf,
    pub(super) files: Vec<PathBuf>,
}

pub(super) fn collect_codex_usage_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    crate::collect_usage_files(dir, &mut files);
    files.sort();
    files
}

pub(super) fn collect_deduped_codex_usage_files(
    sources: &[CodexUsageSource],
) -> Vec<CodexUsageFileGroup> {
    let mut seen = FxHashSet::default();
    let mut groups = Vec::new();
    for source in sources {
        let files = collect_codex_usage_files(&source.dir)
            .into_iter()
            .filter(|file| seen.insert(codex_usage_file_key(source, file)))
            .collect::<Vec<_>>();
        groups.push(CodexUsageFileGroup {
            dir: source.dir.clone(),
            files,
        });
    }
    groups
}

pub(super) fn filter_codex_usage_files(
    sessions_dir: &Path,
    files: &[PathBuf],
    shared: &SharedArgs,
) -> Vec<PathBuf> {
    let Some(eligibility) = CodexFileEligibility::from_shared(shared) else {
        return files.to_vec();
    };
    files
        .iter()
        .filter(|file| {
            eligibility.contains(
                codex_file_date(sessions_dir, file),
                file_modified_millis(file),
            )
        })
        .cloned()
        .collect()
}

#[derive(Clone, Copy)]
struct CodexFileEligibility {
    since_date: Option<Date>,
    until_path_date: Option<Date>,
    since_millis: Option<i64>,
}

impl CodexFileEligibility {
    fn from_shared(shared: &SharedArgs) -> Option<Self> {
        let timezone =
            parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
        let since_date = shared.since.as_deref().and_then(parse_compact_date);
        let until_date = shared.until.as_deref().and_then(parse_compact_date);
        let (since_millis, until_millis) = date_range_bounds_ms(
            shared.since.as_deref(),
            shared.until.as_deref(),
            timezone.as_ref(),
        );
        if since_date.is_none() && until_date.is_none() {
            return None;
        }
        Some(Self {
            since_date,
            until_path_date: until_millis.and_then(utc_path_date),
            since_millis,
        })
    }

    fn contains(self, start_date: Option<Date>, modified_millis: Option<i64>) -> bool {
        self.until_path_date
            .is_none_or(|until| start_date.is_none_or(|date| date <= until))
            && self.since_date.is_none_or(|since| {
                start_date.is_none_or(|date| date >= since)
                    || modified_millis.is_none_or(|modified| {
                        self.since_millis
                            .is_none_or(|since_millis| modified >= since_millis)
                    })
            })
    }
}

fn utc_path_date(millis: i64) -> Option<Date> {
    let timestamp = jiff::Timestamp::from_millisecond(millis).ok()?;
    let zoned = timestamp.to_zoned(JiffTimeZone::get("UTC").ok()?);
    Date::new(
        i16::from(zoned.year()),
        i8::from(zoned.month()),
        i8::from(zoned.day()),
    )
    .ok()
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

fn codex_file_date(sessions_dir: &Path, file: &Path) -> Option<Date> {
    let relative = file.strip_prefix(sessions_dir).ok()?;
    let components = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components.windows(3).find_map(|parts| {
        let year = parse_date_part(parts[0], 4)?;
        let month = parse_date_part(parts[1], 2)?;
        let day = parse_date_part(parts[2], 2)?;
        Date::new(year as i16, month as i8, day as i8).ok()
    })
}

fn parse_compact_date(value: &str) -> Option<Date> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = parse_date_part(&value[..4], 4)?;
    let month = parse_date_part(&value[4..6], 2)?;
    let day = parse_date_part(&value[6..], 2)?;
    Date::new(year as i16, month as i8, day as i8).ok()
}

fn parse_date_part(value: &str, length: usize) -> Option<u16> {
    (value.len() == length && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| {
        value
            .bytes()
            .fold(0_u16, |number, byte| number * 10 + u16::from(byte - b'0'))
    })
}

fn codex_usage_file_key(source: &CodexUsageSource, file: &Path) -> (PathBuf, PathBuf) {
    let relative = file.strip_prefix(&source.dir).unwrap_or(file).to_path_buf();
    (source.dedupe_scope.clone(), relative)
}

pub(super) fn codex_home_paths() -> Result<Vec<PathBuf>> {
    if let Ok(env_paths) = env::var("CODEX_HOME") {
        return Ok(env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect());
    }

    let home = home::home_dir().ok_or_else(|| cli_error("home directory is not set"))?;
    Ok(vec![home.join(".codex")])
}

#[cfg(test)]
pub(super) fn set_file_modified(path: &Path, timestamp: crate::TimestampMs) {
    let milliseconds = u64::try_from(timestamp.as_millis()).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(milliseconds)))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::Fixture;

    #[test]
    fn includes_archived_sessions_next_to_sessions() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");
        let _ = fixture.create_dir_all("codex/archived_sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(
            paths,
            vec![
                fixture.path("codex/sessions"),
                fixture.path("codex/archived_sessions"),
            ]
        );
    }

    #[test]
    fn uses_sessions_without_missing_archived_sessions_path() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn uses_archived_sessions_without_direct_path_fallback() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/archived_sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(paths, vec![fixture.path("codex/archived_sessions")]);
    }

    #[test]
    fn falls_back_to_direct_path_when_no_session_directories_exist() {
        let fixture = Fixture::new();
        let home = fixture.create_dir_all("codex");

        let paths = codex_usage_paths_from_homes(vec![home.clone()]);

        assert_eq!(paths, vec![home]);
    }

    #[test]
    fn deduplicates_usage_paths_across_repeated_homes() {
        let fixture = Fixture::new();
        let home = fixture.create_dir_all("codex");
        let _ = fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![home.clone(), home]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn keeps_active_session_file_when_archived_file_has_same_relative_path() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("codex/sessions");
        let _ = fixture.create_dir_all("codex/archived_sessions");
        let _ = fixture.write_file("codex/sessions/session.jsonl", "");
        let _ = fixture.write_file("codex/archived_sessions/session.jsonl", "");
        let _ = fixture.write_file("codex/archived_sessions/archive-only.jsonl", "");

        let sources = codex_usage_sources_from_homes(vec![fixture.path("codex")]);
        let groups = collect_deduped_codex_usage_files(&sources);

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    dir: fixture.path("codex/sessions"),
                    files: vec![fixture.path("codex/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    dir: fixture.path("codex/archived_sessions"),
                    files: vec![fixture.path("codex/archived_sessions/archive-only.jsonl")],
                },
            ]
        );
    }

    #[test]
    fn keeps_same_relative_session_file_across_different_homes() {
        let fixture = Fixture::new();
        let _ = fixture.create_dir_all("work/sessions");
        let _ = fixture.create_dir_all("personal/sessions");
        let _ = fixture.write_file("work/sessions/session.jsonl", "");
        let _ = fixture.write_file("personal/sessions/session.jsonl", "");

        let sources =
            codex_usage_sources_from_homes(vec![fixture.path("work"), fixture.path("personal")]);
        let groups = collect_deduped_codex_usage_files(&sources);

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    dir: fixture.path("work/sessions"),
                    files: vec![fixture.path("work/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    dir: fixture.path("personal/sessions"),
                    files: vec![fixture.path("personal/sessions/session.jsonl")],
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn deduplicates_non_utf8_relative_session_paths_without_lossy_strings() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let file_name = PathBuf::from(OsString::from_vec(b"session-\xFF.jsonl".to_vec()));
        let source = CodexUsageSource::new_for_test(
            PathBuf::from("/codex/sessions"),
            PathBuf::from("/codex"),
        );

        assert_eq!(
            codex_usage_file_key(&source, &source.dir.join(&file_name)),
            (PathBuf::from("/codex"), file_name)
        );
    }

    #[test]
    fn keeps_a_resumed_session_that_started_more_than_a_month_before_since() {
        let fixture = Fixture::new();
        let sessions_dir = fixture.create_dir_all("codex/sessions");
        let historical = fixture.write_file("codex/sessions/2025/01/01/historical.jsonl", "");
        let resumed = fixture.write_file("codex/sessions/2026/01/01/resumed.jsonl", "");
        let current = fixture.write_file("codex/sessions/2026/03/15/current.jsonl", "");
        set_file_modified(
            &historical,
            crate::parse_ts_timestamp("2025-01-01T00:00:00.000Z").unwrap(),
        );
        set_file_modified(
            &resumed,
            crate::parse_ts_timestamp("2026-03-15T12:00:00.000Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260315".to_string()),
            until: Some("20260315".to_string()),
            ..SharedArgs::default()
        };

        let filtered = filter_codex_usage_files(
            &sessions_dir,
            &[historical, resumed.clone(), current.clone()],
            &shared,
        );

        assert_eq!(filtered, vec![resumed, current]);
    }

    #[test]
    fn keeps_undated_files_when_date_filter_cannot_prove_their_range() {
        let fixture = Fixture::new();
        let sessions_dir = fixture.create_dir_all("codex/sessions");
        let file = fixture.write_file("codex/sessions/custom.jsonl", "");
        let shared = SharedArgs {
            since: Some("20260131".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filter_codex_usage_files(&sessions_dir, std::slice::from_ref(&file), &shared),
            vec![file]
        );
    }
}
