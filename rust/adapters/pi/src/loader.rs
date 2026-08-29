use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, collect_files_with_extension, debug_log,
    parse_tz, read_files_parallel,
};

use super::{parser, paths};

pub fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("pi-agent"),
        shared.json,
        || load_entries_inner(shared, custom_path, pricing),
    )
}

fn load_entries_inner(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries_from_paths(
        shared,
        paths::paths(custom_path)?,
        pricing,
        PiLoadScope::Default,
    )
}

#[doc(hidden)]
pub fn load_entries_for_store_path(
    shared: &SharedArgs,
    store_path: &str,
    store_name: &str,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries_for_store_paths(
        shared,
        paths::named_store_paths(store_path)?,
        store_name,
        pricing,
    )
}

pub fn load_entries_for_store_paths(
    shared: &SharedArgs,
    store_paths: Vec<PathBuf>,
    store_name: &str,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries_from_paths(
        shared,
        store_paths,
        pricing,
        PiLoadScope::Named { store_name },
    )
}

#[derive(Clone, Copy)]
enum PiLoadScope<'a> {
    Default,
    Named { store_name: &'a str },
}

impl<'a> PiLoadScope<'a> {
    fn store_name(self) -> &'a str {
        match self {
            Self::Default => "pi",
            Self::Named { store_name } => store_name,
        }
    }

    fn debug_label(self) -> String {
        match self {
            Self::Default => "pi".to_string(),
            Self::Named { store_name } => format!("pi-format store '{store_name}'"),
        }
    }
}

