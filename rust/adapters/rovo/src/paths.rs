use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::{Result, collect_files_with_extension, path_utils::expand_home_path};

pub(super) const ROVO_DATA_DIR_ENV: &str = "ROVO_DATA_DIR";
const ROVO_SESSION_FILE_NAME: &str = "session_context.json";
const ROVO_CONFIG_FILE_NAME: &str = "config.yml";

fn roots() -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(ROVO_DATA_DIR_ENV) {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(raw);
            if path.is_dir() && seen.insert(path.clone()) {
                roots.push(path);
            }
        }
        return Ok(roots);
    }

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".rovodev");
        if path.is_dir() {
            roots.push(path);
        }
    }
    Ok(roots)
}

pub(super) fn discover_session_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in roots()? {
        let mut candidates = Vec::new();
        collect_files_with_extension(&root, "json", &mut candidates);
        // Rovo Dev lets users move the sessions directory outside ~/.rovodev
        // via `sessions.persistenceDir` in config.yml; scan it too when it is
        // not already covered by the recursive walk above.
        if let Some(persistence_dir) = persistence_dir_from_config(&root)
            && !persistence_dir.starts_with(&root)
            && persistence_dir.is_dir()
        {
            collect_files_with_extension(&persistence_dir, "json", &mut candidates);
        }
        files.extend(candidates.into_iter().filter(|file| {
            file.file_name().and_then(|name| name.to_str()) == Some(ROVO_SESSION_FILE_NAME)
        }));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn persistence_dir_from_config(root: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(root.join(ROVO_CONFIG_FILE_NAME)).ok()?;
    persistence_dir_from_yaml(&content)
}

/// Minimal scan for `sessions.persistenceDir` so the adapter does not need a
/// YAML dependency: find the top-level `sessions:` block, then the first
/// `persistenceDir:` entry indented under it.
fn persistence_dir_from_yaml(content: &str) -> Option<PathBuf> {
    let mut in_sessions = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            // Tolerate trailing whitespace or a comment after the key so
            // hand-edited configs (`sessions:  # session settings`) still open
            // the block.
            in_sessions = trimmed
                .strip_prefix("sessions:")
                .is_some_and(|rest| rest.trim().is_empty() || rest.trim_start().starts_with('#'));
            continue;
        }
        if !in_sessions {
            continue;
        }
        if let Some(raw) = trimmed.strip_prefix("persistenceDir:") {
            let value = raw
                .split(" #")
                .next()
                .unwrap_or(raw)
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if value.is_empty() {
                return None;
            }
            return Some(expand_home_path(value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_session_context_files_under_env_root() {
        let fixture = fs_fixture!({
            "sessions/session-a/session_context.json": "{}",
            "sessions/session-a/metadata.json": "{}",
            "sessions/nested/session-b/session_context.json": "{}",
            "mcp.json": "{}",
        });
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.root());
        let files = discover_session_files().unwrap();

        assert_eq!(
            files,
            vec![
                fixture.path("sessions/nested/session-b/session_context.json"),
                fixture.path("sessions/session-a/session_context.json"),
            ]
        );
    }

    #[test]
    fn follows_sessions_persistence_dir_from_config_yml() {
        let fixture = fs_fixture!({
            "rovodev/config.yml": "",
            "workspace/.rovodev/sessions/session-c/session_context.json": "{}",
        });
        let persistence_dir = fixture.path("workspace/.rovodev/sessions");
        let _ = fixture.write_file(
            "rovodev/config.yml",
            format!(
                "version: 1\nsessions:\n  persistenceDir: {}\n  retentionDays: 30\n",
                persistence_dir.display()
            ),
        );
        let _cleanup = EnvVarGuard::set(ROVO_DATA_DIR_ENV, fixture.path("rovodev"));
        let files = discover_session_files().unwrap();

        assert_eq!(
            files,
            vec![fixture.path("workspace/.rovodev/sessions/session-c/session_context.json")]
        );
    }

    #[test]
    fn parses_persistence_dir_only_under_the_sessions_block() {
        let yaml = "logging:\n  persistenceDir: /wrong\nsessions:\n  # a comment\n  persistenceDir: \"/data/rovo sessions\" # trailing\n";

        assert_eq!(
            persistence_dir_from_yaml(yaml),
            Some(PathBuf::from("/data/rovo sessions"))
        );
        assert_eq!(persistence_dir_from_yaml("sessions:\n  other: 1\n"), None);
    }

    #[test]
    fn opens_the_sessions_block_despite_trailing_comment_or_whitespace() {
        assert_eq!(
            persistence_dir_from_yaml("sessions: # session settings\n  persistenceDir: /data/a\n"),
            Some(PathBuf::from("/data/a"))
        );
        assert_eq!(
            persistence_dir_from_yaml("sessions:   \n  persistenceDir: /data/b\n"),
            Some(PathBuf::from("/data/b"))
        );
        assert_eq!(
            persistence_dir_from_yaml("sessionsOld:\n  persistenceDir: /data/c\n"),
            None
        );
    }

    #[test]
    fn expands_home_in_persistence_dir() {
        let fixture = fs_fixture!({});
        let _home = EnvVarGuard::set("HOME", fixture.root());

        assert_eq!(
            persistence_dir_from_yaml("sessions:\n  persistenceDir: ~/rovo-sessions\n"),
            Some(fixture.path("rovo-sessions"))
        );
    }
}
