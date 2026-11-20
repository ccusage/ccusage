use ccusage_adapter_common::{json::JsonText, jsonl};
use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use ccusage_core::{Diagnostics, Skip, SkipKind};

use crate::{CodexRawUsage, TimestampMs};

use super::{
    parser::{
        CodexForkReplay, codex_timestamp_instant, visit_codex_session_file_with_compaction_history,
    },
    reader::{PROBE_BUFFER_SIZE, rollout_policy},
    types::CodexSessionMeta,
};

pub(super) struct CodexReplayPlan {
    parent_by_child: HashMap<PathBuf, ParentReplay>,
    usage_by_parent: HashMap<PathBuf, ParentUsage>,
    lineage_edges: Vec<(String, String)>,
}

struct ParentReplay {
    path: Option<PathBuf>,
    parent_id: String,
    child_id: Option<String>,
    unreadable: bool,
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
        available: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let mut failed_parent_ids = HashSet::new();
        let files = read_replay_files(
            &collect_replay_files(selected.iter().copied()),
            single_thread,
            Some(diagnostics),
            &mut failed_parent_ids,
        );
        Self::for_metadata(
            files,
            available,
            single_thread,
            diagnostics,
            failed_parent_ids,
        )
    }

    pub(super) fn for_probes<'a>(
        probes: Vec<ReplayProbe<'a>>,
        available: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
        diagnostics: &Diagnostics,
    ) -> Self {
        let mut failed_parent_ids = HashSet::new();
        let files = decode_probes(probes, &mut failed_parent_ids);
        Self::for_metadata(
            files,
            available,
            single_thread,
            diagnostics,
            failed_parent_ids,
        )
    }

    fn for_metadata<'a>(
        files: Vec<CodexReplayFile<'a>>,
        available: impl IntoIterator<Item = (&'a Path, &'a [PathBuf])>,
        single_thread: bool,
        diagnostics: &Diagnostics,
        mut failed_parent_ids: HashSet<String>,
    ) -> Self {
        let children = files
            .iter()
            .filter(|file| file.metadata.is_fork())
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();
        if children.is_empty() {
            return Self {
                parent_by_child: HashMap::new(),
                usage_by_parent: HashMap::new(),
                lineage_edges: Vec::new(),
            };
        }
        let mut metadata = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<HashMap<_, _>>();
        let available = available.into_iter().collect::<Vec<_>>();
        let missing = collect_replay_files(available.iter().copied())
            .into_iter()
            .filter(|(path, _)| !metadata.contains_key(path))
            .collect::<Vec<_>>();
        for file in read_replay_files(&missing, single_thread, None, &mut failed_parent_ids) {
            metadata.insert(file.path.clone(), file);
        }
        let files = available
            .iter()
            .flat_map(|(_, paths)| paths.iter())
            .filter_map(|path| metadata.remove(path))
            .collect();
        Self::new(
            files,
            Some((&children, &failed_parent_ids)),
            single_thread,
            diagnostics,
        )
    }

    pub(super) fn new(
        files: Vec<CodexReplayFile<'_>>,
        selection: Option<(&HashSet<PathBuf>, &HashSet<String>)>,
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
            .filter(|file| selection.is_none_or(|(children, _)| children.contains(&file.path)))
            .filter_map(|file| {
                let parent_id = file.metadata.parent_id.as_deref()?;
                let path = first_non_child_path(&files_by_session_id, parent_id, &file.path);
                let unreadable = path.is_none()
                    && selection.is_some_and(|(_, failed)| failed.contains(parent_id));
                if unreadable {
                    diagnostics.record(Skip::unit(
                        &file.path,
                        SkipKind::UnreadableForkParent,
                        format_args!("parent session {parent_id}"),
                    ));
                }
                Some((
                    file.path.clone(),
                    ParentReplay {
                        path,
                        parent_id: parent_id.to_string(),
                        child_id: file
                            .metadata
                            .session_id
                            .clone()
                            .filter(|id| !id.trim().is_empty()),
                        unreadable,
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
            lineage_edges: files.iter().filter_map(lineage_edge).collect(),
        }
    }

    pub(super) fn lineage_edges(&self) -> &[(String, String)] {
        &self.lineage_edges
    }

    pub(super) fn fork_replay(&self, child: &Path) -> Option<CodexForkReplay<'_>> {
        let parent = self.parent_by_child.get(child)?;
        Some(CodexForkReplay {
            parent_usage: self.replay_prefix(parent),
            missing_parent: (parent.path.is_none() && !parent.unreadable)
                .then_some(parent.parent_id.as_str()),
            child_id: parent.child_id.as_deref(),
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
        let replay_len = parent.forked_at.map_or(stream.usage.len(), |forked_at| {
            stream
                .timestamps
                .iter()
                .position(|timestamp| *timestamp > forked_at)
                .unwrap_or(stream.usage.len())
        });
        &stream.usage[..replay_len]
    }

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

pub(super) type ReplayProbe<'a> = (PathBuf, &'a Path, Option<CodexSessionMetadata>);

fn lineage_edge(file: &CodexReplayFile<'_>) -> Option<(String, String)> {
    Some((
        file.metadata.session_id.clone()?,
        file.metadata.lineage_parent_id.clone()?,
    ))
}

pub(super) fn read_lineage_edges(
    files: &[(PathBuf, &Path)],
    single_thread: bool,
) -> Vec<(String, String)> {
    read_replay_files(files, single_thread, None, &mut HashSet::new())
        .iter()
        .filter_map(lineage_edge)
        .collect()
}

fn read_replay_files<'a>(
    files: &[(PathBuf, &'a Path)],
    single_thread: bool,
    diagnostics: Option<&Diagnostics>,
    failed_parent_ids: &mut HashSet<String>,
) -> Vec<CodexReplayFile<'a>> {
    let read = |(path, sessions_dir): &(PathBuf, &'a Path)| {
        (
            path.clone(),
            *sessions_dir,
            read_codex_session_metadata(path, diagnostics),
        )
    };
    let probes = ccusage_adapter_common::map_parallel(
        files,
        single_thread,
        ccusage_adapter_common::Split::ByCount,
        read,
    );
    decode_probes(probes, failed_parent_ids)
}

fn decode_probes<'a>(
    probes: Vec<ReplayProbe<'a>>,
    failed_parent_ids: &mut HashSet<String>,
) -> Vec<CodexReplayFile<'a>> {
    probes
        .into_iter()
        .map(|(path, sessions_dir, metadata)| {
            if metadata.is_none()
                && let Some(name) = path.file_name().and_then(|name| name.to_str())
            {
                let id = super::rollout_identity(name)
                    .map_or_else(|| super::normalize_session_id(name), |(thread, _)| thread);
                failed_parent_ids.insert(id.to_string());
            }
            CodexReplayFile {
                path,
                sessions_dir,
                metadata: metadata.unwrap_or_default(),
            }
        })
        .collect()
}

fn read_parent_usage(
    parents: &[(PathBuf, &Path)],
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> HashMap<PathBuf, ParentUsage> {
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
    lineage_parent_id: Option<String>,
    timestamp: Option<TimestampMs>,
    spawned: bool,
}

impl CodexSessionMetadata {
    pub(super) fn is_fork(&self) -> bool {
        self.parent_id.is_some()
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(super) fn lineage_parent_id(&self) -> Option<&str> {
        self.lineage_parent_id.as_deref()
    }
}

fn read_codex_session_metadata(
    path: &Path,
    diagnostics: Option<&Diagnostics>,
) -> Option<CodexSessionMetadata> {
    jsonl::read(
        path,
        diagnostics,
        rollout_policy(path, PROBE_BUFFER_SIZE, false),
        |line, _, _| ControlFlow::Break(session_metadata_from_line(path, line.raw)),
    )
    .ok()
    .map(Option::unwrap_or_default)
}

pub(super) fn session_metadata_from_line(path: &Path, line: &[u8]) -> CodexSessionMetadata {
    let text = JsonText::new(line);
    let Ok(value) = text.parse::<CodexSessionMeta<'_>>() else {
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
    let lineage_parent_id = payload.and_then(|payload| {
        payload
            .forked_from_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .or(spawn_parent_id)
    });
    let parent_id = lineage_parent_id
        .filter(|_| payload.is_some_and(|payload| payload.history_base.is_none()))
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
        lineage_parent_id: lineage_parent_id.map(str::to_string),
        spawned: spawn_parent_id.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;

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
    #[test]
    fn reference_backed_history_keeps_usage_while_physical_forks_dedupe() {
        ccusage_test_support::install_pricing_snapshots();
        let parent_id = "01a0bb8c-c7c0-7630-9d82-860b875930f0";
        let child_id = "01a0cc9d-d8d1-8741-ae93-971c986041a1";
        let replacement_id = "01a0ddae-e9e2-7852-bfa4-a82da97152b2";
        for (child_file, history_base, expected) in [
            (
                "child.jsonl".to_string(),
                serde_json::json!({"thread_id":parent_id,"end_ordinal_exclusive":2}),
                300,
            ),
            (
                "non-object-history.jsonl".to_string(),
                serde_json::json!("parent"),
                150,
            ),
            (
                format!("rollout-2026-09-19T12-01-00-{child_id}_{replacement_id}.jsonl"),
                serde_json::Value::Null,
                300,
            ),
            (
                format!("backup-{child_id}_{replacement_id}.jsonl"),
                serde_json::Value::Null,
                150,
            ),
            (
                format!("rollout-backup-{child_id}_{replacement_id}.jsonl"),
                serde_json::Value::Null,
                150,
            ),
        ] {
            let usage = |timestamp: &str| {
                serde_json::json!({"timestamp":timestamp,"type":"event_msg","payload":{
                "type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}
            }}).to_string()
            };
            let fixture = fs_fixture!({
                "parent.jsonl": [
                    serde_json::json!({"timestamp":"2026-09-19T12:00:00Z","type":"session_meta","payload":{"id":parent_id}}).to_string(),
                    usage("2026-09-19T12:00:01Z"),
                ].join("\n"),
            });
            fixture.write_file(child_file, [
                    serde_json::json!({"timestamp":"2026-09-19T12:01:00Z","type":"session_meta","payload":{"id":child_id,"forked_from_id":parent_id,"history_base":history_base}}).to_string(),
                    usage("2026-09-19T12:02:00Z"),
                ].join("\n"));
            for single_thread in [false, true] {
                let groups = crate::test_support::load_groups_from_directory(
                    fixture.root(),
                    &crate::cli::SharedArgs {
                        single_thread,
                        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                        ..Default::default()
                    },
                    crate::cli::AgentReportKind::Daily,
                )
                .unwrap();
                assert_eq!(
                    groups.values().map(|group| group.total_tokens).sum::<u64>(),
                    expected,
                    "single_thread={single_thread}"
                );
            }
        }
    }

    #[test]
    fn bounded_regular_sessions_do_not_open_unrelated_rollouts() {
        let fixture = fs_fixture!({
            "current.jsonl": r#"{"type":"session_meta","payload":{"id":"current"}}"#,
            "other.jsonl.zst": "invalid zstd",
        });
        let selected = [fixture.path("current.jsonl")];
        let available = [selected[0].clone(), fixture.path("other.jsonl.zst")];
        let diagnostics = Diagnostics::new("codex");
        let mut visited = false;
        let plan = CodexReplayPlan::for_selection(
            &[(fixture.root(), selected.as_slice())],
            std::iter::once((fixture.root(), available.as_slice())).inspect(|_| visited = true),
            true,
            &diagnostics,
        );
        assert!(!visited);
        assert!(plan.parent_by_child.is_empty());
    }

    #[test]
    fn bounded_forks_warn_when_the_parent_is_unreadable_or_missing() {
        ccusage_test_support::Environment::with_var("LOG_LEVEL", "1").run(|| {
        let usage = |timestamp: &str| {
            serde_json::json!({"timestamp":timestamp,"type":"event_msg","payload":{
                "type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}
            }}).to_string()
        };
        let replayed = usage("2026-09-19T12:00:01Z");
        let fixture = fs_fixture!({
            "child.jsonl": [
                r#"{"type":"session_meta","timestamp":"2026-09-19T12:01:00Z","payload":{"id":"child","forked_from_id":"parent"}}"#.to_string(),
                replayed.clone(),
                usage("2026-09-19T12:02:00Z"),
            ].join("\n"),
            "parent.jsonl": [
                r#"{"type":"session_meta","timestamp":"2026-09-19T12:00:00Z","payload":{"id":"parent"}}"#,
                &replayed,
            ].join("\n"),
            "parent.jsonl.zst": "invalid zstd",
            "other.jsonl.zst": "invalid zstd",
        });
        let child = fixture.path("child.jsonl");
        let selected = [child.clone()];
        for (parents, expected_tokens, expected_warning) in [
            (
                vec![
                    fixture.path("parent.jsonl"),
                    fixture.path("parent.jsonl.zst"),
                ],
                150,
                None,
            ),
            (vec![fixture.path("parent.jsonl.zst")], 300, Some(("read", "unreadable"))),
            (vec![], 300, Some(("find", "missing"))),
        ] {
            let mut available = vec![child.clone(), fixture.path("other.jsonl.zst")];
            available.extend(parents);
            for single_thread in [true, false] {
                let diagnostics = Diagnostics::new("codex");
                let plan = CodexReplayPlan::for_selection(
                    &[(fixture.root(), &selected)],
                    [(fixture.root(), available.as_slice())],
                    single_thread,
                    &diagnostics,
                );
                let mut tokens = 0;
                crate::parser::visit_codex_session_file_with_diagnostics(
                    fixture.root(),
                    &selected[0],
                    plan.fork_replay(&selected[0]),
                    |event| tokens += event.total_tokens,
                    &diagnostics,
                );
                assert_eq!(tokens, expected_tokens);
                let default_warnings = diagnostics.warnings(&crate::cli::SharedArgs::default());
                let debug_warnings = diagnostics.warnings(&crate::cli::SharedArgs {
                    debug: true,
                    ..Default::default()
                });
                if let Some((verb, label)) = expected_warning {
                    let warning = format!("WARN  codex: could not {verb} the parent rollout for 1 forked session. Replayed usage may be counted twice.");
                    assert_eq!(default_warnings, std::slice::from_ref(&warning));
                    assert_eq!(
                        debug_warnings,
                        [
                            warning,
                            format!(
                                "codex: {label} fork parent: {}: parent session parent",
                                child.display()
                            ),
                        ]
                    );
                } else {
                    assert!(default_warnings.is_empty(), "{default_warnings:?}");
                    assert!(debug_warnings.is_empty(), "{debug_warnings:?}");
                }
            }
        }
        })
    }

    #[test]
    fn bounded_forks_probe_each_rollout_metadata_once() {
        let fixture = fs_fixture!({
            "child.jsonl": r#"{"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#,
            "parent.jsonl": r#"{"type":"session_meta","payload":{"id":"parent"}}"#,
            "other.jsonl.zst": "invalid zstd",
        });
        let selected = [fixture.path("child.jsonl")];
        let parent = fixture.path("parent.jsonl");
        let available = [
            selected[0].clone(),
            parent.clone(),
            fixture.path("other.jsonl.zst"),
        ];
        let diagnostics = Diagnostics::new("codex");
        let plan = CodexReplayPlan::for_selection(
            &[(fixture.root(), selected.as_slice())],
            std::iter::once((fixture.root(), available.as_slice()))
                .inspect(|_| std::fs::remove_file(&selected[0]).unwrap()),
            true,
            &diagnostics,
        );
        assert_eq!(
            plan.parent_by_child[&selected[0]].path.as_ref(),
            Some(&parent)
        );
        assert!(plan.usage_by_parent.contains_key(&parent));
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::ReadInterrupted),
            0
        );
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
    }
}