fn load_entries_from_paths(
    shared: &SharedArgs,
    paths: Vec<PathBuf>,
    pricing: Option<&PricingMap>,
    scope: PiLoadScope<'_>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut discovered_files = Vec::new();
    let mut loaded_files = Vec::new();
    for path in paths {
        let mut files = Vec::new();
        collect_files_with_extension(&path, "jsonl", &mut files);
        // Read session files in parallel; the first-wins dedup runs sequentially
        // over the original file order so the surviving record per id matches the
        // single-threaded read.
        let loaded = read_files_parallel(&files, shared.single_thread, |file| {
            let result = match scope {
                PiLoadScope::Default => {
                    parser::read_session_file_data(file, tz.as_ref(), shared.mode, pricing)
                }
                PiLoadScope::Named { store_name } => parser::read_session_file_data_for_store(
                    file,
                    &path,
                    tz.as_ref(),
                    shared.mode,
                    pricing,
                    store_name,
                ),
            };
            match result {
                Ok(data) => Some(data),
                Err(error) => {
                    let label = scope.debug_label();
                    debug_log(
                        shared,
                        format!(
                            "Failed to read {label} session file {}: {error}",
                            file.display()
                        ),
                    );
                    None
                }
            }
        });
        discovered_files.extend(files.into_iter().map(|file| DiscoveredFile { path: file }));
        loaded_files.extend(loaded);
    }

    let replay_plan = PiReplayPlan::new(&discovered_files, &loaded_files);
    for (index, (_file, data)) in discovered_files.into_iter().zip(loaded_files).enumerate() {
        let Some(data) = data else {
            continue;
        };
        for entry in data
            .entries
            .into_iter()
            .skip(replay_plan.skip_prefix(index))
        {
            let id = match scope {
                PiLoadScope::Default => parser::entry_id(&entry),
                PiLoadScope::Named { .. } => parser::entry_id_for_store(scope.store_name(), &entry),
            };
            if seen.insert(id) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

struct DiscoveredFile {
    path: PathBuf,
}

struct ParentReplay {
    parent_index: usize,
    fork_timestamp: crate::TimestampMs,
}

struct PiReplayPlan {
    skip_by_file: HashMap<usize, usize>,
}

impl PiReplayPlan {
    fn new(files: &[DiscoveredFile], loaded: &[Option<parser::PiSessionData>]) -> Self {
        let mut files_by_path = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            files_by_path
                .entry(normalize_path(&file.path))
                .or_insert(index);
        }

        let mut invalid_lineage = HashSet::new();
        for (index, data) in loaded.iter().enumerate() {
            let Some(header) = data.as_ref().and_then(|data| data.header.as_ref()) else {
                invalid_lineage.insert(index);
                continue;
            };
            if header.timestamp.is_none() || header.parent_session_is_malformed {
                invalid_lineage.insert(index);
            }
        }

        let mut parent_by_child = HashMap::new();
        for (child_index, data) in loaded.iter().enumerate() {
            let Some(header) = data.as_ref().and_then(|data| data.header.as_ref()) else {
                continue;
            };
            let Some(parent_path) = header.parent_session.as_ref() else {
                continue;
            };
            let Some(fork_timestamp) = header.timestamp else {
                continue;
            };
            let Some(parent_index) =
                resolve_parent_index(parent_path, &files[child_index].path, &files_by_path)
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
mod tests {
    use super::*;
    use crate::cli::{CostMode, SharedArgs};
    use ccusage_test_support::Fixture;
    use serde_json::json;
    use std::path::Path;

    fn session_line(id: &str, timestamp: &str, parent: Option<&Path>) -> String {
        let mut line = json!({
            "type": "session",
            "id": id,
            "timestamp": timestamp,
        });
        if let Some(parent) = parent {
            line["parentSession"] = json!(parent.to_string_lossy().to_string());
        }
        line.to_string()
    }

    fn usage_line_with_model_and_total(
        timestamp: &str,
        model: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        total_tokens: u64,
    ) -> String {
        json!({
            "type": "message",
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "model": model,
                "usage": {
                    "input": input,
                    "output": output,
                    "cacheRead": cache_read,
                    "cacheWrite": cache_write,
                    "totalTokens": total_tokens,
                },
            },
        })
        .to_string()
    }

    fn usage_line(
        timestamp: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> String {
        usage_line_with_model_and_total(
            timestamp,
            "gpt-5",
            input,
            output,
            cache_read,
            cache_write,
            input + output + cache_read + cache_write,
        )
    }

    fn linked_usage_line(id: &str, parent_id: Option<&str>, timestamp: &str, input: u64) -> String {
        let mut line = json!({
            "type": "message",
            "id": id,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "model": "gpt-5",
                "usage": {
                    "input": input,
                    "output": 10,
                    "cacheRead": 20,
                    "cacheWrite": 3,
                    "totalTokens": input + 33,
                },
            },
        });
        line["parentId"] = parent_id.map_or(serde_json::Value::Null, |parent_id| json!(parent_id));
        line.to_string()
    }

    fn usage_line_with_display_cost(timestamp: &str, display_cost: f64) -> String {
        json!({
            "type": "message",
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "model": "gpt-5",
                "usage": {
                    "input": 10,
                    "output": 20,
                    "cacheRead": 30,
                    "cacheWrite": 40,
                    "totalTokens": 100,
                    "cost": {"total": display_cost},
                },
            },
        })
        .to_string()
    }

    #[test]
    fn skips_replayed_parent_prefix_but_keeps_child_usage() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
            ]
            .join("\n"),
        );
        let _child_a = fixture.write_file(
            "sessions/project-a/child-a.jsonl",
            [
                session_line("child-a", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
            ]
            .join("\n"),
        );
        let _child_b = fixture.write_file(
            "sessions/project-a/child-b.jsonl",
            [
                session_line("child-b", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 999),
                usage_line("2026-01-03T01:00:00.000Z", 70, 7, 8, 2),
            ]
            .join("\n"),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 6, "single_thread={single_thread}");
            let child_a_entries = entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child-a")
                .collect::<Vec<_>>();
            assert_eq!(child_a_entries.len(), 1, "single_thread={single_thread}");
            assert_eq!(
                child_a_entries[0].data.message.usage.input_tokens, 50,
                "single_thread={single_thread}"
            );
            let child_b_entries = entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child-b")
                .collect::<Vec<_>>();
            assert_eq!(child_b_entries.len(), 2, "single_thread={single_thread}");
            assert_eq!(
                child_b_entries[0]
                    .data
                    .message
                    .usage
                    .cache_creation_input_tokens,
                999,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn skips_the_copied_active_parent_branch_after_an_abandoned_sibling() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                linked_usage_line("a", None, "2026-01-02T10:00:00.000Z", 100),
                linked_usage_line("x", Some("a"), "2026-01-02T11:00:00.000Z", 200),
                linked_usage_line("y", Some("a"), "2026-01-02T12:00:00.000Z", 300),
                linked_usage_line("z", Some("y"), "2026-01-02T13:00:00.000Z", 400),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                linked_usage_line("copy-a", None, "2026-01-02T10:00:00.000Z", 100),
                linked_usage_line("copy-y", Some("copy-a"), "2026-01-02T12:00:00.000Z", 300),
                linked_usage_line("copy-z", Some("copy-y"), "2026-01-02T13:00:00.000Z", 400),
                linked_usage_line(
                    "child-only",
                    Some("copy-z"),
                    "2026-01-03T01:00:00.000Z",
                    500,
                ),
            ]
            .join("\n"),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 5, "single_thread={single_thread}");
            assert!(entries.iter().any(|entry| {
                entry.session_id.as_ref() == "root" && entry.data.message.usage.input_tokens == 200
            }));
            let child_entries = entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child")
                .collect::<Vec<_>>();
            assert_eq!(child_entries.len(), 1, "single_thread={single_thread}");
            assert_eq!(
                child_entries[0].data.message.usage.input_tokens, 500,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn fails_open_for_missing_malformed_and_unrelated_same_token_sessions() {
        let fixture = Fixture::new();
        let missing_parent = fixture.path("sessions/project-a/missing.jsonl");
        let malformed_parent = fixture.path("sessions/project-a/malformed-parent.jsonl");
        let outside_parent = fixture.write_file(
            "outside/root.jsonl",
            [
                session_line("outside", "2026-01-01T00:00:00.000Z", None),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/missing-child.jsonl",
            [
                session_line(
                    "missing-child",
                    "2026-01-03T00:00:00.000Z",
                    Some(&missing_parent),
                ),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/outside-parent-child.jsonl",
            [
                session_line(
                    "outside-parent-child",
                    "2026-01-03T00:00:00.000Z",
                    Some(&outside_parent),
                ),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/malformed-parent.jsonl",
            [
                "not json".to_string(),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/malformed-parent-child.jsonl",
            [
                session_line(
                    "malformed-parent-child",
                    "2026-01-03T00:00:00.000Z",
                    Some(&malformed_parent),
                ),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/malformed-parent-grandchild.jsonl",
            [
                session_line(
                    "malformed-parent-grandchild",
                    "2026-01-04T00:00:00.000Z",
                    Some(&fixture.path("sessions/project-a/malformed-parent-child.jsonl")),
                ),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/malformed-child.jsonl",
            [
                "not json".to_string(),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/regular-a.jsonl",
            usage_line("2026-01-02T11:00:00.000Z", 100, 10, 20, 3),
        );
        let _ = fixture.write_file(
            "sessions/project-a/regular-b.jsonl",
            usage_line("2026-01-02T11:00:00.000Z", 100, 10, 20, 3),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 8, "single_thread={single_thread}");
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "missing-child")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "outside-parent-child")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "regular-a")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "regular-b")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "malformed-parent-grandchild")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn fails_open_for_self_referential_and_cyclic_sessions() {
        let fixture = Fixture::new();
        let self_path = fixture.path("sessions/project-a/self.jsonl");
        let cycle_a = fixture.path("sessions/project-a/cycle-a.jsonl");
        let cycle_b = fixture.path("sessions/project-a/cycle-b.jsonl");
        let _ = fixture.write_file(
            "sessions/project-a/self.jsonl",
            [
                session_line("self", "2026-01-03T00:00:00.000Z", Some(&self_path)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/cycle-a.jsonl",
            [
                session_line("cycle-a", "2026-01-03T00:00:00.000Z", Some(&cycle_b)),
                usage_line("2026-01-02T10:00:00.000Z", 200, 20, 30, 4),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/cycle-b.jsonl",
            [
                session_line("cycle-b", "2026-01-03T00:00:00.000Z", Some(&cycle_a)),
                usage_line("2026-01-02T10:00:00.000Z", 200, 20, 30, 4),
            ]
            .join("\n"),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 3, "single_thread={single_thread}");
        }
    }

    #[test]
    fn uses_raw_parent_stream_for_nested_forks() {
        let fixture = Fixture::new();
        let root = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
            ]
            .join("\n"),
        );
        let child = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&root)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-03T01:00:00.000Z", 300, 30, 40, 5),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/grandchild.jsonl",
            [
                session_line("grandchild", "2026-01-04T00:00:00.000Z", Some(&child)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-02T11:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-03T01:00:00.000Z", 300, 30, 40, 5),
                usage_line("2026-01-04T01:00:00.000Z", 400, 40, 50, 6),
            ]
            .join("\n"),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 4, "single_thread={single_thread}");
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "child")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == "grandchild")
                    .count(),
                1,
                "single_thread={single_thread}"
            );
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.session_id.as_ref() == "grandchild")
                    .unwrap()
                    .data
                    .message
                    .usage
                    .input_tokens,
                400,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn compares_every_effective_usage_field_before_suppressing_replay() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_model_and_total(
                    "2026-01-02T10:00:00.000Z",
                    "gpt-5",
                    10,
                    20,
                    30,
                    40,
                    100,
                ),
            ]
            .join("\n"),
        );
        let variants = [
            ("input", "gpt-5", 11, 20, 30, 40, 101),
            ("output", "gpt-5", 10, 21, 30, 40, 101),
            ("cache-read", "gpt-5", 10, 20, 31, 40, 101),
            ("cache-write", "gpt-5", 10, 20, 30, 41, 101),
            ("model", "gpt-5-mini", 10, 20, 30, 40, 100),
            ("total-fallback", "gpt-5", 10, 20, 30, 40, 110),
        ];
        for (name, model, input, output, cache_read, cache_write, total_tokens) in variants {
            let _ = fixture.write_file(
                format!("sessions/project-a/child-{name}.jsonl"),
                [
                    session_line(name, "2026-01-03T00:00:00.000Z", Some(&parent)),
                    usage_line_with_model_and_total(
                        "2026-01-02T10:00:00.000Z",
                        model,
                        input,
                        output,
                        cache_read,
                        cache_write,
                        total_tokens,
                    ),
                    usage_line("2026-01-03T01:00:00.000Z", 1, 2, 3, 4),
                ]
                .join("\n"),
            );
        }

        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: false,
            ..SharedArgs::default()
        };
        let entries = load_entries_from_paths(
            &shared,
            vec![fixture.path("sessions")],
            None,
            PiLoadScope::Default,
        )
        .unwrap();

        assert_eq!(entries.len(), 1 + variants.len() * 2);
        for (name, ..) in variants {
            let session_id = format!("child-{name}");
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.session_id.as_ref() == session_id)
                    .count(),
                2,
                "variant={name}"
            );
        }
    }

    #[test]
    fn keeps_child_usage_when_display_cost_differs() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 1.0),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 2.0),
            ]
            .join("\n"),
        );

        for mode in [CostMode::Display, CostMode::Auto] {
            let shared = SharedArgs {
                mode,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 2, "mode={mode:?}");
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.session_id.as_ref() == "child")
                    .unwrap()
                    .cost,
                2.0,
                "mode={mode:?}"
            );
        }
    }

    #[test]
    fn ignores_stored_display_cost_when_calculating_replay_identity() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 1.0),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 2.0),
            ]
            .join("\n"),
        );
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            ..SharedArgs::default()
        };
        let entries = load_entries_from_paths(
            &shared,
            vec![fixture.path("sessions")],
            None,
            PiLoadScope::Default,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "root");
    }

    #[test]
    fn treats_signed_zero_display_costs_as_equal_replay_identity() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 0.0),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", -0.0),
            ]
            .join("\n"),
        );

        for mode in [CostMode::Display, CostMode::Auto] {
            let shared = SharedArgs {
                mode,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 1, "mode={mode:?}");
            assert_eq!(entries[0].session_id.as_ref(), "root", "mode={mode:?}");
        }
    }

    #[test]
    fn treats_missing_and_zero_display_costs_as_equal_replay_identity() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_model_and_total(
                    "2026-01-02T10:00:00.000Z",
                    "gpt-5",
                    10,
                    20,
                    30,
                    40,
                    100,
                ),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_display_cost("2026-01-02T10:00:00.000Z", 0.0),
            ]
            .join("\n"),
        );
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..SharedArgs::default()
        };
        let entries = load_entries_from_paths(
            &shared,
            vec![fixture.path("sessions")],
            None,
            PiLoadScope::Default,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "root");
    }

    #[test]
    fn dedupes_replay_when_raw_total_underreports_billable_usage() {
        let fixture = Fixture::new();
        let parent = fixture.write_file(
            "sessions/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line_with_model_and_total(
                    "2026-01-02T10:00:00.000Z",
                    "gpt-5",
                    10,
                    20,
                    30,
                    40,
                    100,
                ),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "sessions/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line_with_model_and_total(
                    "2026-01-02T10:00:00.000Z",
                    "gpt-5",
                    10,
                    20,
                    30,
                    40,
                    99,
                ),
                usage_line("2026-01-03T01:00:00.000Z", 1, 2, 3, 4),
            ]
            .join("\n"),
        );

        for single_thread in [true, false] {
            let shared = SharedArgs {
                mode: CostMode::Display,
                single_thread,
                ..SharedArgs::default()
            };
            let entries = load_entries_from_paths(
                &shared,
                vec![fixture.path("sessions")],
                None,
                PiLoadScope::Default,
            )
            .unwrap();

            assert_eq!(entries.len(), 2, "single_thread={single_thread}");
            let child_entries = entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child")
                .collect::<Vec<_>>();
            assert_eq!(child_entries.len(), 1, "single_thread={single_thread}");
            assert_eq!(
                child_entries[0].data.message.usage.input_tokens, 1,
                "single_thread={single_thread}"
            );
        }
    }

    #[test]
    fn matches_parent_files_across_multiple_paths_of_one_named_store() {
        let fixture = Fixture::new();
        let first_store = fixture.create_dir_all("first");
        let second_store = fixture.create_dir_all("second");
        let parent = fixture.write_file(
            "first/project-a/root.jsonl",
            [
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "second/project-a/child.jsonl",
            [
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T01:00:00.000Z", 50, 5, 6, 1),
            ]
            .join("\n"),
        );

        let shared = SharedArgs {
            mode: CostMode::Display,
            single_thread: false,
            ..SharedArgs::default()
        };
        let entries =
            load_entries_for_store_paths(&shared, vec![first_store, second_store], "omp", None)
                .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.session_id.as_ref() == "child")
                .count(),
            1
        );
        assert_eq!(entries[1].model.as_deref(), Some("[omp] gpt-5"));
    }
}
