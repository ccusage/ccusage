use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    thread,
};

use ccusage_core::Diagnostics;
use serde_json::Value;

use crate::{CodexRawUsage, TimestampMs, chunk_file_indexes_by_size, parse_ts_timestamp};

use super::{
    parser::{
        CodexForkReplay, codex_value_timestamp, visit_codex_session_file_with_compaction_history,
    },
    reader::{PROBE_BUFFER_SIZE, open_rollout_lines},
};

pub(super) struct CodexReplayPlan {
    parent_by_child: HashMap<PathBuf, ParentReplay>,
    usage_by_parent: HashMap<PathBuf, ParentUsage>,
}

struct ParentReplay {
    /// `None` when the referenced parent log is not part of the scanned files.
    path: Option<PathBuf>,
    forked_at: Option<TimestampMs>,
    spawned: bool,
}

struct ParentUsage {
    timestamps: Vec<Option<TimestampMs>>,
    usage: Vec<CodexRawUsage>,
    compactions: HashMap<String, Option<TimestampMs>>,
}

impl CodexReplayPlan {
    /// Plans from metadata already read from the first line of every file in
    /// `groups`, in the same order.
    pub(super) fn with_metadata<'a>(
        groups: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        metadata: Vec<CodexSessionMetadata>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        Self::from_metadata(
            collect_replay_files(groups),
            metadata,
            single_thread,
            diagnostics,
        )
    }

    fn from_metadata(
        files: Vec<(PathBuf, &Path)>,
        metadata: Vec<CodexSessionMetadata>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let files_by_session_id = index_session_paths(&files, &metadata);
        let parent_by_child = build_parent_replays(&files, metadata, &files_by_session_id);
        let parent_paths = parent_by_child
            .values()
            .filter_map(|parent| parent.path.clone())
            .collect::<HashSet<_>>();
        let parents = files
            .iter()
            .filter(|(path, _)| parent_paths.contains(path))
            .cloned()
            .collect::<Vec<_>>();

        Self {
            parent_by_child,
            usage_by_parent: read_parent_usage(&parents, single_thread, diagnostics),
        }
    }

    pub(super) fn for_bounded_files<'a>(
        children: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        available_files: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let children = collect_replay_files(children);
        let available_files = collect_replay_files(available_files);
        let available_metadata = read_session_metadata(&available_files, single_thread);
        let files_by_session_id = index_session_paths(&available_files, &available_metadata);
        let mut metadata_by_path = HashMap::with_capacity(available_files.len());
        for ((path, _), metadata) in available_files.iter().zip(available_metadata) {
            metadata_by_path.entry(path.clone()).or_insert(metadata);
        }
        let child_metadata = children.iter().map(|(child, _)| {
            metadata_by_path
                .get(child)
                .cloned()
                .unwrap_or_else(|| read_codex_session_metadata(child))
        });
        let parent_by_child = build_parent_replays(&children, child_metadata, &files_by_session_id);
        let parent_paths = parent_by_child
            .values()
            .filter_map(|parent| parent.path.clone())
            .collect::<HashSet<_>>();
        let parents = available_files
            .iter()
            .filter(|(path, _)| parent_paths.contains(path))
            .cloned()
            .collect::<Vec<_>>();

        Self {
            parent_by_child,
            usage_by_parent: read_parent_usage(&parents, single_thread, diagnostics),
        }
    }

    /// Usage history that `child` replayed from the session it forked from.
    ///
    /// Returns `None` for sessions that are not forks. Forked sessions whose
    /// parent log is unavailable get an empty prefix, which tells the parser
    /// to fall back to the rewritten-burst heuristic.
    pub(super) fn fork_replay(&self, child: &Path) -> Option<CodexForkReplay<'_>> {
        let parent = self.parent_by_child.get(child)?;
        Some(CodexForkReplay {
            prefix: self.replay_prefix(parent),
            ends_at_usage_record: parent.spawned,
        })
    }

    fn replay_prefix(&self, parent: &ParentReplay) -> &[CodexRawUsage] {
        let Some(stream) = parent
            .path
            .as_ref()
            .and_then(|path| self.usage_by_parent.get(path))
        else {
            return &[];
        };
        // Usage the parent recorded after the fork was never replayed, so it must
        // not mask the child's own events.
        let replay_len = parent.forked_at.map_or(stream.usage.len(), |forked_at| {
            stream
                .timestamps
                .iter()
                .position(|timestamp| timestamp.is_some_and(|timestamp| timestamp > forked_at))
                .unwrap_or(stream.usage.len())
        });
        &stream.usage[..replay_len]
    }

    /// Recognizes copied compaction requests even when date filtering excludes
    /// their parent from the report. Requests after the fork stay countable.
    pub(super) fn is_replayed_compaction(&self, child: &Path, response_id: Option<&str>) -> bool {
        let Some(response_id) = response_id else {
            return false;
        };
        let Some(parent) = self.parent_by_child.get(child) else {
            return false;
        };
        let Some(timestamp) = parent
            .path
            .as_ref()
            .and_then(|path| self.usage_by_parent.get(path))
            .and_then(|stream| stream.compactions.get(response_id))
        else {
            return false;
        };
        parent
            .forked_at
            .is_none_or(|forked_at| timestamp.is_none_or(|timestamp| timestamp <= forked_at))
    }
}

