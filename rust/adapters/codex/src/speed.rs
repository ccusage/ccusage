use std::{fs, path::Path};

use crate::{CodexServiceTier, cli::CodexSpeed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexSpeedPolicy {
    Auto,
    Forced(CodexServiceTier),
}

pub(crate) fn resolve_codex_speed(requested: CodexSpeed) -> CodexSpeedPolicy {
    match requested {
        CodexSpeed::Auto => CodexSpeedPolicy::Auto,
        CodexSpeed::Standard => CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CodexSpeed::Fast => CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
        CodexSpeed::Flex => CodexSpeedPolicy::Forced(CodexServiceTier::Flex),
    }
}

pub(super) fn configured_tier(home: &Path) -> Option<CodexServiceTier> {
    fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|content| codex_config_service_tier(&content))
}

fn codex_config_service_tier(content: &str) -> Option<CodexServiceTier> {
    let config = content.parse::<toml::Table>().ok()?;
    let profile_tier = config
        .get("profile")
        .and_then(toml::Value::as_str)
        .and_then(|name| config.get("profiles")?.get(name)?.get("service_tier"));
    CodexServiceTier::from_codex_name(
        profile_tier
            .or_else(|| config.get("service_tier"))?
            .as_str()?,
    )
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;

    use crate::CodexServiceTier;

    use super::codex_config_service_tier;

    #[test]
    fn detects_explicit_service_tier_values() {
        assert_eq!(
            codex_config_service_tier(r#"service_tier = "fast""#,),
            Some(CodexServiceTier::Fast)
        );
        assert_eq!(
            codex_config_service_tier(r#"service_tier = 'priority' # use higher tier"#,),
            Some(CodexServiceTier::Fast)
        );
        assert_eq!(
            codex_config_service_tier(r#"service_tier = "flex""#,),
            Some(CodexServiceTier::Flex)
        );
    }

    #[test]
    fn ignores_unrelated_or_substring_service_tier_values() {
        assert_eq!(
            codex_config_service_tier(r#"service_tier_override = "fast""#,),
            None
        );
        assert_eq!(
            codex_config_service_tier(r#"service_tier = "breakfast""#,),
            None
        );
        assert_eq!(
            codex_config_service_tier(r#"service_tier = "standard""#,),
            Some(CodexServiceTier::Standard)
        );
    }

    #[test]
    fn ignores_inactive_profiles_when_resolving_config_tier() {
        assert_eq!(
            codex_config_service_tier(
                r#"
                    service_tier = "standard"
                    [profiles.flex]
                    service_tier = "flex"
                "#,
            ),
            Some(CodexServiceTier::Standard)
        );
        assert_eq!(
            codex_config_service_tier(
                r#"
                    [profiles.flex]
                    service_tier = "flex"
                "#,
            ),
            None
        );
    }

    #[test]
    fn selected_profile_overrides_top_level_tier() {
        assert_eq!(
            codex_config_service_tier(
                r#"
                    service_tier = "standard"
                    profile = "flex.#"
                    [profiles."flex.#"]
                    service_tier = "flex"
                    [profiles.fast]
                    service_tier = "priority"
                "#,
            ),
            Some(CodexServiceTier::Flex)
        );
        assert_eq!(
            codex_config_service_tier(
                r#"
                    service_tier = "priority"
                    profile = "standard"
                    [profiles.standard]
                    service_tier = "default"
                "#,
            ),
            Some(CodexServiceTier::Standard)
        );
    }

    #[test]
    fn selected_profile_without_a_tier_inherits_top_level_tier() {
        assert_eq!(
            codex_config_service_tier(
                r#"
                    service_tier = "flex"
                    profile = "work"
                    [profiles.work]
                    model = "gpt-6.1-sol"
                    [profiles.fast]
                    service_tier = "priority"
                "#,
            ),
            Some(CodexServiceTier::Flex)
        );
    }

    #[test]
    fn ignores_tier_assignments_inside_multiline_strings() {
        assert_eq!(
            codex_config_service_tier(
                r#"
                    service_tier = "standard"
                    instructions = '''
                    service_tier = "flex"
                    '''
                "#,
            ),
            Some(CodexServiceTier::Standard)
        );
    }

    #[test]
    fn invalid_config_does_not_select_a_tier() {
        assert_eq!(codex_config_service_tier("service_tier = \"flex"), None);
    }

    #[test]
    fn inactive_flex_profile_does_not_discount_unclassified_usage() {
        let fixture = fs_fixture!({
            "config.toml": r#"
                service_tier = "standard"
                [profiles.flex]
                service_tier = "flex"
            "#,
        });
        assert_eq!(
            super::configured_tier(fixture.root()),
            Some(CodexServiceTier::Standard)
        );
    }
}
