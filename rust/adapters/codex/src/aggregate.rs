use ccusage_adapter_common::{DiscoverFiles, DiscoveredFile, read_discovered_files};
use ccusage_core::{Diagnostics, PricingMap};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::{
    CodexGroup, CodexTokenUsageEvent, Result,
    cli::{AgentReportKind, SharedArgs},
};

use super::{
    parser, paths,
    replay::{self, CodexReplayFile, CodexReplayPlan, CodexSessionMetadata},
};

mod accumulate;
mod dedupe;
mod intern;

use dedupe::{CodexDedupe, CodexEventProvenance};

struct CodexAggregateRun<'a> {
    source: &'a paths::CodexUsageSource,
    files: &'a [PathBuf],
    shared: &'a SharedArgs,
    replay: &'a CodexReplayPlan,
    file_offset: usize,
    diagnostics: &'a Diagnostics,
}

pub(super) fn load_groups_with_diagnostics(
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    load_groups_from_sources_with_diagnostics(
        &paths::codex_usage_sources()?,
        shared,
        kind,
        pricing,
        diagnostics,
    )
}

pub(super) fn load_groups_from_sources_with_diagnostics(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let source_refs = sources.iter().collect::<Vec<_>>();
    let mut discover = |emit: &mut dyn FnMut(usize, PathBuf)| {
        paths::visit_deduped_codex_usage_files(sources, diagnostics, emit);
    };
    aggregate_discovered(
        &source_refs,
        shared,
        kind,
        pricing,
        diagnostics,
        &mut discover,
    )
}

pub(super) fn select_session_files(
    file_groups: &[paths::CodexUsageFileGroup],
    requested: &str,
    logical_only: bool,
) -> Vec<Vec<PathBuf>> {
    let requested = super::normalize_session_id(requested);
    let identity = |path: &Path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .and_then(super::rollout_identity)
            .map(|(thread, rollout)| (thread == requested, thread != rollout))
    };
    let logical_session = logical_only
        || file_groups
            .iter()
            .flat_map(|group| &group.files)
            .any(|path| identity(path) == Some((true, true)));
    file_groups
        .iter()
        .map(|group| {
            group
                .files
                .iter()
                .filter(|path| {
                    identity(path).is_some_and(|(owned, _)| owned)
                        || (!logical_session
                            && super::session_id_matches(
                                &parser::codex_session_id(group.source.dir(), path),
                                requested,
                            ))
                })
                .cloned()
                .collect()
        })
        .collect()
}

pub(super) fn load_groups_from_file_selection(
    file_groups: &[paths::CodexUsageFileGroup],
    files_by_group: &[Vec<PathBuf>],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let all_files = file_groups
        .iter()
        .map(|group| (group.source.dir(), group.files.as_slice()))
        .collect::<Vec<_>>();
    let selected_files = file_groups
        .iter()
        .zip(files_by_group)
        .map(|(group, files)| (&group.source, files.as_slice()))
        .collect::<Vec<_>>();
    aggregate_file_groups(
        &selected_files,
        &all_files,
        shared,
        kind,
        pricing,
        diagnostics,
    )
}

fn aggregate_file_groups(
    selected: &[(&paths::CodexUsageSource, &[PathBuf])],
    available: &[(&Path, &[PathBuf])],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
) -> Result<BTreeMap<String, CodexGroup>> {
    let filtered = selected
        .iter()
        .zip(available)
        .any(|((_, files), (_, all_files))| files.len() != all_files.len());
    if !filtered && !has_date_bounds(shared) {
        let sources = selected
            .iter()
            .map(|(source, _)| *source)
            .collect::<Vec<_>>();
        return aggregate_discovered(&sources, shared, kind, pricing, diagnostics, &mut |emit| {
            for (group, (_, files)) in selected.iter().enumerate() {
                for file in *files {
                    emit(group, file.clone());
                }
            }
        })
        .map(|(groups, _)| groups);
    }

    let selected_paths = selected
        .iter()
        .map(|(source, files)| (source.dir(), *files))
        .collect::<Vec<_>>();
    let replay = CodexReplayPlan::for_selection(
        &selected_paths,
        available.iter().copied(),
        shared.single_thread,
        diagnostics,
    );
    aggregate_planned(selected, shared, kind, pricing, diagnostics, &replay)
}

fn aggregate_planned(
    selected: &[(&paths::CodexUsageSource, &[PathBuf])],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
    replay: &CodexReplayPlan,
) -> Result<BTreeMap<String, CodexGroup>> {
    let seen = CodexDedupe::new(selected.iter().map(|(_, files)| files.len()).sum(), pricing);
    seen.record_lineage_edges(replay.lineage_edges())?;
    let mut file_offset = 0;
    for &(source, files) in selected {
        let run = CodexAggregateRun {
            source,
            files,
            shared,
            replay,
            file_offset,
            diagnostics,
        };
        aggregate_files_with_dedupe(&run, &seen)?;
        file_offset += files.len();
    }
    seen.fold(shared, kind)
}