fn index_session_paths(
    files: &[(PathBuf, &Path)],
    metadata: &[CodexSessionMetadata],
) -> HashMap<String, Vec<PathBuf>> {
    let mut files_by_session_id = HashMap::<String, Vec<PathBuf>>::with_capacity(files.len());
    for ((path, _), metadata) in files.iter().zip(metadata) {
        if let Some(session_id) = metadata.session_id.as_deref() {
            files_by_session_id
                .entry(session_id.to_string())
                .or_default()
                .push(path.clone());
        }
    }
    files_by_session_id
}

fn build_parent_replays(
    children: &[(PathBuf, &Path)],
    metadata: impl IntoIterator<Item = CodexSessionMetadata>,
    files_by_session_id: &HashMap<String, Vec<PathBuf>>,
) -> HashMap<PathBuf, ParentReplay> {
    children
        .iter()
        .zip(metadata)
        .filter_map(|((child, _), metadata)| {
            let parent_id = metadata.parent_id.as_deref()?;
            Some((
                child.clone(),
                ParentReplay {
                    path: first_non_child_path(files_by_session_id, parent_id, child),
                    forked_at: metadata.timestamp,
                    spawned: metadata.spawned,
                },
            ))
        })
        .collect()
}

fn first_non_child_path(
    files_by_session_id: &HashMap<String, Vec<PathBuf>>,
    parent_id: &str,
    child: &Path,
) -> Option<PathBuf> {
    files_by_session_id
        .get(parent_id)?
        .iter()
        .find(|candidate| candidate.as_path() != child)
        .cloned()
}

fn collect_replay_files<'a>(
    groups: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
) -> Vec<(PathBuf, &'a Path)> {
    groups
        .into_iter()
        .flat_map(|(sessions_dir, files)| {
            files.iter().map(move |path| (path.clone(), sessions_dir))
        })
        .collect()
}

fn replay_worker_count(files: usize, single_thread: bool) -> usize {
    if single_thread || files <= 1 {
        return 1;
    }
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(files)
}

fn read_session_metadata(
    files: &[(PathBuf, &Path)],
    single_thread: bool,
) -> Vec<CodexSessionMetadata> {
    let worker_count = replay_worker_count(files.len(), single_thread);
    if worker_count <= 1 {
        return files
            .iter()
            .map(|(path, _)| read_codex_session_metadata(path))
            .collect();
    }
    // Every file costs one open plus one line, so split by count instead of by
    // size, and keep chunks in order so duplicate session ids resolve the same
    // way as the single-threaded pass.
    let chunk_size = files.len().div_ceil(worker_count);
    thread::scope(|scope| {
        files
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(|| {
                    chunk
                        .iter()
                        .map(|(path, _)| read_codex_session_metadata(path))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .expect("codex replay metadata worker panicked")
            })
            .collect()
    })
}

