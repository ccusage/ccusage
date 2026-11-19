use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
};

use ccusage_core::{Diagnostics, LoadedEntry, ReportWindow, Skip, SkipKind, cli::SharedArgs};

pub mod claims;
pub mod jsonl;
#[cfg(any(test, feature = "testing"))]
pub mod parity;
pub(crate) mod pipeline;
pub mod report;
pub mod rows;
pub mod source;
#[cfg(test)]
mod test_source;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod units;

pub use report::print_table_for_agent;
pub use units::{SqliteStage, UnitError, read_units, read_units_flat};

/// Collects the files under `dir` whose extension is `extension`, like
/// [`collect_files_matching`].
pub fn collect_files_with_extension(
    dir: &Path,
    extension: &str,
    files: &mut Vec<PathBuf>,
    diagnostics: &Diagnostics,
) {
    collect_files_matching(
        dir,
        &|path| path.extension().is_some_and(|ext| ext == extension),
        files,
        diagnostics,
    );
}

/// Collects the files under `dir` that `matches` accepts, recursing into
/// subdirectories without following symlinks. What cannot be read is
/// recorded like [`dir_entries`] does.
pub fn collect_files_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    files: &mut Vec<PathBuf>,
    diagnostics: &Diagnostics,
) {
    visit_files_matching(dir, matches, &mut |path| files.push(path), diagnostics);
}

/// Hands each `.jsonl` file under `dir` to `visit` as the walk finds it.
/// It uses the same traversal and diagnostics as [`collect_files_matching`].
pub fn visit_usage_files(dir: &Path, visit: &mut dyn FnMut(PathBuf), diagnostics: &Diagnostics) {
    visit_files_matching(
        dir,
        &|path| path.extension().is_some_and(|ext| ext == "jsonl"),
        visit,
        diagnostics,
    );
}

/// Like [`collect_files_matching`], but hands each file to `visit` as the walk
/// finds it, so reading can start before the walk ends.
fn visit_files_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    visit: &mut dyn FnMut(PathBuf),
    diagnostics: &Diagnostics,
) {
    for (path, file_type) in dir_entries(dir, diagnostics) {
        if file_type.is_file() && matches(&path) {
            visit(path);
        } else if file_type.is_dir() {
            visit_files_matching(&path, matches, visit, diagnostics);
        }
    }
}

/// The entries directly inside `dir`, each with its own (not a symlink
/// target's) file type, in the order the directory lists them.
///
/// A missing `dir` holds no entries. A directory, entry or file type that
/// cannot be read is recorded in `diagnostics`, so files it hides show up as
/// a warning rather than as lower usage.
pub fn dir_entries(dir: &Path, diagnostics: &Diagnostics) -> Vec<(PathBuf, fs::FileType)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                diagnostics.record(Skip::unit(
                    dir,
                    SkipKind::Unreadable,
                    "cannot read directory",
                ));
            }
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| {
            let Ok(entry) = entry else {
                diagnostics.record(Skip::unit(
                    dir,
                    SkipKind::ReadInterrupted,
                    "cannot read directory entry",
                ));
                return None;
            };
            let path = entry.path();
            match entry.file_type() {
                Ok(file_type) => Some((path, file_type)),
                Err(_) => {
                    diagnostics.record(Skip::unit(
                        &path,
                        SkipKind::Unreadable,
                        "cannot read file type",
                    ));
                    None
                }
            }
        })
        .collect()
}

pub fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    pipeline::retain_entries_in(entries, &ReportWindow::from_shared(shared));
}

/// Keeps the sessions whose last activity falls on a day inside the
/// `--since`/`--until` window, with the day taken in the report timezone the
/// same way `LoadedEntry::date` is.
#[cfg(test)]
fn filter_session_summaries_by_last_activity(
    rows: &mut Vec<ccusage_core::UsageSummary>,
    shared: &SharedArgs,
) {
    pipeline::retain_sessions_last_active_in(rows, &ReportWindow::from_shared(shared));
}

