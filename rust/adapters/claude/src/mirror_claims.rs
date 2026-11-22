use std::{
    collections::HashSet,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use ccusage_adapter_common::{claims::NativeSessions, dir_entries, read_discovered_paths};
use ccusage_core::{Diagnostics, ReportWindow, Result, cli::SharedArgs};

use crate::{
    MainThreadSessions, claude_paths, containing_projects_root,
    daily::{ClaimEntry, find_mirror_sessions, has_usage, read_claim_entries},
    dedupe::{DedupEntry, dedupe_files},
    main_thread_sessions,
    paths::{extract_project, is_project_path_segment, select_usage_files},
    projects_roots, read_usage_files_since, retain_copied_entries, visit_usage_files,
};

type Files<E> = Vec<(PathBuf, Vec<E>)>;

pub(super) struct ClaimingFiles<E> {
    pub(super) report: Files<E>,
    pub(super) pruned_usage: bool,
    pub(super) sessions: MainThreadSessions,
}

pub(super) fn read_claiming_files<E: DedupEntry + Send>(
    shared: &SharedArgs,
    project: &str,
    diagnostics: &Diagnostics,
    read_report: impl Fn(&Path, Option<&Path>) -> Vec<E> + Sync,
) -> Result<ClaimingFiles<E>> {
    let paths = claude_paths()?;
    let roots = projects_roots(&paths);
    let mut files = Vec::new();
    visit_usage_files(&paths, None, &mut |file| files.push(file), diagnostics);
    let report_paths = files
        .iter()
        .filter(|file| extract_project(file, containing_projects_root(file, &roots)) == project)
        .cloned()
        .collect();
    let (report_paths, pruned) = select_usage_files(report_paths, shared, &roots);
    let (claim_paths, _) = select_usage_files(files, shared, &roots);
    let report_paths: HashSet<_> = report_paths.into_iter().collect();
    let claim_paths: HashSet<_> = claim_paths.into_iter().collect();
    let loaded = read_discovered_paths(
        shared.single_thread,
        |visit| report_paths.union(&claim_paths).cloned().for_each(visit),
        |file| {
            let root = containing_projects_root(file, &roots);
            if report_paths.contains(file) {
                let entries = read_report(file, root);
                let claims = claim_paths
                    .contains(file)
                    .then(|| entries.iter().map(ClaimEntry::from_entry).collect());
                (Some(entries), claims)
            } else {
                (None, Some(read_claim_entries(file, root, diagnostics)))
            }
        },
    );
    let mut report = Vec::new();
    let mut claims = Vec::new();
    let mut pruned_usage = false;
    let has_report_usage = loaded
        .iter()
        .any(|(_, (entries, _))| entries.as_ref().is_some_and(|entries| !entries.is_empty()));
    let pruned: HashSet<_> = pruned.into_iter().collect();
    for (file, (entries, claim_entries)) in loaded {
        if let Some(entries) = entries {
            report.push((file.clone(), entries));
        }
        if let Some(entries) = claim_entries {
            pruned_usage |= !has_report_usage && pruned.contains(&file) && !entries.is_empty();
            claims.push((file, entries));
        }
    }
    if !has_report_usage && !pruned_usage {
        pruned_usage = pruned
            .iter()
            .any(|file| !claim_paths.contains(file) && has_usage(file));
    }
    Ok(ClaimingFiles {
        report,
        pruned_usage,
        sessions: windowed_sessions(claims, shared),
    })
}

pub(super) fn scan_windowed_sessions(
    shared: &SharedArgs,
    diagnostics: &Diagnostics,
) -> Result<MainThreadSessions> {
    let paths = claude_paths()?;
    let roots = projects_roots(&paths);
    let read = |file: &PathBuf| {
        read_claim_entries(file, containing_projects_root(file, &roots), diagnostics)
    };
    let files = if shared.since.is_some() {
        read_usage_files_since(
            &paths,
            None,
            diagnostics,
            shared,
            shared.clock.now().into(),
            read,
        )
        .kept
    } else {
        read_discovered_paths(
            shared.single_thread,
            |visit| visit_usage_files(&paths, None, visit, diagnostics),
            read,
        )
    };
    Ok(windowed_sessions(files, shared))
}

fn windowed_sessions(files: Files<ClaimEntry>, shared: &SharedArgs) -> MainThreadSessions {
    let (mut entries, mut copies) = dedupe_files(files);
    let window = ReportWindow::from_shared(shared);
    retain_copied_entries(&mut entries, &mut copies, &window, |entry| {
        crate::date_at(entry.timestamp(), window.timezone())
    });
    main_thread_sessions(&entries, &copies)
}

pub(super) fn find_unbounded_sessions(
    selected_project: Option<&str>,
    pending: &mut NativeSessions,
    found: &mut MainThreadSessions,
    diagnostics: &Diagnostics,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let paths = claude_paths()?;
    let roots = projects_roots(&paths);
    let mut visited = HashSet::new();
    let names: Vec<_> = pending
        .iter()
        .filter(|id| is_project_path_segment(id))
        .map(|id| format!("{id}.jsonl"))
        .filter(|name| Path::new(name).file_name() == Some(OsStr::new(name)))
        .collect();

    // NOTE(2026-11-22, #2498): An unbounded scan only needs one admitted main-thread record for an ID. Claude deduplication retains it as either a survivor or an alias, so try ordinary transcript filenames first.
    for root in &roots {
        for (project, kind) in dir_entries(root, diagnostics) {
            if !(kind.is_dir() || kind.is_symlink())
                || selected_project
                    .is_some_and(|selected| project.file_name() == Some(OsStr::new(selected)))
            {
                continue;
            }
            for name in &names {
                let file = project.join(name);
                if file.is_file() && visited.insert(file.clone()) {
                    find_mirror_sessions(
                        &file,
                        containing_projects_root(&file, &roots),
                        diagnostics,
                        pending,
                        found,
                    );
                    if pending.is_empty() {
                        return Ok(());
                    }
                }
            }
        }
    }

    visit_usage_files(
        &paths,
        None,
        &mut |file| {
            if pending.is_empty() || visited.contains(&file) {
                return;
            }
            let root = containing_projects_root(&file, &roots);
            if selected_project.is_none_or(|selected| extract_project(&file, root) != selected) {
                find_mirror_sessions(&file, root, diagnostics, pending, found);
            }
        },
        diagnostics,
    );
    Ok(())
}
