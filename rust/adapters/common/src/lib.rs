use std::{
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
};

use ccusage_core::{Diagnostics, LoadedEntry, ReportWindow, Skip, SkipKind, cli::SharedArgs};

pub mod claims;
pub mod dedupe;
pub mod jsonl;
#[cfg(any(test, feature = "testing"))]
pub mod parity;
pub mod pipeline;
pub mod report;
pub mod rows;
pub mod source;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(test)]
mod test_source;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod timestamps;
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
    let _ = visit_files_matching(
        dir,
        matches,
        &mut |path| {
            files.push(path);
            ControlFlow::Continue(())
        },
        diagnostics,
    );
}

/// Hands each `.jsonl` file under `dir` to `visit` as the walk finds it, like
/// [`visit_files_matching`].
pub fn visit_usage_files(dir: &Path, visit: &mut dyn FnMut(PathBuf), diagnostics: &Diagnostics) {
    let _ = visit_files_matching(
        dir,
        &|path| path.extension().is_some_and(|ext| ext == "jsonl"),
        &mut |path| {
            visit(path);
            ControlFlow::Continue(())
        },
        diagnostics,
    );
}

/// Like [`collect_files_matching`], but hands each file to `visit` as the walk
/// finds it, so reading can start before the walk ends.
pub fn visit_files_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    visit: &mut dyn FnMut(PathBuf) -> ControlFlow<()>,
    diagnostics: &Diagnostics,
) -> ControlFlow<()> {
    for (path, file_type) in dir_entries(dir, diagnostics) {
        if file_type.is_file() && matches(&path) {
            visit(path)?;
        } else if file_type.is_dir() {
            visit_files_matching(&path, matches, visit, diagnostics)?;
        }
    }
    ControlFlow::Continue(())
}

pub fn any_file_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    diagnostics: &Diagnostics,
) -> bool {
    visit_files_matching(dir, matches, &mut |_| ControlFlow::Break(()), diagnostics).is_break()
}

/// The entries directly inside `dir`, each with its own (not a symlink
/// target's) file type, sorted by path.
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
    let mut entries: Vec<_> = entries
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
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

pub fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    pipeline::retain_entries_in(entries, &ReportWindow::from_shared(shared));
}

pub fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
    // Callers derive the count from available_parallelism, which can be 0 for a
    // caller that clamps against an empty file list; one chunk still returns
    // every index rather than indexing an empty vector.
    let chunk_count = chunk_count.max(1);
    chunk_indexes_by_size(files, PathBuf::as_path, chunk_count)
}

