use std::{
    collections::VecDeque,
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    thread,
};

use ccusage_core::{
    Diagnostics, LoadedEntry, ReportWindow, Skip, SkipKind,
    cli::{SharedArgs, home::path_identity},
    fast::FxHashSet,
    parallel::spawn_scoped_worker,
};

mod discovery;

pub use discovery::{DiscoverFiles, DiscoveredFile, read_discovered_files, read_discovered_paths};

pub mod claims;
pub mod dedupe;
pub mod jsonl;
pub(crate) mod pipeline;
pub mod report;
pub mod roots;
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

#[cfg(any(test, feature = "testing"))]
pub use pipeline::parity;
pub use units::{SqliteStage, UnitError, read_units, read_units_flat};

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

pub fn visit_files_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    visit: &mut dyn FnMut(PathBuf) -> ControlFlow<()>,
    diagnostics: &Diagnostics,
) -> ControlFlow<()> {
    let mut walk = FileWalk {
        matches,
        visit,
        diagnostics,
        seen: FxHashSet::default(),
        links_after_ordinary_tree: VecDeque::new(),
    };
    walk.visit_directory(dir, path_identity(dir))?;
    while let Some(path) = walk.links_after_ordinary_tree.pop_front() {
        walk.visit_symlink(path)?;
    }
    ControlFlow::Continue(())
}

struct FileWalk<'a> {
    matches: &'a dyn Fn(&Path) -> bool,
    visit: &'a mut dyn FnMut(PathBuf) -> ControlFlow<()>,
    diagnostics: &'a Diagnostics,
    seen: FxHashSet<PathBuf>,
    links_after_ordinary_tree: VecDeque<PathBuf>,
}

impl FileWalk<'_> {
    fn visit_directory(&mut self, dir: &Path, identity: PathBuf) -> ControlFlow<()> {
        if !self.seen.insert(identity.clone()) {
            return ControlFlow::Continue(());
        }
        for (path, file_type) in dir_entries(dir, self.diagnostics) {
            if file_type.is_symlink() {
                self.links_after_ordinary_tree.push_back(path);
            } else if self.is_candidate(&path, file_type)
                && let Some(name) = path.file_name()
            {
                let identity = identity.join(name);
                self.visit_entry(path, file_type, identity)?;
            }
        }
        ControlFlow::Continue(())
    }

    fn visit_symlink(&mut self, path: PathBuf) -> ControlFlow<()> {
        let Ok(metadata) = fs::metadata(&path) else {
            self.diagnostics.record(Skip::unit(
                &path,
                SkipKind::Unreadable,
                "cannot read file type",
            ));
            return ControlFlow::Continue(());
        };
        let file_type = metadata.file_type();
        if !self.is_candidate(&path, file_type) {
            return ControlFlow::Continue(());
        }

        let identity = path_identity(&path);
        if identity == path {
            self.diagnostics.record(Skip::unit(
                &path,
                SkipKind::Unreadable,
                "cannot resolve symlink",
            ));
            return ControlFlow::Continue(());
        }
        self.visit_entry(path, file_type, identity)
    }

    fn is_candidate(&self, path: &Path, file_type: fs::FileType) -> bool {
        file_type.is_dir() || (file_type.is_file() && (self.matches)(path))
    }

    fn visit_entry(
        &mut self,
        path: PathBuf,
        file_type: fs::FileType,
        identity: PathBuf,
    ) -> ControlFlow<()> {
        if file_type.is_dir() {
            self.visit_directory(&path, identity)
        } else if self.seen.insert(identity) {
            (self.visit)(path)
        } else {
            ControlFlow::Continue(())
        }
    }
}

