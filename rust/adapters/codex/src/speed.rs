use std::fs;

use crate::{CodexServiceTier, Pricing, cli::CodexSpeed};

use super::paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexSpeedPolicy {
    Auto(ConfiguredTiers),
    Forced(CodexServiceTier),
}

pub(crate) fn resolve_codex_speed(requested: CodexSpeed) -> CodexSpeedPolicy {
    match requested {
        CodexSpeed::Auto => CodexSpeedPolicy::Auto(detect_configured_tiers()),
        CodexSpeed::Standard => CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        CodexSpeed::Fast => CodexSpeedPolicy::Forced(CodexServiceTier::Fast),
        CodexSpeed::Flex => CodexSpeedPolicy::Forced(CodexServiceTier::Flex),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfiguredTiers(u8);

impl From<CodexServiceTier> for ConfiguredTiers {
    fn from(tier: CodexServiceTier) -> Self {
        Self(1 << tier as u8)
    }
}

impl ConfiguredTiers {
    fn contains(self, tier: CodexServiceTier) -> bool {
        self.0 & Self::from(tier).0 != 0
    }

    pub(crate) fn for_pricing(self, pricing: &Pricing) -> CodexServiceTier {
        use CodexServiceTier::{Fast, Flex, Standard, Ultrafast};

        if self.contains(Ultrafast) && pricing.has_ultrafast_rate() {
            Ultrafast
        } else if self.contains(Fast) && pricing.has_fast_rate() {
            Fast
        } else if self.contains(Flex) && pricing.has_flex_rate() {
            Flex
        } else {
            Standard
        }
    }
}

fn detect_configured_tiers() -> ConfiguredTiers {
    codex_home_paths()
        .into_iter()
        .filter_map(|path| {
            fs::read_to_string(path.join("config.toml"))
                .ok()
                .and_then(|content| codex_config_service_tier(&content))
        })
        .fold(ConfiguredTiers::default(), |tiers, tier| {
            ConfiguredTiers(tiers.0 | ConfiguredTiers::from(tier).0)
        })
}

fn codex_home_paths() -> Vec<std::path::PathBuf> {
    paths::codex_home_paths().unwrap_or_default()
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
    use ccusage_test_support::{Environment, fs_fixture};

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
    fn retains_each_configured_tier_including_a_selected_ultrafast_profile() {
        let fixture = fs_fixture!({
            "fast/config.toml": r#"service_tier = "fast""#,
            "ultrafast/config.toml": r#"service_tier = "default"
                profile = "work"
                [profiles.work]
                service_tier = "ultrafast""#,
            "flex/config.toml": r#"service_tier = "flex""#,
        });
        let homes = ["fast", "ultrafast", "flex"]
            .map(|root| fixture.path(root).display().to_string())
            .join(",");
        Environment::with_var("CODEX_HOME", homes).run(|| {
            let super::CodexSpeedPolicy::Auto(tiers) =
                super::resolve_codex_speed(crate::cli::CodexSpeed::Auto)
            else {
                panic!("expected automatic pricing");
            };
            assert!(tiers.contains(CodexServiceTier::Ultrafast));
            assert!(tiers.contains(CodexServiceTier::Fast));
            assert!(tiers.contains(CodexServiceTier::Flex));
        })
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
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "config.toml": r#"
                service_tier = "standard"
                [profiles.flex]
                service_tier = "flex"
            "#,
        });
        Environment::with_var("CODEX_HOME", fixture.path("")).run(|| {
            let mut pricing = crate::PricingMap::default();
            pricing.load_json(
                r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2, "flex": 0.5 }
                }
            }"#,
            );
            let usage = crate::CodexModelUsage {
                total_tokens: 30,
                ..crate::CodexModelUsage::from_buckets(
                    crate::CodexUsageBucket {
                        input_tokens: 30,
                        ..Default::default()
                    },
                    {
                        let mut tiers = crate::TierBuckets::default();
                        tiers[crate::CodexServiceTier::Flex] = crate::CodexUsageBucket {
                            input_tokens: 10,
                            ..Default::default()
                        };
                        tiers[crate::CodexServiceTier::Fast] = crate::CodexUsageBucket {
                            input_tokens: 10,
                            ..Default::default()
                        };
                        tiers
                    },
                )
            };

            let cost = crate::report::calculate_codex_model_cost(
                "gpt-test",
                &usage,
                Some(&pricing),
                super::resolve_codex_speed(crate::cli::CodexSpeed::Auto),
            );

            assert!((cost - 35e-6).abs() < 1e-12);
        })
    }
}