fn chunk_indexes_by_size<I>(
    items: &[I],
    path: fn(&I) -> &Path,
    chunk_count: usize,
) -> Vec<Vec<usize>> {
    let mut weighted_indexes = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let size = fs::metadata(path(item)).map_or(0, |metadata| metadata.len());
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

pub fn worker_count(single_thread: bool, items: usize) -> usize {
    if single_thread || items <= 1 {
        return 1;
    }
    thread::available_parallelism()
        .map_or(1, usize::from)
        .min(items)
}

pub enum Split<I> {
    BySize(fn(&I) -> &Path),
    ByCount,
}

pub fn map_parallel<I, T, F>(items: &[I], single_thread: bool, split: Split<I>, map: F) -> Vec<T>
where
    I: Sync,
    T: Send,
    F: Fn(&I) -> T + Sync,
{
    let workers = worker_count(single_thread, items.len());
    if workers == 1 {
        return items.iter().map(map).collect();
    }
    let chunks = match split {
        Split::BySize(path) => chunk_indexes_by_size(items, path, workers),
        Split::ByCount => {
            let chunk_size = items.len().div_ceil(workers);
            (0..items.len())
                .collect::<Vec<_>>()
                .chunks(chunk_size)
                .map(<[usize]>::to_vec)
                .collect()
        }
    };
    let map = &map;
    let mut results = thread::scope(|scope| {
        chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|index| (index, map(&items[index])))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(join_worker)
            .collect::<Vec<_>>()
    });
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

fn join_worker<T>(handle: thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// Reads `files` by applying `read` to each path and returns results in file order.
pub fn read_files_parallel<T, F>(files: &[PathBuf], single_thread: bool, read: F) -> Vec<T>
where
    T: Send,
    F: Fn(&Path) -> T + Sync,
{
    map_parallel(
        files,
        single_thread,
        Split::BySize(PathBuf::as_path),
        |file| read(file),
    )
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
        workers.into_iter().flat_map(join_worker).collect()
    })
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::Fixture;

    use super::{
        Split, chunk_file_indexes_by_size, collect_files_matching, map_parallel,
        read_files_parallel, worker_count,
    };

    #[test]
    fn discovery_is_sorted_and_stops_at_the_first_match() {
        let fixture = Fixture::new();
        let _ = fixture.write_file("z.jsonl", "");
        let first = fixture.write_file("a/first.jsonl", "");
        let _ = fixture.write_file("a/second.jsonl", "");
        let diagnostics = ccusage_core::Diagnostics::new("test");
        let mut visited = Vec::new();
        assert!(super::any_file_matching(
            fixture.root(),
            &|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            },
            &diagnostics,
        ));
        let result = super::visit_files_matching(
            fixture.root(),
            &|_| true,
            &mut |path| {
                visited.push(path);
                std::ops::ControlFlow::Break(())
            },
            &diagnostics,
        );
        assert_eq!(result, std::ops::ControlFlow::Break(()));
        assert_eq!(visited, [first]);
    }

    #[test]
    fn directory_entries_and_recursive_collection_have_path_order() {
        let fixture = Fixture::new();
        for name in ["z.jsonl", "b/z.jsonl", "b/a.jsonl", "a.jsonl"] {
            let _ = fixture.write_file(name, "");
        }
        let diagnostics = ccusage_core::Diagnostics::new("test");
        let mut files = Vec::new();
        super::collect_files_with_extension(fixture.root(), "jsonl", &mut files, &diagnostics);
        assert_eq!(
            files,
            ["a.jsonl", "b/a.jsonl", "b/z.jsonl", "z.jsonl"].map(|name| fixture.path(name))
        );
    }

    #[test]
    fn balances_file_chunks_by_size() {
        let fixture = ccusage_test_support::fs_fixture!({
            "large-a.jsonl": "x".repeat(100),
            "small-a.jsonl": "x",
            "small-b.jsonl": "x",
            "large-b.jsonl": "x".repeat(100),
        });
        let files = [
            "large-a.jsonl",
            "small-a.jsonl",
            "small-b.jsonl",
            "large-b.jsonl",
        ]
        .into_iter()
        .map(|name| fixture.path(name))
        .collect::<Vec<_>>();

        let chunks = chunk_file_indexes_by_size(&files, 2);
        assert_eq!(chunks.len(), 2);
        let mut indexes = chunks.iter().flatten().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        assert_eq!(indexes, vec![0, 1, 2, 3]);

        let chunk_sizes = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|index| std::fs::metadata(&files[*index]).unwrap().len())
                    .sum::<u64>()
            })
            .collect::<Vec<_>>();
        assert_eq!(chunk_sizes, vec![101, 101]);
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

    #[test]
    fn count_split_keeps_item_order() {
        let items = Vec::from_fn(1000, |index| index);

        for single_thread in [true, false] {
            assert_eq!(
                map_parallel(&items, single_thread, Split::ByCount, |item| item * 2),
                Vec::from_fn(1000, |index| index * 2),
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn worker_count_is_one_for_a_single_thread_or_at_most_one_item() {
        assert_eq!(worker_count(true, 100), 1);
        assert_eq!(worker_count(false, 0), 1);
        assert_eq!(worker_count(false, 1), 1);
        assert!((1..=3).contains(&worker_count(false, 3)));
    }

    #[test]
    #[should_panic(expected = "worker failed")]
    fn a_worker_panic_reaches_the_caller() {
        let items = Vec::from_fn(64, |index| index);

        map_parallel(&items, false, Split::ByCount, |item| {
            assert_ne!(*item, 7, "worker failed");
        });
    }
}