pub fn any_file_matching(
    dir: &Path,
    matches: &dyn Fn(&Path) -> bool,
    diagnostics: &Diagnostics,
) -> bool {
    visit_files_matching(dir, matches, &mut |_| ControlFlow::Break(()), diagnostics).is_break()
}

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
                spawn_scoped_worker(scope, move || {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ccusage_test_support::Fixture;

    use super::{
        Split, chunk_file_indexes_by_size, collect_files_matching, map_parallel, worker_count,
    };

    #[test]
    fn discovery_is_sorted_and_stops_at_the_first_match() {
        let fixture = Fixture::new();
        fixture.write_file("z.jsonl", "");
        let first = fixture.write_file("a/first.jsonl", "");
        fixture.write_file("a/second.jsonl", "");
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
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
            fixture.write_file(name, "");
        }
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        let mut files = Vec::new();
        super::collect_files_with_extension(fixture.root(), "jsonl", &mut files, &diagnostics);
        assert_eq!(
            files,
            ["a.jsonl", "b/a.jsonl", "b/z.jsonl", "z.jsonl"].map(|name| fixture.path(name))
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_follows_symlinked_files() {
        let fixture = Fixture::new();
        let target = fixture.write_file("transcript", "");
        let root = fixture.create_dir_all("root");
        let alias = root.join("session.jsonl");
        std::os::unix::fs::symlink(target, &alias).unwrap();

        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

        assert_eq!(files, [alias]);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_follows_symlinked_directories() {
        let fixture = Fixture::new();
        fixture.write_file("project/nested/session.jsonl", "");
        let root = fixture.create_dir_all("root");
        let alias = root.join("linked-project");
        std::os::unix::fs::symlink(fixture.path("project"), &alias).unwrap();

        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

        assert_eq!(files, [alias.join("nested/session.jsonl")]);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_reads_multiple_links_to_the_same_target_once() {
        let fixture = Fixture::new();
        let target = fixture.write_file("transcript.jsonl", "usage");
        let root = fixture.create_dir_all("root");
        let first = root.join("a.jsonl");
        std::os::unix::fs::symlink(&target, &first).unwrap();
        std::os::unix::fs::symlink(&target, root.join("b.jsonl")).unwrap();

        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        let reads = super::read_discovered_paths(
            true,
            |visit| super::visit_usage_files(&root, visit, &diagnostics),
            |path| std::fs::read_to_string(path).unwrap(),
        );

        assert_eq!(reads, [(first, "usage".to_owned())]);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_prefers_the_original_path_over_its_aliases() {
        let fixture = Fixture::new();
        let root = fixture.create_dir_all("root");
        let target = fixture.write_file("root/b/session.jsonl", "");
        let linked = root.join("d.jsonl");
        std::os::unix::fs::symlink(root.join("b"), root.join("a")).unwrap();
        std::os::unix::fs::symlink(&target, root.join("c.jsonl")).unwrap();
        std::os::unix::fs::symlink(fixture.write_file("external.jsonl", ""), &linked).unwrap();

        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

        assert_eq!(files, [target, linked]);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_stops_at_symlink_cycles() {
        let fixture = Fixture::new();
        let root = fixture.create_dir_all("root");
        let original = fixture.write_file("root/a.jsonl", "");
        fixture.write_file("project/b.jsonl", "");
        let alias = root.join("linked-project");
        std::os::unix::fs::symlink(fixture.path("project"), &alias).unwrap();
        std::os::unix::fs::symlink(&root, fixture.path("project/back")).unwrap();

        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

        assert_eq!(files, [original, alias.join("b.jsonl")]);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_reports_dangling_symlinks_as_unreadable() {
        let fixture = Fixture::new();
        let original = fixture.write_file("a.jsonl", "");
        std::os::unix::fs::symlink(fixture.path("missing"), fixture.path("b.jsonl")).unwrap();

        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());
        super::collect_files_with_extension(fixture.root(), "jsonl", &mut files, &diagnostics);

        assert_eq!(files, [original]);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 1);
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
        fixture.write_file("a/skipped.txt", "");
        let mut files = Vec::new();
        let diagnostics = ccusage_core::Diagnostics::at("test", ccusage_core::utc_now());

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
        let diagnostics = Diagnostics::at("test", ccusage_core::utc_now());
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
        let read = |path: &PathBuf| {
            let content = std::fs::read_to_string(path).unwrap();
            content.split(':').next().unwrap().to_string()
        };

        let single = map_parallel(&files, true, Split::BySize(PathBuf::as_path), read);
        let multi = map_parallel(&files, false, Split::BySize(PathBuf::as_path), read);
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
        let read = |path: &PathBuf| std::fs::read_to_string(path).unwrap();

        for single_thread in [true, false] {
            let mut results = super::read_discovered_paths(
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
        assert!(super::read_discovered_paths(false, |_| {}, read).is_empty());
    }

    #[test]
    fn reads_discovered_work_before_discovery_returns() {
        let (started, observed) = std::sync::mpsc::sync_channel(1);
        let results = super::read_discovered_files(
            false,
            &mut |emit| {
                emit(7, PathBuf::from("rollout.jsonl"));
                assert_eq!(
                    observed
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap(),
                    7
                );
            },
            |file| {
                started.send(file.group).unwrap();
                file.group * 2
            },
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.index, 0);
        assert_eq!(results[0].1, 14);
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

        assert!(map_parallel(&empty, false, Split::BySize(PathBuf::as_path), |_| 0_u8).is_empty());
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
