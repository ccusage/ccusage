use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use ccusage_core::Diagnostics;

use crate::{CodexRawUsage, TimestampMs};

use super::{
    parser::{
        CodexForkReplay, codex_timestamp_instant, visit_codex_session_file_with_compaction_history,
    },
    reader::{PROBE_BUFFER_SIZE, open_rollout_lines},
    types::CodexSessionMeta,
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
    timestamps: Vec<TimestampMs>,
    usage: Vec<CodexRawUsage>,
    compactions: HashMap<String, Option<TimestampMs>>,
}

pub(super) struct CodexReplayFile<'a> {
    pub(super) path: PathBuf,
    pub(super) sessions_dir: &'a Path,
    pub(super) metadata: CodexSessionMetadata,
}

impl CodexReplayPlan {
    pub(super) fn for_selection<'a>(
        selected: &[(&'a Path, &'a [PathBuf])],
        available: &[(&'a Path, &'a [PathBuf])],
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let mut files = read_session_metadata(selected.iter().copied(), single_thread, diagnostics);
        let children = files
            .iter()
            .filter(|file| file.metadata.is_fork())
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();
        if children.is_empty() {
            return Self {
                parent_by_child: HashMap::new(),
                usage_by_parent: HashMap::new(),
            };
        }
        let mut metadata = files
            .drain(..)
            .map(|file| (file.path.clone(), file))
            .collect::<HashMap<_, _>>();
        let missing = collect_replay_files(available.iter().copied())
            .into_iter()
            .filter(|(path, _)| !metadata.contains_key(path))
            .collect::<Vec<_>>();
        for file in read_replay_files(&missing, single_thread, diagnostics) {
            metadata.insert(file.path.clone(), file);
        }
        let files = available
            .iter()
            .flat_map(|(_, paths)| paths.iter())
            .filter_map(|path| metadata.remove(path))
            .collect();
        Self::new(files, Some(&children), single_thread, diagnostics)
    }

    pub(super) fn new(
        files: Vec<CodexReplayFile<'_>>,
        children: Option<&HashSet<PathBuf>>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let mut files_by_session_id = HashMap::<String, Vec<PathBuf>>::with_capacity(files.len());
        for file in &files {
            if let Some(id) = &file.metadata.session_id {
                files_by_session_id
                    .entry(id.clone())
                    .or_default()
                    .push(file.path.clone());
            }
        }
        let parent_by_child = files
            .iter()
            .filter(|file| children.is_none_or(|children| children.contains(&file.path)))
            .filter_map(|file| {
                let parent_id = file.metadata.parent_id.as_deref()?;
                Some((
                    file.path.clone(),
                    ParentReplay {
                        path: first_non_child_path(&files_by_session_id, parent_id, &file.path),
                        forked_at: file.metadata.timestamp,
                        spawned: file.metadata.spawned,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();
        let parent_paths = parent_by_child
            .values()
            .filter_map(|parent| parent.path.as_ref())
            .collect::<HashSet<_>>();
        let parents = files
            .iter()
            .filter(|file| parent_paths.contains(&file.path))
            .map(|file| (file.path.clone(), file.sessions_dir))
            .collect::<Vec<_>>();
        Self {
            usage_by_parent: read_parent_usage(&parents, single_thread, diagnostics),
            parent_by_child,
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
                .position(|timestamp| *timestamp > forked_at)
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

fn read_session_metadata<'a>(
    groups: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> Vec<CodexReplayFile<'a>> {
    read_replay_files(&collect_replay_files(groups), single_thread, diagnostics)
}

fn read_replay_files<'a>(
    files: &[(PathBuf, &'a Path)],
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> Vec<CodexReplayFile<'a>> {
    let read = |(path, sessions_dir): &(PathBuf, &'a Path)| CodexReplayFile {
        path: path.clone(),
        sessions_dir,
        metadata: read_codex_session_metadata(path, diagnostics),
    };
    ccusage_adapter_common::map_parallel(
        files,
        single_thread,
        ccusage_adapter_common::Split::ByCount,
        read,
    )
}

fn read_parent_usage(
    parents: &[(PathBuf, &Path)],
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> HashMap<PathBuf, ParentUsage> {
    // A parent the report also reads records the same skips again, which
    // `Diagnostics` counts once.
    let read = |(path, sessions_dir): &(PathBuf, &Path)| {
        read_parent_usage_file(path, sessions_dir, diagnostics)
    };
    ccusage_adapter_common::map_parallel(
        parents,
        single_thread,
        ccusage_adapter_common::Split::BySize(|(path, _)| path.as_path()),
        read,
    )
    .into_iter()
    .collect()
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
    visit_codex_session_file_with_compaction_history(
        sessions_dir,
        path,
        None,
        Some(compactions),
        |event| {
            // Response IDs deduplicate compactions independently of this normal
            // token-count prefix, including when a fork omits compaction records.
            if event.response_id.is_some() {
                return;
            }
            timestamps.push(event.timestamp);
            usage.push(event.raw_usage());
        },
        diagnostics,
    );
    stream
}

#[derive(Clone, Default)]
pub(super) struct CodexSessionMetadata {
    pub(super) is_session_meta: bool,
    session_id: Option<String>,
    parent_id: Option<String>,
    pub(super) timestamp: Option<TimestampMs>,
    spawned: bool,
}

impl CodexSessionMetadata {
    /// Whether the rollout replays history from the session it forked from.
    pub(super) fn is_fork(&self) -> bool {
        self.parent_id.is_some()
    }
}

fn read_codex_session_metadata(path: &Path, diagnostics: &Diagnostics) -> CodexSessionMetadata {
    let (mut lines, _) = match open_rollout_lines(path, PROBE_BUFFER_SIZE) {
        Ok(opened) => opened,
        Err(failed) => {
            ccusage_adapter_common::UnitError::new(
                ccusage_core::SkipKind::Unreadable,
                "rollout open failed",
            )
            .record(&failed, diagnostics);
            return CodexSessionMetadata::default();
        }
    };
    match lines.next_line() {
        Ok(line) => line.map_or_else(CodexSessionMetadata::default, |line| {
            session_metadata_from_line(path, line)
        }),
        Err(_) => {
            super::reader::record_rollout_read_failure(path, diagnostics);
            CodexSessionMetadata::default()
        }
    }
}

pub(super) fn session_metadata_from_line(path: &Path, line: &[u8]) -> CodexSessionMetadata {
    let Ok(value) = serde_json::from_slice::<CodexSessionMeta<'_>>(line) else {
        return CodexSessionMetadata::default();
    };
    let payload = (value.entry_type.as_deref() == Some("session_meta"))
        .then_some(value.payload.as_ref())
        .flatten();
    let spawn_parent_id = payload
        .and_then(|payload| payload.source.as_ref())
        .and_then(|source| source.subagent.as_ref())
        .and_then(|subagent| subagent.thread_spawn.as_ref())
        .and_then(|spawn| spawn.parent_thread_id.as_deref())
        .filter(|id| !id.is_empty());
    let session_id = payload.and_then(|payload| payload.id.as_deref());
    let parent_id = payload
        .filter(|payload| payload.history_base.is_none())
        .and_then(|payload| {
            payload
                .forked_from_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .or(spawn_parent_id)
        })
        .filter(|_| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(super::rollout_identity)
                .is_some_and(|(thread, rollout)| thread != rollout && session_id == Some(thread))
        })
        .map(str::to_string);
    CodexSessionMetadata {
        is_session_meta: value.entry_type.as_deref() == Some("session_meta"),
        timestamp: codex_timestamp_instant(value.timestamp.as_ref()),
        session_id: session_id.map(str::to_string),
        parent_id,
        spawned: spawn_parent_id.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_keeps_a_spawn_parent_when_optional_fields_are_malformed() {
        let metadata = session_metadata_from_line(
            Path::new("session.jsonl"),
            br#"{
            "type": "session_meta",
            "timestamp": 1767312000,
            "payload": {
                "id": false,
                "forked_from_id": [],
                "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent"}}},
                "base_instructions": {"irrelevant": null}
            }
        }"#,
        );
        assert!(metadata.is_fork());
        assert_eq!(metadata.parent_id.as_deref(), Some("parent"));
        assert_eq!(metadata.session_id, None);
        assert!(metadata.spawned);
        assert_eq!(
            metadata.timestamp,
            Some(TimestampMs::from_millis(1_767_312_000_000))
        );
    }

    #[test]
    fn metadata_uses_the_last_duplicate_identity() {
        let metadata = session_metadata_from_line(Path::new("session.jsonl"), br#"{
            "type": "session_meta",
            "payload": {"id": "first", "id": "last", "forked_from_id": "old", "forked_from_id": "parent"}
        }"#);
        assert_eq!(metadata.session_id.as_deref(), Some("last"));
        assert_eq!(metadata.parent_id.as_deref(), Some("parent"));
    }
}
