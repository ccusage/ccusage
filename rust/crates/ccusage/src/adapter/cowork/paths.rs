use std::path::{Path, PathBuf};

use crate::Result;

pub(crate) fn cowork_paths() -> Result<Vec<PathBuf>> {
    if let Ok(env_paths) = std::env::var("COWORK_CONFIG_DIR") {
        return cowork_paths_from_env(&env_paths);
    }
    Ok(cowork_paths_from_root(&default_sessions_root()?))
}

fn default_sessions_root() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var("LOCALAPPDATA")
            .map_err(|_| crate::cli_error("LOCALAPPDATA environment variable is not set"))?;
        Ok(PathBuf::from(local_app_data)
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("Claude")
            .join("local-agent-mode-sessions"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = crate::home::home_dir()
            .ok_or_else(|| crate::cli_error("home directory is not set"))?;
        Ok(home.join("Library/Application Support/Claude/local-agent-mode-sessions"))
    }
}

fn cowork_paths_from_env(env_paths: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = crate::fast::FxHashSet::default();
    for raw in env_paths
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let path = crate::adapter::claude::normalize_claude_config_path(raw);
        if path.join("projects").is_dir() {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
            continue;
        }
        if !is_cowork_sessions_root(&path) {
            continue;
        }
        for discovered in cowork_paths_from_root(&path) {
            if seen.insert(discovered.clone()) {
                paths.push(discovered);
            }
        }
    }
    if paths.is_empty() {
        return Err(crate::cli_error(format!(
            "No valid Cowork data directories found in COWORK_CONFIG_DIR. Expected each path to be a Cowork local-agent-mode-sessions directory, a .claude config directory containing 'projects/', or the 'projects/' directory itself: {env_paths}"
        )));
    }
    paths.sort_by_cached_key(|path| path.to_string_lossy().into_owned());
    Ok(paths)
}

fn is_cowork_sessions_root(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "local-agent-mode-sessions")
}

pub(crate) fn cowork_paths_from_root(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_cowork_paths(root, &mut paths);
    paths.sort_by_cached_key(|path| path.to_string_lossy().into_owned());
    paths
}

fn collect_cowork_paths(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("local_"))
        {
            let config = path.join(".claude");
            if config.join("projects").is_dir() {
                paths.push(config);
                continue;
            }
        }
        collect_cowork_paths(&path, paths);
    }
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;

    use super::cowork_paths_from_root;

    #[test]
    fn discovers_local_agent_mode_claude_config_dirs() {
        let fixture = fs_fixture!({
            "workspace-a/session-a/local_111/.claude/projects/project-a/session-a.jsonl": "",
            "workspace-a/session-a/local_222/.claude/projects/project-b/session-b.jsonl": "",
            "workspace-a/session-a/not-local/.claude/projects/project-c/session-c.jsonl": "",
            "workspace-b/session-b/local_333/no-claude/projects/project-d/session-d.jsonl": "",
        });

        let paths = cowork_paths_from_root(fixture.root());

        assert_eq!(
            paths,
            vec![
                fixture.path("workspace-a/session-a/local_111/.claude"),
                fixture.path("workspace-a/session-a/local_222/.claude"),
            ]
        );
    }

    #[test]
    fn accepts_direct_claude_and_projects_env_paths() {
        let fixture = fs_fixture!({
            "direct/.claude/projects/project-a/session-a.jsonl": "",
            "other/.claude/projects/project-b/session-b.jsonl": "",
        });

        let paths = super::cowork_paths_from_env(&format!(
            "{},{}",
            fixture.path("direct/.claude").display(),
            fixture.path("other/.claude/projects").display()
        ))
        .unwrap();

        assert_eq!(
            paths,
            vec![
                fixture.path("direct/.claude"),
                fixture.path("other/.claude"),
            ]
        );
    }

    #[test]
    fn sorts_mixed_env_paths_globally() {
        let fixture = fs_fixture!({
            "local-agent-mode-sessions/local_b/.claude/projects/project-b/session-b.jsonl": "",
            "local-agent-mode-sessions/local_c/.claude/projects/project-c/session-c.jsonl": "",
            "a-direct/.claude/projects/project-a/session-a.jsonl": "",
        });

        let paths = super::cowork_paths_from_env(&format!(
            "{},{}",
            fixture.path("local-agent-mode-sessions").display(),
            fixture.path("a-direct/.claude").display()
        ))
        .unwrap();

        assert_eq!(
            paths,
            vec![
                fixture.path("a-direct/.claude"),
                fixture.path("local-agent-mode-sessions/local_b/.claude"),
                fixture.path("local-agent-mode-sessions/local_c/.claude"),
            ]
        );
    }

    #[test]
    fn rejects_arbitrary_env_roots_before_recursive_discovery() {
        let fixture = fs_fixture!({
            "home/local_111/.claude/projects/project-a/session-a.jsonl": "",
        });

        let error = super::cowork_paths_from_env(&fixture.path("home").display().to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("No valid Cowork data directories found in COWORK_CONFIG_DIR"));
    }

    #[test]
    fn dedupes_equivalent_direct_claude_and_projects_env_paths() {
        let fixture = fs_fixture!({
            "direct/.claude/projects/project-a/session-a.jsonl": "",
        });

        let paths = super::cowork_paths_from_env(&format!(
            "{},{}",
            fixture.path("direct/.claude").display(),
            fixture.path("direct/.claude/projects").display()
        ))
        .unwrap();

        assert_eq!(paths, vec![fixture.path("direct/.claude")]);
    }

    #[test]
    fn rejects_invalid_cowork_env_paths() {
        let fixture = fs_fixture!({
            "empty": "",
        });

        let error = super::cowork_paths_from_env(&fixture.path("empty").display().to_string())
            .unwrap_err()
            .to_string();

        assert!(error.contains("No valid Cowork data directories found in COWORK_CONFIG_DIR"));
    }
}
