use std::{
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ccusage_adapter_common::{
    roots::{self, RootsEnv},
    visit_files_matching,
};
use ccusage_core::Diagnostics;
use jiff::{civil::Date, tz::TimeZone as JiffTimeZone};

use crate::{
    CodexServiceTier, Result,
    cli::{SharedArgs, home},
    date_range_bounds_ms,
    fast::FxHashSet,
    resolve_tz,
};

use super::reader::{is_rollout_file, uncompressed_rollout_path};

#[cfg(test)]
mod test_support;

pub(super) fn codex_usage_sources() -> Result<Vec<CodexUsageSource>> {
    Ok(codex_usage_sources_from_homes(codex_home_paths()?))
}

fn codex_usage_sources_from_homes(homes: Vec<PathBuf>) -> Vec<CodexUsageSource> {
    let mut paths = Vec::new();
    let mut seen = FxHashSet::default();
    for path in homes {
        let configured_tier = super::speed::configured_tier(&path);
        let source = |dir| CodexUsageSource {
            dir,
            dedupe_scope: path.clone(),
            configured_tier,
        };
        let sessions = path.join("sessions");
        let archived_sessions = path.join("archived_sessions");
        let mut found_usage_dir = false;
        if sessions.is_dir() {
            if seen.insert(sessions.clone()) {
                paths.push(source(sessions));
            }
            found_usage_dir = true;
        }
        if archived_sessions.is_dir() {
            if seen.insert(archived_sessions.clone()) {
                paths.push(source(archived_sessions));
            }
            found_usage_dir = true;
        }
        if !found_usage_dir && seen.insert(path.clone()) {
            paths.push(source(path.clone()));
        }
    }
    paths
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodexUsageSource {
    dir: PathBuf,
    dedupe_scope: PathBuf,
    pub(super) configured_tier: Option<CodexServiceTier>,
}

impl CodexUsageSource {
    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CodexUsageFileGroup {
    pub(super) source: CodexUsageSource,
    pub(super) files: Vec<PathBuf>,
}

pub(super) fn visit_deduped_codex_usage_files(
    sources: &[CodexUsageSource],
    diagnostics: &Diagnostics,
    mut visit: impl FnMut(usize, PathBuf),
) {
    let mut seen = FxHashSet::default();
    for (group, source) in sources.iter().enumerate() {
        let _ = visit_files_matching(
            &source.dir,
            &is_rollout_file,
            &mut |path| {
                // NOTE(2026-10-14, #1890): Codex restores the plain `.jsonl` before deleting the compressed copy on resume. Prefer the plain rollout while both exist.
                if seen.insert(codex_usage_file_key(source, &path)) {
                    visit(group, path);
                }
                ControlFlow::Continue(())
            },
            diagnostics,
        );
    }
}

pub(super) fn collect_deduped_codex_usage_files(
    sources: &[CodexUsageSource],
    diagnostics: &Diagnostics,
) -> Vec<CodexUsageFileGroup> {
    let mut groups = sources
        .iter()
        .map(|source| CodexUsageFileGroup {
            source: source.clone(),
            files: Vec::new(),
        })
        .collect::<Vec<_>>();
    visit_deduped_codex_usage_files(sources, diagnostics, |group, path| {
        groups[group].files.push(path);
    });
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
        .filter(|file| eligibility.contains_file(sessions_dir, file))
        .cloned()
        .collect()
}

#[derive(Clone, Copy)]
pub(super) struct CodexFileEligibility {
    since_date: Option<Date>,
    until_path_date: Option<Date>,
    since_millis: Option<i64>,
}

impl CodexFileEligibility {
    pub(super) fn from_shared(shared: &SharedArgs) -> Option<Self> {
        let timezone = resolve_tz(shared.timezone.as_ref());
        let since_date = shared.since.and_then(|date| date.day().civil());
        let until_date = shared.until.and_then(|date| date.day().civil());
        let (since_millis, until_millis) =
            date_range_bounds_ms(shared.since, shared.until, &timezone);
        if since_date.is_none() && until_date.is_none() {
            return None;
        }
        Some(Self {
            since_date,
            // NOTE(2026-11-18, #2097): Codex names rollout paths in its local calendar. A later path day can still contain events before the UTC cutoff.
            until_path_date: until_millis
                .and_then(|end| end.checked_add(crate::MILLIS_PER_DAY - 1))
                .and_then(utc_path_date),
            since_millis,
        })
    }

    pub(super) fn contains_file(self, dir: &Path, file: &Path) -> bool {
        self.contains(codex_file_date(dir, file), file_modified_millis(file))
    }

    fn contains(self, start_date: Option<Date>, modified_millis: Option<i64>) -> bool {
        self.until_path_date
            .is_none_or(|until| start_date.is_none_or(|date| date <= until))
            && self.since_date.is_none_or(|since| {
                start_date.is_some_and(|date| date >= since)
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
    Date::new(zoned.year(), zoned.month(), zoned.day()).ok()
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
        Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
    })
}

fn parse_date_part(value: &str, length: usize) -> Option<u16> {
    (value.len() == length && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| {
        value
            .bytes()
            .fold(0_u16, |number, byte| number * 10 + u16::from(byte - b'0'))
    })
}

fn codex_usage_file_key(source: &CodexUsageSource, file: &Path) -> (PathBuf, PathBuf) {
    let relative = file.trim_prefix(&source.dir);
    (
        source.dedupe_scope.clone(),
        uncompressed_rollout_path(relative),
    )
}

fn codex_home_paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List("CODEX_HOME"), || {
        home::home_dir()
            .map(|home| home.join(".codex"))
            .into_iter()
            .collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_usage_paths_from_homes(homes: Vec<PathBuf>) -> Vec<PathBuf> {
        codex_usage_sources_from_homes(homes)
            .into_iter()
            .map(|source| source.dir)
            .collect()
    }

    use ccusage_core::SkipKind;
    use ccusage_test_support::Fixture;

    #[test]
    fn configured_codex_roots_share_list_normalization() {
        let fixture = Fixture::new();
        let first = fixture.create_dir_all("first");
        let second = fixture.create_dir_all("second");
        ccusage_test_support::Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (
                "CODEX_HOME",
                Some(" ~/first, ~/second, ~/first/., , ".into()),
            ),
        ])
        .run(|| {
            assert_eq!(codex_home_paths().unwrap(), vec![first, second]);
        })
    }

    #[test]
    fn blank_codex_list_uses_the_default_home() {
        let fixture = Fixture::new();
        let root = fixture.create_dir_all(".codex");
        ccusage_test_support::Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            ("CODEX_HOME", Some(" , , ".into())),
        ])
        .run(|| {
            assert_eq!(codex_home_paths().unwrap(), vec![root]);
        })
    }

    #[test]
    fn includes_archived_sessions_next_to_sessions() {
        let fixture = Fixture::new();
        fixture.create_dir_all("codex/sessions");
        fixture.create_dir_all("codex/archived_sessions");

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
        fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![fixture.path("codex")]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn uses_archived_sessions_without_direct_path_fallback() {
        let fixture = Fixture::new();
        fixture.create_dir_all("codex/archived_sessions");

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
        fixture.create_dir_all("codex/sessions");

        let paths = codex_usage_paths_from_homes(vec![home.clone(), home]);

        assert_eq!(paths, vec![fixture.path("codex/sessions")]);
    }

    #[test]
    fn keeps_active_session_file_when_archived_file_has_same_relative_path() {
        let fixture = Fixture::new();
        fixture.create_dir_all("codex/sessions");
        fixture.create_dir_all("codex/archived_sessions");
        fixture.write_file("codex/sessions/session.jsonl", "");
        fixture.write_file("codex/archived_sessions/session.jsonl", "");
        fixture.write_file("codex/archived_sessions/archive-only.jsonl", "");

        let sources = codex_usage_sources_from_homes(vec![fixture.path("codex")]);
        let groups = collect_deduped_codex_usage_files(&sources, &Diagnostics::new("codex"));

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("codex/sessions"),
                        fixture.path("codex")
                    ),
                    files: vec![fixture.path("codex/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("codex/archived_sessions"),
                        fixture.path("codex")
                    ),
                    files: vec![fixture.path("codex/archived_sessions/archive-only.jsonl")],
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn records_a_session_subdirectory_it_cannot_list() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let kept = fixture.write_file("codex/sessions/2026/01/01/kept.jsonl", "");
        let blocked = fixture.create_dir_all("codex/sessions/2026/01/02");
        fixture.write_file("codex/sessions/2026/01/02/hidden.jsonl", "");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let diagnostics = Diagnostics::new("codex");

        let sources = codex_usage_sources_from_homes(vec![fixture.path("codex")]);
        let groups = collect_deduped_codex_usage_files(&sources, &diagnostics);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            groups,
            vec![CodexUsageFileGroup {
                source: CodexUsageSource::new(
                    fixture.path("codex/sessions"),
                    fixture.path("codex")
                ),
                files: vec![kept],
            }]
        );
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn deduplicates_rollouts_across_encodings_and_session_directories() {
        let fixture = Fixture::new();
        fixture.write_file("codex/sessions/both.jsonl", "");
        fixture.write_file("codex/sessions/both.jsonl.zst", "");
        fixture.write_file("codex/sessions/compressed.jsonl.zst", "");
        fixture.write_file("codex/sessions/other.zst", "");
        fixture.write_file("codex/archived_sessions/compressed.jsonl", "");
        fixture.write_file("codex/archived_sessions/both.jsonl.zst", "");
        fixture.write_file("codex/archived_sessions/archived.jsonl.zst", "");

        let sources = codex_usage_sources_from_homes(vec![fixture.path("codex")]);
        let groups = collect_deduped_codex_usage_files(&sources, &Diagnostics::new("codex"));

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("codex/sessions"),
                        fixture.path("codex")
                    ),
                    files: vec![
                        fixture.path("codex/sessions/both.jsonl"),
                        fixture.path("codex/sessions/compressed.jsonl.zst"),
                    ],
                },
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("codex/archived_sessions"),
                        fixture.path("codex")
                    ),
                    files: vec![fixture.path("codex/archived_sessions/archived.jsonl.zst")],
                },
            ]
        );
    }

    #[test]
    fn keeps_same_relative_session_file_across_different_homes() {
        let fixture = Fixture::new();
        fixture.create_dir_all("work/sessions");
        fixture.create_dir_all("personal/sessions");
        fixture.write_file("work/sessions/session.jsonl", "");
        fixture.write_file("personal/sessions/session.jsonl", "");

        let sources =
            codex_usage_sources_from_homes(vec![fixture.path("work"), fixture.path("personal")]);
        let groups = collect_deduped_codex_usage_files(&sources, &Diagnostics::new("codex"));

        assert_eq!(
            groups,
            vec![
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("work/sessions"),
                        fixture.path("work")
                    ),
                    files: vec![fixture.path("work/sessions/session.jsonl")],
                },
                CodexUsageFileGroup {
                    source: CodexUsageSource::new(
                        fixture.path("personal/sessions"),
                        fixture.path("personal")
                    ),
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
        let source =
            CodexUsageSource::new(PathBuf::from("/codex/sessions"), PathBuf::from("/codex"));

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
        crate::test_support::set_file_modified(
            &historical,
            crate::parse_ts_timestamp("2025-01-01T00:00:00.000Z").unwrap(),
        );
        crate::test_support::set_file_modified(
            &resumed,
            crate::parse_ts_timestamp("2026-03-15T12:00:00.000Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260315".parse().unwrap()),
            until: Some("20260315".parse().unwrap()),
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
    fn keeps_an_undated_rollout_modified_on_or_after_since() {
        let fixture = Fixture::new();
        let archived_dir = fixture.create_dir_all("codex/archived_sessions");
        let file = fixture.write_file("codex/archived_sessions/custom.jsonl", "");
        crate::test_support::set_file_modified(
            &file,
            crate::parse_ts_timestamp("2026-01-31T00:00:00.000Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260131".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        assert_eq!(
            filter_codex_usage_files(&archived_dir, std::slice::from_ref(&file), &shared),
            vec![file]
        );
    }

    #[test]
    fn skips_an_undated_rollout_last_modified_before_since() {
        let fixture = Fixture::new();
        let archived_dir = fixture.create_dir_all("codex/archived_sessions");
        let file = fixture.write_file("codex/archived_sessions/custom.jsonl", "");
        crate::test_support::set_file_modified(
            &file,
            crate::parse_ts_timestamp("2026-01-30T23:59:59.999Z").unwrap(),
        );
        let shared = SharedArgs {
            since: Some("20260131".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        assert_eq!(
            filter_codex_usage_files(&archived_dir, std::slice::from_ref(&file), &shared),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn keeps_an_undated_rollout_whose_mtime_cannot_be_read() {
        let fixture = Fixture::new();
        let archived_dir = fixture.create_dir_all("codex/archived_sessions");
        let missing = fixture.path("codex/archived_sessions/missing.jsonl");
        let shared = SharedArgs {
            since: Some("20260131".parse().unwrap()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filter_codex_usage_files(&archived_dir, std::slice::from_ref(&missing), &shared),
            vec![missing]
        );
    }
}
