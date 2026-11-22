use std::path::{Component, Path, PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use crate::Result;
use ccusage_adapter_common::collect_files_with_extension;
use ccusage_core::Diagnostics;

const QWEN_DATA_DIR_ENV: &str = "QWEN_DATA_DIR";

fn paths() -> Result<Vec<PathBuf>> {
    Ok(roots::resolve(RootsEnv::List(QWEN_DATA_DIR_ENV), || {
        crate::cli::home::home_dir()
            .into_iter()
            .flat_map(|home| [".qwen"].map(|path| home.join(path)))
            .collect()
    }))
}

pub(super) fn discover_chat_files(diagnostics: &Diagnostics) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in paths()? {
        let projects = root.join("projects");
        if !projects.is_dir() {
            continue;
        }
        let mut root_files = Vec::new();
        collect_files_with_extension(&projects, "jsonl", &mut root_files, diagnostics);
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
    let [
        Component::Normal(project),
        Component::Normal(chats),
        rest @ ..,
    ] = parts.as_slice()
    else {
        return false;
    };
    if project.is_empty() || chats.to_str() != Some("chats") {
        return false;
    }
    let file = match rest {
        [Component::Normal(file)] => file,
        // NOTE(2026-11-15, #2090): Qwen Code moves finished sessions into the flat chats/archive/ directory without rewriting them. Discover that directory to include archived usage.
        [Component::Normal(archive), Component::Normal(file)]
            if archive.to_str() == Some("archive") =>
        {
            file
        }
        _ => return false,
    };
    file.to_string_lossy().ends_with(".jsonl")
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
        EntrySource, LoadRequest, ProgressOwner, SourceOptions, UsageSource,
    };
    use ccusage_test_support::{Environment, Fixture};

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
        fixture.write_file(
            "qwen/projects/project-a/chats/archive/nested/ignored.jsonl",
            "",
        );
        fixture.write_file("qwen/projects/project-a/chats/other/ignored.jsonl", "");
        Environment::with_var("QWEN_DATA_DIR", fixture.path("qwen")).run(|| {
            assert_eq!(
                discover_chat_files(&ccusage_core::Diagnostics::new("qwen")).unwrap(),
                [archived.clone(), direct]
            );
            assert_eq!(project_from_file(&archived).as_deref(), Some("project-a"));
        })
    }

    #[test]
    fn archived_chats_keep_session_ids_and_remain_detected_outside_the_window() {
        let fixture = Fixture::new();
        fixture.write_file(
            "qwen/projects/project-a/chats/archive/arch-0002.jsonl",
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T03:00:00.000Z","sessionId":"arch-0002","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
        );
        fixture.write_file(
            "qwen/projects/project-a/chats/archive/file-stem.jsonl",
            r#"{"type":"assistant","model":"qwen3-coder-plus","timestamp":"2026-02-24T04:00:00.000Z","usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
        );
        Environment::with_var("QWEN_DATA_DIR", fixture.path("qwen")).run(|| {
            let shared = SharedArgs {
                mode: CostMode::Display,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                offline: true,
                ..SharedArgs::default()
            };

            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                None,
            )
            .unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].session_id.as_ref(), "arch-0002");
            assert_eq!(entries[0].project_path.as_ref(), "project-a");
            assert_eq!(entries[1].session_id.as_ref(), "project-a-file-stem");
            assert_eq!(entries[1].project_path.as_ref(), "project-a");

            ccusage_adapter_common::parity::assert_focused_matches_unified(
                &SOURCE,
                ccusage_core::cli::AgentId::Qwen,
                &shared,
            );
            for kind in [
                AgentReportKind::Daily,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ] {
                let rows = ccusage_adapter_common::source::EntrySource::summarize(
                    &crate::SOURCE,
                    &entries,
                    ccusage_adapter_common::source::SummaryKind::for_report(
                        kind,
                        ccusage_core::cli::WeekDay::Monday,
                    ),
                )
                .unwrap();
                let report = crate::SOURCE.report_json(&rows, kind);
                assert_eq!(report["totals"]["inputTokens"], 20);
                assert_eq!(report["totals"]["outputTokens"], 10);
            }

            let outside_window = SharedArgs {
                since: Some("20270101".parse().unwrap()),
                ..shared
            };
            let report = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &outside_window,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();

            assert!(report.detected);
            assert!(report.rows.is_empty());
        })
    }
}
