use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    thread,
};

#[cfg(test)]
use std::cell::RefCell;

use serde_json::Value;

use crate::{CodexRawUsage, TimestampMs, chunk_file_indexes_by_size, parse_ts_timestamp};

use super::parser::{codex_value_timestamp, visit_codex_session_file};

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(super) enum ObservedFileRead {
    MetadataProbe(PathBuf),
    ParentUsage(PathBuf),
}

#[cfg(test)]
thread_local! {
    static OBSERVED_FILE_READS: RefCell<Vec<ObservedFileRead>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_observed_file_reads() -> Vec<PathBuf> {
    take_observed_file_read_events()
        .into_iter()
        .map(|read| match read {
            ObservedFileRead::MetadataProbe(path) | ObservedFileRead::ParentUsage(path) => path,
        })
        .collect()
}

#[cfg(test)]
pub(super) fn take_observed_file_read_events() -> Vec<ObservedFileRead> {
    OBSERVED_FILE_READS.with(|paths| std::mem::take(&mut *paths.borrow_mut()))
}

fn observe_metadata_probe(path: &Path) {
    #[cfg(test)]
    {
        OBSERVED_FILE_READS.with(|paths| {
            paths
                .borrow_mut()
                .push(ObservedFileRead::MetadataProbe(path.to_path_buf()));
        });
    }
    #[cfg(not(test))]
    {
        let _ = path;
    }
}

fn observe_parent_usage(path: &Path) {
    #[cfg(test)]
    {
        OBSERVED_FILE_READS.with(|paths| {
            paths
                .borrow_mut()
                .push(ObservedFileRead::ParentUsage(path.to_path_buf()));
        });
    }
    #[cfg(not(test))]
    {
        let _ = path;
    }
}

pub(super) struct CodexReplayPlan {
    parent_by_child: HashMap<PathBuf, ParentReplay>,
    usage_by_parent: HashMap<PathBuf, ParentUsage>,
}

struct ParentReplay {
    /// `None` when the referenced parent log is not part of the scanned files.
    path: Option<PathBuf>,
    forked_at: Option<TimestampMs>,
}

struct ParentUsage {
    timestamps: Vec<Option<TimestampMs>>,
    usage: Vec<CodexRawUsage>,
}

impl CodexReplayPlan {
    pub(super) fn new<'a>(
        groups: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
    ) -> Self {
        let files = collect_replay_files(groups);
        let metadata = read_session_metadata(&files, single_thread);
        // The same session can be reachable from more than one source directory,
        // so keep the first file and stay independent of source ordering.
        let mut files_by_session_id = HashMap::new();
        for ((path, _), metadata) in files.iter().zip(&metadata) {
            if let Some(session_id) = metadata.session_id.as_deref() {
                files_by_session_id.entry(session_id).or_insert(path);
            }
        }
        let parent_by_child = files
            .iter()
            .zip(&metadata)
            .filter_map(|((child, _), metadata)| {
                let parent_id = metadata.parent_id.as_deref()?;
                let path = files_by_session_id
                    .get(parent_id)
                    // A session listing itself as its own parent would match its
                    // whole stream and drop every event it recorded.
                    .filter(|parent| **parent != child)
                    .map(|parent| (*parent).clone());
                Some((
                    child.clone(),
                    ParentReplay {
                        path,
                        forked_at: metadata.timestamp,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();
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
            usage_by_parent: read_parent_usage(&parents, single_thread),
        }
    }

    pub(super) fn for_bounded_files<'a>(
        children: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        available_files: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
    ) -> Self {
        let children = collect_replay_files(children);
        let available_files = collect_replay_files(available_files);
        let available_metadata = read_session_metadata(&available_files, single_thread);
        let mut files_by_session_id = HashMap::with_capacity(available_files.len());
        let mut metadata_by_path = HashMap::with_capacity(available_files.len());
        for ((path, _), metadata) in available_files.iter().zip(available_metadata) {
            if let Some(session_id) = metadata.session_id.as_deref() {
                files_by_session_id
                    .entry(session_id.to_string())
                    .or_insert_with(|| path.clone());
            }
            metadata_by_path.entry(path.clone()).or_insert(metadata);
        }
        let child_metadata = children.iter().map(|(child, _)| {
            metadata_by_path
                .get(child)
                .cloned()
                .unwrap_or_else(|| read_codex_session_metadata(child))
        });
        let parent_by_child = children
            .iter()
            .zip(child_metadata)
            .filter_map(|((child, _), metadata)| {
                let parent_id = metadata.parent_id.as_deref()?;
                let path = files_by_session_id
                    .get(parent_id)
                    .filter(|parent| **parent != *child)
                    .cloned();
                Some((
                    child.clone(),
                    ParentReplay {
                        path,
                        forked_at: metadata.timestamp,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();
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
            usage_by_parent: read_parent_usage(&parents, single_thread),
        }
    }

    /// Usage history that `child` replayed from the session it forked from.
    ///
    /// Returns `None` for sessions that are not forks. Forked sessions whose
    /// parent log is unavailable return an empty slice, which tells the parser
    /// to fall back to the rewritten-second heuristic.
    pub(super) fn replay_prefix(&self, child: &Path) -> Option<&[CodexRawUsage]> {
        let parent = self.parent_by_child.get(child)?;
        let Some(stream) = parent
            .path
            .as_ref()
            .and_then(|path| self.usage_by_parent.get(path))
        else {
            return Some(&[]);
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
        Some(&stream.usage[..replay_len])
    }
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
) -> HashMap<PathBuf, ParentUsage> {
    if parents.is_empty() {
        return HashMap::new();
    }
    let worker_count = replay_worker_count(parents.len(), single_thread);
    if worker_count <= 1 {
        return parents
            .iter()
            .map(|(path, sessions_dir)| read_parent_usage_file(path, sessions_dir))
            .collect();
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
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|index| {
                            let (path, sessions_dir) = &parents[index];
                            read_parent_usage_file(path, sessions_dir)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|handle| handle.join().expect("codex replay worker panicked"))
            .collect()
    })
}

fn read_parent_usage_file(path: &Path, sessions_dir: &Path) -> (PathBuf, ParentUsage) {
    let (timestamps, usage) = read_usage_events(sessions_dir, path)
        .into_iter()
        .map(|(timestamp, usage)| (parse_ts_timestamp(&timestamp), usage))
        .unzip();
    (path.to_path_buf(), ParentUsage { timestamps, usage })
}

fn read_usage_events(sessions_dir: &Path, path: &Path) -> Vec<(String, CodexRawUsage)> {
    observe_parent_usage(path);
    let mut usage = Vec::new();
    let _ = visit_codex_session_file(sessions_dir, path, None, |event| {
        usage.push((
            event.timestamp,
            CodexRawUsage {
                input_tokens: event.input_tokens,
                cached_input_tokens: event.cached_input_tokens,
                cache_creation_tokens: event.cache_creation_tokens,
                output_tokens: event.output_tokens,
                reasoning_output_tokens: event.reasoning_output_tokens,
                total_tokens: event.total_tokens,
            },
        ));
        Ok(())
    });
    usage
}

#[derive(Clone, Default)]
struct CodexSessionMetadata {
    session_id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<TimestampMs>,
}

fn read_codex_session_metadata(path: &Path) -> CodexSessionMetadata {
    observe_metadata_probe(path);
    let Ok(file) = File::open(path) else {
        return CodexSessionMetadata::default();
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let Ok(bytes_read) = reader.read_line(&mut line) else {
        return CodexSessionMetadata::default();
    };
    if bytes_read == 0 {
        return CodexSessionMetadata::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(&line) else {
        return CodexSessionMetadata::default();
    };
    let payload = (value.get("type").and_then(Value::as_str) == Some("session_meta"))
        .then_some(value.get("payload"))
        .flatten();
    CodexSessionMetadata {
        timestamp: codex_value_timestamp(value.get("timestamp")),
        session_id: payload
            .and_then(|payload| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        parent_id: payload
            .and_then(|payload| payload.get("forked_from_id"))
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .and_then(|payload| {
                        payload.pointer("/source/subagent/thread_spawn/parent_thread_id")
                    })
                    .and_then(Value::as_str)
            })
            .filter(|parent_id| !parent_id.is_empty())
            .map(str::to_string),
    }
}
