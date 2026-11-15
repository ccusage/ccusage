use std::{
    collections::HashSet,
    env,
    path::{Component, Path, PathBuf},
};

use crate::Result;
use ccusage_adapter_common::{
    collect_files_with_extension, collect_files_with_extension_diagnosed,
};
use ccusage_core::Diagnostics;

const QWEN_DATA_DIR_ENV: &str = "QWEN_DATA_DIR";

fn paths() -> Result<Vec<PathBuf>> {
    let candidates = if let Ok(paths) = env::var(QWEN_DATA_DIR_ENV) {
        paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    } else {
        crate::home::home_dir()
            .map(|home| vec![home.join(".qwen")])
            .unwrap_or_default()
    };
    let mut seen = HashSet::new();
    Ok(candidates
        .into_iter()
        .filter(|path| path.is_dir())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

pub(super) fn discover_chat_files() -> Result<Vec<PathBuf>> {
    discover_chat_files_inner(None)
}

pub(super) fn discover_chat_files_with_diagnostics(
    diagnostics: &Diagnostics,
) -> Result<Vec<PathBuf>> {
    discover_chat_files_inner(Some(diagnostics))
}

fn discover_chat_files_inner(diagnostics: Option<&Diagnostics>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in paths()? {
        let projects = root.join("projects");
        if !projects.is_dir() {
            continue;
        }
        let mut root_files = Vec::new();
        if let Some(diagnostics) = diagnostics {
            collect_files_with_extension_diagnosed(
                &projects,
                "jsonl",
                &mut root_files,
                diagnostics,
            );
        } else {
            collect_files_with_extension(&projects, "jsonl", &mut root_files);
        }
        root_files.retain(|file| is_chat_file(&projects, file));
        files.extend(root_files);
    }
    files.sort();
    Ok(files)
}

fn is_chat_file(projects: &Path, file: &Path) -> bool {
    let Ok(relative) = file.strip_prefix(projects) else {
        return false;
    };
    let parts = relative.components().collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        [Component::Normal(project), Component::Normal(chats), Component::Normal(file)]
            if !project.is_empty()
                && chats.to_str() == Some("chats")
                && file.to_string_lossy().ends_with(".jsonl")
    ) || matches!(
        parts.as_slice(),
        [
            Component::Normal(project),
            Component::Normal(chats),
            Component::Normal(archive),
            Component::Normal(file)
        ] if !project.is_empty()
            && chats.to_str() == Some("chats")
            && archive.to_str() == Some("archive")
            && file.to_string_lossy().ends_with(".jsonl")
    )
}

pub(super) fn project_from_file(file: &Path) -> Option<String> {
    let parts = file.components().collect::<Vec<_>>();
    for window in parts.windows(4).rev() {
        if let [
            Component::Normal(projects),
            Component::Normal(project),
            Component::Normal(chats),
            Component::Normal(_),
        ] = window
            && projects.to_str() == Some("projects")
            && chats.to_str() == Some("chats")
        {
            return Some(project.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::source::{
        ProgressOwner, SourceOptions, UnifiedRequest, UsageSource,
    };
    use ccusage_test_support::{EnvVarGuard, Fixture};

    use super::{discover_chat_files, project_from_file};
    use crate::{
        SOURCE,
        cli::{AgentReportKind, CostMode, SharedArgs},
    };

    #[test]
    fn discovers_direct_and_flat_archive_chats_only() {
        let fixture = Fixture::new();
        let direct = fixture.write_file("qwen/projects/project-a/chats/live.jsonl", "");
        let archived =
            fixture.write_file("qwen/projects/project-a/chats/archive/archived.jsonl", "");
        let _ = fixture.write_file(
            "qwen/projects/project-a/chats/archive/nested/ignored.jsonl",
            "",
        );
        let _ = fixture.write_file("qwen/projects/project-a/chats/other/ignored.jsonl", "");
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.path("qwen"));

        assert_eq!(discover_chat_files().unwrap(), [archived.clone(), direct]);
        assert_eq!(project_from_file(&archived).as_deref(), Some("project-a"));
    }

    #[test]
    fn archived_chats_keep_session_ids_and_remain_detected_outside_the_window() {
        let fixture = Fixture::new();
        let _ = fixture.write_file(
            "qwen/projects/project-a/chats/archive/arch-0002.jsonl",
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T03:00:00.000Z","sessionId":"arch-0002","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
        );
        let _ = fixture.write_file(
            "qwen/projects/project-a/chats/archive/file-stem.jsonl",
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T04:00:00.000Z","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
        );
        let _guard = EnvVarGuard::set("QWEN_DATA_DIR", fixture.path("qwen"));
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            offline: true,
            ..SharedArgs::default()
        };

        let entries = crate::load_entries(&shared).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id.as_ref(), "arch-0002");
        assert_eq!(entries[0].project_path.as_ref(), "project-a");
        assert_eq!(entries[1].session_id.as_ref(), "project-a-file-stem");
        assert_eq!(entries[1].project_path.as_ref(), "project-a");

        let outside_window = SharedArgs {
            since: Some("20270101".to_string()),
            ..shared
        };
        let report = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &outside_window,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
            })
            .unwrap();

        assert!(report.detected);
        assert!(report.rows.is_empty());
    }
}
