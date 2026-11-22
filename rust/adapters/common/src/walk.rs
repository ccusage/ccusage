use std::{
    collections::VecDeque,
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use ccusage_core::{Diagnostics, Skip, SkipKind, cli::home::path_identity, fast::FxHashSet};

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
    entries.sort_unstable_by(|left, right| left.0.as_os_str().cmp(right.0.as_os_str()));
    entries
}

#[cfg(test)]
mod tests;