fn aggregate_discovered(
    sources: &[&paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: Option<&PricingMap>,
    diagnostics: &Diagnostics,
    discover: &mut DiscoverFiles<'_>,
) -> Result<(BTreeMap<String, CodexGroup>, bool)> {
    let seen = CodexDedupe::new(usize::MAX, pricing);
    let eligibility = paths::CodexFileEligibility::from_shared(shared);
    let mut loaded = read_discovered_files(shared.single_thread, discover, |file| {
        let source = sources[file.group];
        eligibility
            .is_none_or(|filter| filter.contains_file(source.dir(), &file.path))
            .then(|| fold_rollout(&seen, None, source, &file.path, file.index, diagnostics))
            .transpose()
    });
    let detected = !loaded.is_empty();
    let has_forks = loaded.iter().any(|(_, result)| {
        result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(Option::as_ref)
            .is_some_and(CodexSessionMetadata::is_fork)
    });
    if has_forks {
        loaded.sort_unstable_by_key(|(file, _)| file.index);
        let bounded = has_date_bounds(shared);
        let mut available = vec![Vec::new(); sources.len()];
        let mut probes = Vec::new();
        let mut replay_files = Vec::new();
        let mut forks = Vec::new();
        for (file, result) in loaded {
            let metadata = result?;
            let dir = sources[file.group].dir();
            if metadata
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(CodexSessionMetadata::is_fork)
            {
                forks.push(DiscoveredFile {
                    path: file.path.clone(),
                    ..file
                });
            }
            if bounded {
                if let Some(metadata) = metadata {
                    probes.push((file.path.clone(), dir, metadata));
                }
                available[file.group].push(file.path);
            } else if let Some(metadata) = metadata {
                replay_files.push(CodexReplayFile {
                    path: file.path,
                    sessions_dir: dir,
                    metadata: metadata.unwrap_or_default(),
                });
            }
        }
        let replay = if bounded {
            CodexReplayPlan::for_probes(
                probes,
                sources
                    .iter()
                    .map(|source| source.dir())
                    .zip(available.iter().map(Vec::as_slice)),
                shared.single_thread,
                diagnostics,
            )
        } else {
            CodexReplayPlan::new(replay_files, None, shared.single_thread, diagnostics)
        };
        seen.record_lineage_edges(replay.lineage_edges())?;
        ccusage_adapter_common::map_parallel(
            &forks,
            shared.single_thread,
            ccusage_adapter_common::Split::BySize(|file| &file.path),
            |file| {
                fold_rollout(
                    &seen,
                    Some(&replay),
                    sources[file.group],
                    &file.path,
                    file.index,
                    diagnostics,
                )
                .map(|_| ())
            },
        )
        .into_iter()
        .collect::<Result<()>>()?;
    } else {
        let mut excluded = Vec::new();
        let mut has_lineage = false;
        for (file, result) in loaded {
            match result? {
                None => excluded.push((file.path, sources[file.group].dir())),
                Some(metadata) => {
                    has_lineage |= metadata
                        .as_ref()
                        .is_some_and(|metadata| metadata.lineage_parent_id().is_some());
                }
            }
        }
        if has_lineage && !excluded.is_empty() {
            seen.record_lineage_edges(&replay::read_lineage_edges(
                &excluded,
                shared.single_thread,
            ))?;
        }
    }
    seen.fold(shared, kind).map(|groups| (groups, detected))
}

fn aggregate_files_with_dedupe(run: &CodexAggregateRun<'_>, seen: &CodexDedupe<'_>) -> Result<()> {
    let workers = ccusage_adapter_common::worker_count(run.shared.single_thread, run.files.len());
    let chunks = if workers == 1 {
        vec![(0..run.files.len()).collect()]
    } else {
        crate::chunk_file_indexes_by_size(run.files, workers)
    };
    ccusage_adapter_common::map_parallel(
        &chunks,
        run.shared.single_thread,
        ccusage_adapter_common::Split::ByCount,
        |chunk| {
            for &index in chunk {
                fold_rollout(
                    seen,
                    Some(run.replay),
                    run.source,
                    &run.files[index],
                    run.file_offset + index,
                    run.diagnostics,
                )?;
            }
            Ok(())
        },
    )
    .into_iter()
    .collect()
}

fn fold_rollout(
    seen: &CodexDedupe<'_>,
    replay: Option<&CodexReplayPlan>,
    source: &paths::CodexUsageSource,
    path: &Path,
    index: usize,
    diagnostics: &Diagnostics,
) -> Result<Option<CodexSessionMetadata>> {
    let mut provenance = CodexEventProvenance::new(index, source.configured_tier);
    let mut result = Ok(());
    let mut visit = |event: CodexTokenUsageEvent| {
        if result.is_ok()
            && !replay.is_some_and(|replay| {
                replay.is_replayed_compaction(path, event.response_id.as_deref())
            })
        {
            result = seen.record(&event, &mut provenance);
        }
    };
    let metadata = if let Some(replay) = replay {
        parser::visit_codex_session_file_with_metadata(
            source.dir(),
            path,
            replay.fork_replay(path),
            &mut visit,
            diagnostics,
        )
    } else {
        parser::visit_codex_session_file_unless_fork(source.dir(), path, &mut visit, diagnostics)
    };
    result?;
    if let Some(metadata) = &metadata {
        seen.record_lineage(&provenance, metadata)?;
    }
    Ok(metadata.filter(|_| replay.is_none()))
}

fn has_date_bounds(shared: &SharedArgs) -> bool {
    shared.since.is_some() || shared.until.is_some()
}

#[cfg(test)]
mod tests;
