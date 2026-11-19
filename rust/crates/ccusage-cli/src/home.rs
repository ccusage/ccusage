use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

pub fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(
        env::var_os("HOME"),
        env::var_os("USERPROFILE"),
        env::var_os("HOMEDRIVE"),
        env::var_os("HOMEPATH"),
    )
}

pub fn path_identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn expand_home_path(raw: &str) -> PathBuf {
    expand_home(raw, home_dir().as_deref())
}

pub fn parse_paths(
    raw: &OsStr,
    separator: Option<char>,
    home: Option<&Path>,
) -> Option<Vec<PathBuf>> {
    let Some(raw) = raw.to_str() else {
        return Some(vec![PathBuf::from(raw)]);
    };
    let entries = raw
        .split(|ch| separator == Some(ch))
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return None;
    }
    Some(
        entries
            .into_iter()
            .filter_map(|raw| {
                if home.is_none() && (raw == "~" || raw.starts_with("~/")) {
                    return None;
                }
                Some(expand_home(raw, home))
            })
            .collect(),
    )
}

fn expand_home(raw: &str, home: Option<&Path>) -> PathBuf {
    match (raw.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(raw),
    }
}

/// The Claude Code directories that hold both the usage logs and the user's
/// `ccusage.json`, in priority order.
pub enum ClaudeConfigDirs {
    Env { value: String, dirs: Vec<PathBuf> },
    Default(Vec<PathBuf>),
    NoHome,
}

impl ClaudeConfigDirs {
    pub fn into_dirs(self) -> Vec<PathBuf> {
        match self {
            Self::Env { dirs, .. } | Self::Default(dirs) => dirs,
            Self::NoHome => Vec::new(),
        }
    }
}

pub fn claude_config_dirs() -> ClaudeConfigDirs {
    claude_config_dirs_from(
        env::var("CLAUDE_CONFIG_DIR").ok(),
        env::var_os("XDG_CONFIG_HOME"),
        home_dir(),
    )
}

fn claude_config_dirs_from(
    claude_config_dir: Option<String>,
    xdg_config_home: Option<OsString>,
    home: Option<PathBuf>,
) -> ClaudeConfigDirs {
    if let Some(value) = claude_config_dir
        && let Some(paths) = parse_paths(value.as_ref(), Some(','), home.as_deref())
    {
        let mut dirs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for path in paths {
            let dir = normalize_claude_config_dir(path);
            let identity = path_identity(&dir);
            if seen.insert(identity) {
                dirs.push(dir);
            }
        }
        return ClaudeConfigDirs::Env { value, dirs };
    }
    let Some(home) = home else {
        return ClaudeConfigDirs::NoHome;
    };
    let config_home = non_empty_path(xdg_config_home).unwrap_or_else(|| home.join(".config"));
    ClaudeConfigDirs::Default(vec![config_home.join("claude"), home.join(".claude")])
}

// NOTE(2026-11-16, #2107): Users can point CLAUDE_CONFIG_DIR at the projects directory itself. Resolve its parent before searching for ccusage.json.
fn normalize_claude_config_dir(path: PathBuf) -> PathBuf {
    if path.file_name().is_some_and(|name| name == "projects") && path.is_dir() {
        return path.parent().map(Path::to_path_buf).unwrap_or(path);
    }
    path
}

fn home_dir_from_env(
    home: Option<OsString>,
    user_profile: Option<OsString>,
    home_drive: Option<OsString>,
    home_path: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(path) = non_empty_path(home) {
        return Some(path);
    }
    if let Some(path) = non_empty_path(user_profile) {
        return Some(path);
    }
    let mut drive = home_drive?;
    let path = home_path?;
    if drive.is_empty() || path.is_empty() {
        return None;
    }
    drive.push(path);
    Some(PathBuf::from(drive))
}

fn non_empty_path(path: Option<OsString>) -> Option<PathBuf> {
    let path = path?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_canonical_identity_keeps_the_given_path() {
        let path = Path::new("/ccusage-missing-canonical-root/./store");
        assert_eq!(path_identity(path), path);
    }

    #[test]
    fn prefers_home_when_available() {
        let path = home_dir_from_env(
            Some(OsString::from("/home/user")),
            Some(OsString::from("C:\\Users\\runner")),
            None,
            None,
        );
        assert_eq!(path, Some(PathBuf::from("/home/user")));
    }

    #[test]
    fn falls_back_to_windows_user_profile_without_home() {
        let path = home_dir_from_env(None, Some(OsString::from("C:\\Users\\runner")), None, None);
        assert_eq!(path, Some(PathBuf::from("C:\\Users\\runner")));
    }

    #[test]
    fn falls_back_to_windows_home_drive_and_path() {
        let path = home_dir_from_env(
            None,
            None,
            Some(OsString::from("C:")),
            Some(OsString::from("\\Users\\runner")),
        );
        assert_eq!(path, Some(PathBuf::from("C:\\Users\\runner")));
    }

    fn dirs(resolved: ClaudeConfigDirs) -> Vec<PathBuf> {
        resolved.into_dirs()
    }

    #[test]
    fn claude_dirs_default_to_xdg_config_home_then_dot_claude() {
        let resolved = claude_config_dirs_from(
            None,
            Some(OsString::from("/custom/xdg")),
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(
            dirs(resolved),
            [
                PathBuf::from("/custom/xdg/claude"),
                PathBuf::from("/home/user/.claude"),
            ]
        );
    }

    #[test]
    fn claude_dirs_treat_unset_or_empty_xdg_config_home_as_dot_config() {
        for xdg_config_home in [None, Some(OsString::new())] {
            let resolved =
                claude_config_dirs_from(None, xdg_config_home, Some(PathBuf::from("/home/user")));
            assert_eq!(
                dirs(resolved),
                [
                    PathBuf::from("/home/user/.config/claude"),
                    PathBuf::from("/home/user/.claude"),
                ]
            );
        }
    }

    #[test]
    fn claude_dirs_without_home_or_env_are_reported_as_no_home() {
        let resolved = claude_config_dirs_from(None, Some(OsString::from("/custom/xdg")), None);
        assert!(matches!(resolved, ClaudeConfigDirs::NoHome));
    }

    #[test]
    fn blank_claude_directories_use_defaults() {
        for value in ["", "  ", " , , "] {
            let resolved = claude_config_dirs_from(
                Some(value.to_string()),
                None,
                Some(PathBuf::from("/home/user")),
            );
            assert_eq!(
                dirs(resolved),
                [
                    PathBuf::from("/home/user/.config/claude"),
                    PathBuf::from("/home/user/.claude"),
                ]
            );
        }
    }

    #[test]
    fn claude_dirs_expand_tilde_in_each_comma_separated_env_entry() {
        let resolved = claude_config_dirs_from(
            Some(" ~/.config/claude , ~ ,, /abs/claude, ~/.config/claude".to_string()),
            Some(OsString::from("/ignored")),
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(
            dirs(resolved),
            [
                PathBuf::from("/home/user/.config/claude"),
                PathBuf::from("/home/user"),
                PathBuf::from("/abs/claude"),
            ]
        );
    }

    #[test]
    fn claude_dirs_keep_env_entries_that_only_look_like_tilde_paths() {
        let resolved = claude_config_dirs_from(
            Some("~user/claude,foo~/claude".to_string()),
            None,
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(
            dirs(resolved),
            [PathBuf::from("~user/claude"), PathBuf::from("foo~/claude")]
        );
    }
}
