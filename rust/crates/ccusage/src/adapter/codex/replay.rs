use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    thread,
};

use serde_json::Value;

use crate::{CodexRawUsage, TimestampMs, chunk_file_indexes_by_size, parse_ts_timestamp};

use super::parser::{detect_replay_second, visit_codex_session_file};

pub(super) struct CodexReplayPlan {
    parent_by_child: HashMap<PathBuf, ParentReplay>,
    usage_by_parent: HashMap<PathBuf, ParentUsage>,
}

struct ParentReplay {
    path: PathBuf,
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
        let groups = groups.into_iter().collect::<Vec<_>>();
        let metadata = groups
            .iter()
            .flat_map(|(sessions_dir, files)| {
                files.iter().map(|path| {
                    (
                        path.clone(),
                        *sessions_dir,
                        read_codex_session_metadata(path),
                    )
                })
            })
            .collect::<Vec<_>>();
        let files_by_session_id = metadata
            .iter()
            .filter_map(|(path, _, metadata)| {
                metadata
                    .session_id
                    .as_ref()
                    .map(|session_id| (session_id.as_str(), path.clone()))
            })
            .collect::<HashMap<_, _>>();
        let mut parent_by_child = metadata
            .iter()
            .filter_map(|(child, _, metadata)| {
                metadata.parent_id.as_deref().and_then(|parent_id| {
                    files_by_session_id.get(parent_id).map(|parent| {
                        (
                            child.clone(),
                            ParentReplay {
                                path: parent.clone(),
                                forked_at: metadata.timestamp,
                            },
                        )
                    })
                })
            })
            .collect::<HashMap<_, _>>();
        let parent_paths = parent_by_child
            .values()
            .map(|parent| parent.path.clone())
            .collect::<HashSet<_>>();
        let parents = metadata
            .iter()
            .filter(|(path, _, _)| parent_paths.contains(path))
            .map(|(path, sessions_dir, _)| (path.clone(), *sessions_dir))
            .collect::<Vec<_>>();
        let mut usage_by_parent = read_parent_usage(&parents, single_thread);
        for (child, sessions_dir, metadata) in &metadata {
            if metadata.parent_id.is_some() && !parent_by_child.contains_key(child) {
                let replayed_usage = replayed_usage_from_first_second(sessions_dir, child);
                if !replayed_usage.is_empty() {
                    parent_by_child.insert(
                        child.clone(),
                        ParentReplay {
                            path: child.clone(),
                            forked_at: None,
                        },
                    );
                    usage_by_parent.insert(
                        child.clone(),
                        ParentUsage {
                            timestamps: vec![None; replayed_usage.len()],
                            usage: replayed_usage,
                        },
                    );
                }
            }
        }

        Self {
            parent_by_child,
            usage_by_parent,
        }
    }

    pub(super) fn parent_usage(&self, child: &Path) -> Option<&[CodexRawUsage]> {
        let parent = self.parent_by_child.get(child)?;
        let stream = self.usage_by_parent.get(&parent.path)?;
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

fn read_parent_usage(
    parents: &[(PathBuf, &Path)],
    single_thread: bool,
) -> HashMap<PathBuf, ParentUsage> {
    if parents.is_empty() {
        return HashMap::new();
    }
    let worker_count = if single_thread {
        1
    } else {
        thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(parents.len())
    };
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
                            let (timestamps, usage) = read_usage_events(sessions_dir, path)
                                .into_iter()
                                .map(|(timestamp, usage)| (parse_ts_timestamp(&timestamp), usage))
                                .unzip();
                            (path.clone(), ParentUsage { timestamps, usage })
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

fn read_usage_events(sessions_dir: &Path, path: &Path) -> Vec<(String, CodexRawUsage)> {
    let mut usage = Vec::new();
    let _ = visit_codex_session_file(sessions_dir, path, None, |event| {
        usage.push((
            event.timestamp,
            CodexRawUsage {
                input_tokens: event.input_tokens,
                cached_input_tokens: event.cached_input_tokens,
                output_tokens: event.output_tokens,
                reasoning_output_tokens: event.reasoning_output_tokens,
                total_tokens: event.total_tokens,
            },
        ));
        Ok(())
    });
    usage
}

fn replayed_usage_from_first_second(sessions_dir: &Path, path: &Path) -> Vec<CodexRawUsage> {
    let Some(first_second) = detect_replay_second(path) else {
        return Vec::new();
    };
    read_usage_events(sessions_dir, path)
        .into_iter()
        .take_while(|(timestamp, _)| {
            timestamp.as_bytes().get(..19) == Some(first_second.as_slice())
        })
        .map(|(_, usage)| usage)
        .collect()
}

#[derive(Default)]
struct CodexSessionMetadata {
    session_id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<TimestampMs>,
}

fn read_codex_session_metadata(path: &Path) -> CodexSessionMetadata {
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
        timestamp: value
            .get("timestamp")
            .and_then(|timestamp| match timestamp {
                Value::String(timestamp) => parse_ts_timestamp(timestamp),
                Value::Number(timestamp) => timestamp.as_u64().and_then(|raw| {
                    let millis = if raw > 10_000_000_000 {
                        raw
                    } else {
                        raw.checked_mul(1_000)?
                    };
                    Some(TimestampMs::from_millis(millis.min(i64::MAX as u64) as i64))
                }),
                _ => None,
            }),
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