pub fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
    // Callers derive the count from available_parallelism, which can be 0 for a
    // caller that clamps against an empty file list; one chunk still returns
    // every index rather than indexing an empty vector.
    let chunk_count = chunk_count.max(1);
    let mut weighted_indexes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let size = fs::metadata(file).map_or(0, |metadata| metadata.len());
        weighted_indexes.push((index, size));
    }
    weighted_indexes.sort_unstable_by(|a, b| match b.1.cmp(&a.1) {
        std::cmp::Ordering::Equal => a.0.cmp(&b.0),
        order => order,
    });

    let mut chunks = vec![Vec::new(); chunk_count];
    let mut chunk_sizes = vec![0_u64; chunk_count];
    for (index, size) in weighted_indexes {
        let mut target = 0;
        for candidate in 1..chunk_sizes.len() {
            if chunk_sizes[candidate] < chunk_sizes[target] {
                target = candidate;
            }
        }
        chunks[target].push(index);
        chunk_sizes[target] = chunk_sizes[target].saturating_add(size);
    }

    chunks
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

/// Reads `files` by applying `read` to each path and returns results in file order.
pub(crate) fn read_files_parallel<T, F>(files: &[PathBuf], single_thread: bool, read: F) -> Vec<T>
where
    T: Send,
    F: Fn(&Path) -> T + Sync,
{
    let worker_count = if single_thread {
        1
    } else {
        thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(files.len())
    };
    if worker_count <= 1 {
        return files.iter().map(|file| read(file.as_path())).collect();
    }

    let chunks = chunk_file_indexes_by_size(files, worker_count);
    let read = &read;
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|index| (index, read(files[index].as_path())))
                    .collect::<Vec<_>>()
            }));
        }
        let mut results: Vec<Option<T>> = Vec::with_capacity(files.len());
        results.resize_with(files.len(), || None);
        for (index, value) in handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("file read worker panicked"))
        {
            results[index] = Some(value);
        }
        results
            .into_iter()
            .map(|value| value.expect("file read worker returned every file"))
            .collect()
    })
}

