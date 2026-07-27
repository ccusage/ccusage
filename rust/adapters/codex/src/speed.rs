use std::fs;

use crate::{CodexServiceTier, cli::CodexSpeed};

use super::paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexSpeedPolicy {
    Auto(CodexServiceTier),
    Forced(CodexServiceTier),
}

impl From<CodexSpeed> for CodexSpeedPolicy {
    fn from(speed: CodexSpeed) -> Self {
        match speed {
            CodexSpeed::Auto => Self::Auto(CodexServiceTier::Standard),
            CodexSpeed::Standard => Self::Forced(CodexServiceTier::Standard),
            CodexSpeed::Fast => Self::Forced(CodexServiceTier::Fast),
        }
    }
}

pub fn resolve_codex_speed(requested: CodexSpeed) -> CodexSpeedPolicy {
    match requested {
        CodexSpeed::Auto => {
            if detect_codex_fast_service_tier() {
                CodexSpeedPolicy::Auto(CodexServiceTier::Fast)
            } else {
                CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
            }
        }
        CodexSpeed::Standard => CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CodexSpeed::Fast => CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
    }
}

fn detect_codex_fast_service_tier() -> bool {
    codex_home_paths().iter().any(|path| {
        fs::read_to_string(path.join("config.toml"))
            .ok()
            .is_some_and(|content| codex_config_requests_fast_service_tier(&content))
    })
}

fn codex_home_paths() -> Vec<std::path::PathBuf> {
    paths::codex_home_paths().unwrap_or_default()
}

fn codex_config_requests_fast_service_tier(content: &str) -> bool {
    content.lines().any(|line| {
        let setting = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = setting.split_once('=') else {
            return false;
        };
        if key.trim() != "service_tier" {
            return false;
        }
        let value = value.trim().trim_matches(['"', '\'']);
        matches!(value, "fast" | "priority")
    })
}

#[cfg(test)]
mod tests {
    use super::codex_config_requests_fast_service_tier;

    #[test]
    fn detects_explicit_fast_service_tier_values() {
        assert!(codex_config_requests_fast_service_tier(
            r#"service_tier = "fast""#,
        ));
        assert!(codex_config_requests_fast_service_tier(
            r#"service_tier = 'priority' # use higher tier"#,
        ));
    }

    #[test]
    fn ignores_unrelated_or_substring_service_tier_values() {
        assert!(!codex_config_requests_fast_service_tier(
            r#"service_tier_override = "fast""#,
        ));
        assert!(!codex_config_requests_fast_service_tier(
            r#"service_tier = "breakfast""#,
        ));
        assert!(!codex_config_requests_fast_service_tier(
            r#"service_tier = "standard""#,
        ));
    }
}