fn read_parent_usage(
    parents: &[(PathBuf, &Path)],
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> HashMap<PathBuf, ParentUsage> {
    if parents.is_empty() {
        return HashMap::new();
    }
    let worker_count = replay_worker_count(parents.len(), single_thread);
    // A parent the report also reads records the same skips again, which
    // `Diagnostics` counts once.
    let read = |(path, sessions_dir): &(PathBuf, &Path)| {
        read_parent_usage_file(path, sessions_dir, diagnostics)
    };
    if worker_count <= 1 {
        return parents.iter().map(read).collect();
    }
    let paths = parents
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let chunks = chunk_file_indexes_by_size(&paths, worker_count);
    thread::scope(|scope| {
        chunks
            .into_iter()
            .map(|chunk| {
                let read = &read;
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|index| read(&parents[index]))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|handle| handle.join().expect("codex replay worker panicked"))
            .collect()
    })
}

fn read_parent_usage_file(
    path: &Path,
    sessions_dir: &Path,
    diagnostics: &Diagnostics,
) -> (PathBuf, ParentUsage) {
    (
        path.to_path_buf(),
        read_usage_events(sessions_dir, path, diagnostics),
    )
}

fn read_usage_events(sessions_dir: &Path, path: &Path, diagnostics: &Diagnostics) -> ParentUsage {
    let mut stream = ParentUsage {
        timestamps: Vec::new(),
        usage: Vec::new(),
        compactions: HashMap::new(),
    };
    let ParentUsage {
        timestamps,
        usage,
        compactions,
    } = &mut stream;
    let _ = visit_codex_session_file_with_compaction_history(
        sessions_dir,
        path,
        None,
        Some(compactions),
        |event| {
            // Response IDs deduplicate compactions independently of this normal
            // token-count prefix, including when a fork omits compaction records.
            if event.response_id.is_some() {
                return Ok(());
            }
            timestamps.push(parse_ts_timestamp(&event.timestamp));
            usage.push(event.raw_usage());
            Ok(())
        },
        diagnostics,
    );
    stream
}

#[derive(Clone, Default)]
pub(super) struct CodexSessionMetadata {
    session_id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<TimestampMs>,
    spawned: bool,
}

impl CodexSessionMetadata {
    /// Whether the rollout replays history from the session it forked from.
    pub(super) fn is_fork(&self) -> bool {
        self.parent_id.is_some()
    }
}

fn read_codex_session_metadata(path: &Path) -> CodexSessionMetadata {
    let Ok((mut lines, _)) = open_rollout_lines(path, PROBE_BUFFER_SIZE) else {
        return CodexSessionMetadata::default();
    };
    let Ok(Some(line)) = lines.next_line() else {
        return CodexSessionMetadata::default();
    };
    session_metadata_from_line(line)
}

/// Session metadata from a rollout's first line.
pub(super) fn session_metadata_from_line(line: &[u8]) -> CodexSessionMetadata {
    let Ok(line) = std::str::from_utf8(line) else {
        return CodexSessionMetadata::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return CodexSessionMetadata::default();
    };
    let payload = (value.get("type").and_then(Value::as_str) == Some("session_meta"))
        .then_some(value.get("payload"))
        .flatten();
    let spawn_parent_id = payload
        .and_then(|payload| payload.pointer("/source/subagent/thread_spawn/parent_thread_id"))
        .and_then(Value::as_str)
        .filter(|parent_id| !parent_id.is_empty());
    CodexSessionMetadata {
        timestamp: codex_value_timestamp(value.get("timestamp")),
        session_id: payload
            .and_then(|payload| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        parent_id: payload
            .and_then(|payload| payload.get("forked_from_id"))
            .and_then(Value::as_str)
            .filter(|parent_id| !parent_id.is_empty())
            .or(spawn_parent_id)
            .map(str::to_string),
        spawned: spawn_parent_id.is_some(),
    }
}
