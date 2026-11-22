use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use ccusage_adapter_common::{
    collect_files_with_extension, dedupe::dedupe_first_wins, source::LoadContext,
    units::read_typed_units,
};

use crate::{LoadedEntry, Result};

use super::{
    parser::{self, ParentSession},
    paths,
};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    load_entries_for_context(
        cx,
        &paths::paths(cx.options.pi_path, cx.options.pi_agent_dir)?,
        "pi",
    )
}

pub(crate) fn load_named_store_entries(
    cx: &LoadContext<'_>,
    store_paths: &[PathBuf],
    store_name: &str,
) -> Result<Vec<LoadedEntry>> {
    load_entries_for_context(cx, store_paths, store_name)
}

fn load_entries_for_context(
    cx: &LoadContext<'_>,
    paths: &[PathBuf],
    store_name: &str,
) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let shared = cx.shared;
    let mut discovered_files = Vec::new();
    let mut loaded_files = Vec::new();
    for path in paths {
        let mut files = Vec::new();
        collect_files_with_extension(path, "jsonl", &mut files, cx.diagnostics);
        // NOTE(2026-09-15, #1714): pi-subagents writes debug transcripts under `subagent-artifacts/` that copy calls without their session header or entry IDs. Skip that directory and retain the primary `run-*/` child sessions.
        files.retain(|file| !is_subagent_artifact_transcript(file));
        let loaded = read_typed_units(&files, shared.single_thread, cx.diagnostics, |file| {
            parser::read_session_file_data_for_store_with_diagnostics(
                file,
                path,
                cx.window.timezone(),
                shared.mode,
                pricing,
                store_name,
                cx.diagnostics,
            )
        });
        discovered_files.extend(files);
        loaded_files.extend(loaded);
    }

    let replay_plan = PiReplayPlan::new(&discovered_files, &loaded_files);
    let records = loaded_files
        .into_iter()
        .enumerate()
        .flat_map(|(index, data)| {
            data.into_iter()
                .flat_map(|data| data.entries)
                .skip(replay_plan.skip_prefix(index))
        });
    let mut entries = dedupe_first_wins(records, parser::entry_id);
    entries.sort_by_cached_key(|entry| (entry.timestamp, parser::entry_id(entry)));
    Ok(entries.into_iter().map(|entry| entry.entry).collect())
}

struct ParentReplay {
    parent_index: usize,
    fork_timestamp: crate::TimestampMs,
}

struct PiReplayPlan {
    skip_by_file: HashMap<usize, usize>,
}

impl PiReplayPlan {
    fn new(files: &[PathBuf], loaded: &[Option<parser::PiSessionData>]) -> Self {
        let mut files_by_path = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            files_by_path.entry(normalize_path(file)).or_insert(index);
        }

        let mut invalid_lineage = HashSet::new();
        for (index, data) in loaded.iter().enumerate() {
            let Some(header) = data.as_ref().and_then(|data| data.header.as_ref()) else {
                invalid_lineage.insert(index);
                continue;
            };
            if header.timestamp.is_none() || header.parent_session == ParentSession::Malformed {
                invalid_lineage.insert(index);
            }
        }

        let mut parent_by_child = HashMap::new();
        for (child_index, data) in loaded.iter().enumerate() {
            let Some(header) = data.as_ref().and_then(|data| data.header.as_ref()) else {
                continue;
            };
            let ParentSession::Path(parent_path) = &header.parent_session else {
                continue;
            };
            let Some(fork_timestamp) = header.timestamp else {
                continue;
            };
            let Some(parent_index) =
                resolve_parent_index(parent_path, &files[child_index], &files_by_path)
            else {
                invalid_lineage.insert(child_index);
                continue;
            };
            if parent_index == child_index {
                invalid_lineage.insert(child_index);
                continue;
            }
            parent_by_child.insert(
                child_index,
                ParentReplay {
                    parent_index,
                    fork_timestamp,
                },
            );
        }

        let mut skip_by_file = HashMap::new();
        for (&child_index, replay) in &parent_by_child {
            if !has_valid_lineage(child_index, &parent_by_child, &invalid_lineage) {
                continue;
            }
            let Some(parent) = loaded
                .get(replay.parent_index)
                .and_then(|data| data.as_ref())
            else {
                continue;
            };
            let Some(child) = loaded.get(child_index).and_then(|data| data.as_ref()) else {
                continue;
            };
            let Some(matched) = parent.matching_replay_prefix(child, replay.fork_timestamp) else {
                continue;
            };
            if matched > 0 {
                skip_by_file.insert(child_index, matched);
            }
        }

        Self { skip_by_file }
    }

    fn skip_prefix(&self, file_index: usize) -> usize {
        self.skip_by_file.get(&file_index).copied().unwrap_or(0)
    }
}

fn has_valid_lineage(
    child_index: usize,
    parent_by_child: &HashMap<usize, ParentReplay>,
    invalid_lineage: &HashSet<usize>,
) -> bool {
    let mut visited = HashSet::new();
    let mut current = child_index;
    while let Some(replay) = parent_by_child.get(&current) {
        if invalid_lineage.contains(&current) {
            return false;
        }
        if !visited.insert(current) {
            return false;
        }
        current = replay.parent_index;
    }
    !invalid_lineage.contains(&current)
}

fn resolve_parent_index(
    parent_path: &Path,
    child_path: &Path,
    files_by_path: &HashMap<PathBuf, usize>,
) -> Option<usize> {
    let normalized_parent = normalize_path(parent_path);
    if let Some(parent_index) = files_by_path.get(&normalized_parent).copied() {
        return Some(parent_index);
    }
    if parent_path.is_relative()
        && let Some(parent) = child_path.parent()
    {
        return files_by_path
            .get(&normalize_path(&parent.join(parent_path)))
            .copied();
    }
    None
}

fn is_subagent_artifact_transcript(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == "subagent-artifacts")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_default()
    };
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests;