/// Reads files as `discover` yields them and returns each with its result,
/// in no particular order.
///
/// Workers take the next file as soon as they are free, so reading overlaps a
/// directory walk instead of waiting for its full list, and no file sizes are
/// needed to balance the work. A worker starts with each of the first files,
/// so a discovery that finds few files spawns no more threads than it needs.
pub fn read_discovered_files<T, F>(
    single_thread: bool,
    discover: impl FnOnce(&mut dyn FnMut(PathBuf)),
    read: F,
) -> Vec<(PathBuf, T)>
where
    T: Send,
    F: Fn(&Path) -> T + Sync,
{
    if single_thread {
        let mut results = Vec::new();
        discover(&mut |path| {
            let result = read(&path);
            results.push((path, result));
        });
        return results;
    }
    let worker_count = thread::available_parallelism().map_or(1, usize::from);
    let (sender, receiver) = mpsc::channel::<PathBuf>();
    let receiver = Mutex::new(receiver);
    let (read, receiver) = (&read, &receiver);
    thread::scope(|scope| {
        let mut workers = Vec::new();
        discover(&mut |path| {
            if workers.len() < worker_count {
                workers.push(scope.spawn(move || {
                    let mut results = Vec::new();
                    loop {
                        let next = receiver
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .recv();
                        let Ok(path) = next else {
                            return results;
                        };
                        let result = read(&path);
                        results.push((path, result));
                    }
                }));
            }
            let _ = sender.send(path);
        });
        drop(sender);
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("file read worker panicked"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use ccusage_core::{UsageSummary, cli::SharedArgs};
    use ccusage_test_support::Fixture;

    use super::{
        chunk_file_indexes_by_size, collect_files_matching,
        filter_session_summaries_by_last_activity, read_files_parallel,
    };

    fn session_summary(session_id: &str, last_activity: &str) -> UsageSummary {
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: Some(session_id.to_string()),
            project_path: None,
            last_activity: Some(last_activity.to_string()),
            first_activity: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: Vec::new(),
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }

    fn filtered_session_ids(shared: &SharedArgs) -> Vec<String> {
        let mut rows = vec![
            session_summary("before", "2026-02-22T23:59:59.000Z"),
            session_summary("start-of-day", "2026-02-23T00:00:00.000Z"),
            session_summary("end-of-day", "2026-02-23T23:59:59.999Z"),
            session_summary("after", "2026-02-24T00:00:00.000Z"),
        ];
        filter_session_summaries_by_last_activity(&mut rows, shared);
        rows.into_iter().filter_map(|row| row.session_id).collect()
    }

    #[test]
    fn session_window_bounds_are_inclusive_of_the_whole_day() {
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            until: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_takes_the_day_in_the_report_timezone() {
        // Asia/Tokyo is UTC+9, so the window shifts nine hours earlier in UTC.
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            until: Some("20260223".to_string()),
            timezone: Some("Asia/Tokyo".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(filtered_session_ids(&shared), ["before", "start-of-day"]);
    }

    #[test]
    fn session_window_with_only_until_keeps_everything_through_that_day() {
        let shared = SharedArgs {
            until: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["before", "start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_with_only_since_keeps_everything_from_that_day() {
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day", "after"]
        );
    }

    #[test]
    fn session_window_drops_rows_without_a_parseable_last_activity() {
        let missing = UsageSummary {
            last_activity: None,
            ..session_summary("missing", "")
        };
        let unparsable = session_summary("unparsable", "2026-02-23");
        for shared in [
            SharedArgs {
                since: Some("20260101".to_string()),
                ..SharedArgs::default()
            },
            SharedArgs {
                until: Some("20991231".to_string()),
                ..SharedArgs::default()
            },
        ] {
            let mut rows = vec![missing.clone(), unparsable.clone()];
            filter_session_summaries_by_last_activity(&mut rows, &shared);
            assert!(rows.is_empty(), "{:?}/{:?}", shared.since, shared.until);
        }
    }

    #[test]
    fn collects_nested_files_accepted_by_the_predicate() {
        let fixture = Fixture::new();
        let kept = fixture.write_file("a/b/kept.jsonl", "");
        let _ = fixture.write_file("a/skipped.txt", "");
        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::new("test");

        collect_files_matching(
            fixture.root(),
            &|path| path.extension().is_some_and(|ext| ext == "jsonl"),
            &mut files,
            &diagnostics,
        );

        assert_eq!(files, vec![kept]);
    }

    #[test]
    fn discovery_reports_failed_roots_without_losing_healthy_roots() {
        use ccusage_core::{Diagnostics, SkipKind};

        let fixture = Fixture::new();
        let failed = fixture.write_file("not-a-directory", "");
        let healthy = fixture.write_file("healthy/nested/session.jsonl", "");
        let diagnostics = Diagnostics::new("test");
        let mut files = Vec::new();
        for root in [fixture.path("missing"), failed, fixture.path("healthy")] {
            super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);
        }

        assert_eq!(files, [healthy]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[test]
    fn preserves_file_order_and_matches_single_thread() {
        let fixture = Fixture::new();
        let files = (0..256)
            .map(|index| {
                let body = "x".repeat((index % 17) * 64 + 1);
                fixture.write_file(format!("file-{index:03}.txt"), format!("{index}:{body}"))
            })
            .collect::<Vec<_>>();
        let read = |path: &std::path::Path| {
            let content = std::fs::read_to_string(path).unwrap();
            content.split(':').next().unwrap().to_string()
        };

        let single = read_files_parallel(&files, true, read);
        let multi = read_files_parallel(&files, false, read);
        let expected = Vec::from_fn(256, |index| index.to_string());

        assert_eq!(single, expected);
        assert_eq!(multi, expected);
    }

    #[test]
    fn reads_every_discovered_file_once() {
        let fixture = Fixture::new();
        let files = (0..64)
            .map(|index| {
                fixture.write_file(
                    format!("d{}/file-{index:02}.txt", index % 5),
                    index.to_string(),
                )
            })
            .collect::<Vec<_>>();
        let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();

        for single_thread in [true, false] {
            let mut results = super::read_discovered_files(
                single_thread,
                |visit| files.iter().cloned().for_each(visit),
                read,
            );
            results.sort();
            let mut expected = files
                .iter()
                .map(|path| (path.clone(), read(path)))
                .collect::<Vec<_>>();
            expected.sort();
            assert_eq!(results, expected, "single_thread={single_thread}");
        }
        assert!(super::read_discovered_files(false, |_| {}, read).is_empty());
    }

    #[test]
    fn treats_a_zero_chunk_count_as_one_chunk() {
        let fixture = Fixture::new();
        let files = vec![fixture.write_file("only.txt", "body")];

        assert_eq!(chunk_file_indexes_by_size(&files, 0), vec![vec![0]]);
    }

    #[test]
    fn handles_empty_input() {
        let empty: Vec<std::path::PathBuf> = Vec::new();

        assert!(read_files_parallel(&empty, false, |_| 0_u8).is_empty());
    }
}
