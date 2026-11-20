use std::{
    collections::VecDeque,
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use ccusage_core::{
    Diagnostics, Skip, SkipKind, cli::home::path_identity, fast::FxHashSet,
    parallel::spawn_scoped_worker,
};

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
    walk.visit_links_after_ordinary_tree()
}

const PARALLEL_WALKERS: usize = 4;

/// Visits the files `visit_usage_files` finds, walking top-level subdirectories on several threads.
/// Files arrive in no particular order; symlinks still resolve in sequential walk order.
pub fn visit_usage_files_in_parallel(
    dir: &Path,
    visit: &(dyn Fn(PathBuf) + Sync),
    diagnostics: &Diagnostics,
) {
    let matches = &|path: &Path| path.extension().is_some_and(|ext| ext == "jsonl");
    let mut emit = |path| {
        visit(path);
        ControlFlow::Continue(())
    };
    let mut walk = FileWalk {
        matches,
        visit: &mut emit,
        diagnostics,
        seen: FxHashSet::default(),
        links_after_ordinary_tree: VecDeque::new(),
    };
    let identity = path_identity(dir);
    walk.seen.insert(identity.clone());
    let mut subdirectories = Vec::new();
    let mut links = Vec::new();
    for (path, file_type) in dir_entries(dir, diagnostics) {
        if file_type.is_symlink() {
            links.push(Link::Path(path));
        } else if walk.is_candidate(&path, file_type)
            && let Some(name) = path.file_name()
        {
            let identity = identity.join(name);
            if file_type.is_dir() {
                links.push(Link::Subdirectory(subdirectories.len()));
                subdirectories.push((path, identity));
            } else {
                let _ = walk.visit_entry(path, file_type, identity);
            }
        }
    }
    let mut walked = walk_subdirectories_in_parallel(&subdirectories, matches, visit, diagnostics);
    for link in links {
        match link {
            Link::Path(path) => walk.links_after_ordinary_tree.push_back(path),
            Link::Subdirectory(index) => walk
                .links_after_ordinary_tree
                .append(&mut walked[index].links_after_ordinary_tree),
        }
    }
    if walk.links_after_ordinary_tree.is_empty() {
        return;
    }
    for tree in walked {
        walk.seen.extend(tree.seen);
    }
    let _ = walk.visit_links_after_ordinary_tree();
}

enum Link {
    Path(PathBuf),
    Subdirectory(usize),
}

struct WalkedTree {
    seen: FxHashSet<PathBuf>,
    links_after_ordinary_tree: VecDeque<PathBuf>,
}

fn walk_subdirectories_in_parallel(
    subdirectories: &[(PathBuf, PathBuf)],
    matches: &(dyn Fn(&Path) -> bool + Sync),
    visit: &(dyn Fn(PathBuf) + Sync),
    diagnostics: &Diagnostics,
) -> Vec<WalkedTree> {
    let next = AtomicUsize::new(0);
    let walk_next = || {
        let mut walked = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some((path, identity)) = subdirectories.get(index) else {
                return walked;
            };
            let mut emit = |path| {
                visit(path);
                ControlFlow::Continue(())
            };
            let mut walk = FileWalk {
                matches,
                visit: &mut emit,
                diagnostics,
                seen: FxHashSet::default(),
                links_after_ordinary_tree: VecDeque::new(),
            };
            let _ = walk.visit_directory(path, identity.clone());
            walked.push((
                index,
                WalkedTree {
                    seen: walk.seen,
                    links_after_ordinary_tree: walk.links_after_ordinary_tree,
                },
            ));
        }
    };
    let walkers = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(PARALLEL_WALKERS)
        .min(subdirectories.len());
    let mut walked = thread::scope(|scope| {
        let helpers = (1..walkers)
            .map(|_| spawn_scoped_worker(scope, walk_next))
            .collect::<Vec<_>>();
        let mut walked = walk_next();
        for helper in helpers {
            walked.extend(
                helper
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            );
        }
        walked
    });
    walked.sort_unstable_by_key(|(index, _)| *index);
    walked.into_iter().map(|(_, tree)| tree).collect()
}

struct FileWalk<'a> {
    matches: &'a dyn Fn(&Path) -> bool,
    visit: &'a mut dyn FnMut(PathBuf) -> ControlFlow<()>,
    diagnostics: &'a Diagnostics,
    seen: FxHashSet<PathBuf>,
    links_after_ordinary_tree: VecDeque<PathBuf>,
}

impl FileWalk<'_> {
    fn visit_links_after_ordinary_tree(&mut self) -> ControlFlow<()> {
        while let Some(path) = self.links_after_ordinary_tree.pop_front() {
            self.visit_symlink(path)?;
        }
        ControlFlow::Continue(())
    }

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

#[cfg(test)]
mod tests;
