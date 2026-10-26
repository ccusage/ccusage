use std::ffi::OsString;

use ccusage_cli::{
    AgentCommandArgs, AgentReportKind, CliConfig, CodexSpeed, Command, DailyArgs, SharedArgs,
    SourceArgs,
};
use ccusage_config::ConfigContext;
use ccusage_core::BUILT_IN_AGENT_NAMES;

use super::Cli;

/// A unified report loads every source, and the config can address one source
/// at a time through `<source>.defaults`, `<source>.commands.<report>` and
/// `commands.<source>:<report>`. Those sections sit under the command-line
/// flags, as they do for `ccusage <source> <report>`, so each source's options
/// come from parsing the same command line against that source's view of the
/// config rather than from patching the unified options afterwards. A source
/// the config does not address keeps the unified options.
pub(crate) fn resolve(
    unified: &mut AgentCommandArgs,
    args: &[OsString],
    config: &ConfigContext,
) -> Result<(), String> {
    let start_of_week = unified.start_of_week;
    if unified.kind == AgentReportKind::Daily
        && let Some(claude_project) = project_from_config(config, unified.shared.clone())
    {
        unified.sources.insert(
            "claude".to_string(),
            SourceArgs {
                shared: unified.shared.clone(),
                claude_project: Some(claude_project),
                ..SourceArgs::default()
            },
        );
    }
    let mut global_source = SourceArgs {
        shared: unified.shared.clone(),
        ..SourceArgs::default()
    };
    config.apply_agent_args(
        &mut global_source.codex_speed,
        Some(&mut global_source.pi_path),
        Some(&mut global_source.open_claw_path),
    );
    if global_source.codex_speed != CodexSpeed::Auto {
        unified.sources.insert(
            "codex".to_string(),
            SourceArgs {
                shared: global_source.shared.clone(),
                codex_speed: global_source.codex_speed,
                ..SourceArgs::default()
            },
        );
    }
    if let Some(pi_path) = global_source.pi_path {
        unified.sources.insert(
            "pi".to_string(),
            SourceArgs {
                shared: global_source.shared.clone(),
                pi_path: Some(pi_path),
                ..SourceArgs::default()
            },
        );
    }
    if let Some(open_claw_path) = global_source.open_claw_path {
        unified.sources.insert(
            "openclaw".to_string(),
            SourceArgs {
                shared: global_source.shared,
                open_claw_path: Some(open_claw_path),
                ..SourceArgs::default()
            },
        );
    }
    for agent in BUILT_IN_AGENT_NAMES {
        let Some(view) = config.for_source(agent) else {
            continue;
        };
        let cli = super::try_parse_with(args.to_vec(), &view, Some(start_of_week))
            .map_err(|message| format!("{message} (in {agent} config)"))?;
        let mut source = SourceArgs {
            shared: unified_shared(cli),
            ..SourceArgs::default()
        };
        view.apply_agent_args(
            &mut source.codex_speed,
            Some(&mut source.pi_path),
            Some(&mut source.open_claw_path),
        );
        if *agent == "claude" && unified.kind == AgentReportKind::Daily {
            source.claude_project = project_from_config(&view, source.shared.clone());
        }
        unified.sources.insert(agent.to_string(), source);
    }
    Ok(())
}

fn project_from_config(config: &dyn CliConfig, shared: SharedArgs) -> Option<String> {
    let mut daily = DailyArgs {
        shared,
        instances: false,
        project: None,
        project_aliases: None,
    };
    config.apply_daily_args(&mut daily);
    daily.project
}

/// The shared options the unified command parsed to, whether it was spelled
/// out or left as the bare `ccusage`.
fn unified_shared(cli: Cli) -> SharedArgs {
    match cli.command {
        Some(Command::All(args)) => args.shared,
        _ => cli.shared,
    }
}

#[cfg(test)]
mod tests {
    use ccusage_cli::{AgentReportKind, CodexSpeed, CostMode, WeekDay};
    use ccusage_core::{PeriodUnit, format_date, last_periods_since, utc_now};
    use ccusage_test_support::fs_fixture;

    use super::*;
    use crate::cli::{try_parse_with, unified_or_command};

    const CONFIG: &str = r#"{
        "defaults": { "mode": "display", "since": "20260101" },
        "codex": {
            "defaults": { "speed": "fast", "since": "20260301" },
            "commands": { "daily": { "mode": "calculate" } }
        },
        "pi": { "defaults": { "piPath": "/data/pi/sessions" } },
        "openclaw": { "commands": { "daily": { "openClawPath": "/data/openclaw" } } }
    }"#;

    const REPORT_CONFIG: &str = r#"{
        "defaults": {
            "since": "20260101",
            "mode": "display",
            "timezone": "UTC",
            "offline": true
        },
        "codex": {
            "defaults": {
                "speed": "fast",
                "timezone": "Europe/London",
                "offline": false
            },
            "commands": {
                "daily": { "since": "20260301" },
                "weekly": { "since": "20260302" },
                "monthly": { "since": "20260303" },
                "session": { "since": "20260304" }
            }
        },
        "commands": {
            "codex:daily": { "mode": "calculate" },
            "codex:weekly": { "mode": "display" },
            "codex:monthly": { "mode": "auto" },
            "codex:session": { "mode": "calculate" }
        }
    }"#;

    const GLOBAL_AGENT_CONFIG: &str = r#"{
        "defaults": {
            "speed": "fast",
            "piPath": "/global/pi/sessions",
            "openClawPath": "/global/openclaw"
        }
    }"#;

    fn try_unified_with_config(
        args: &[&str],
        config_contents: &str,
    ) -> Result<AgentCommandArgs, String> {
        let fixture = fs_fixture!({
            "ccusage.json": config_contents,
        });
        let config_path = fixture.path("ccusage.json").to_string_lossy().into_owned();
        let args = args
            .iter()
            .map(|arg| arg.to_string())
            .chain(["--config".to_string(), config_path])
            .collect::<Vec<_>>();
        let config = ConfigContext::from_args(&args);
        let argv = std::iter::once(OsString::from("ccusage"))
            .chain(args.iter().map(OsString::from))
            .collect::<Vec<_>>();
        let cli = try_parse_with(argv.clone(), &config, None)?;
        let Command::All(mut unified) = unified_or_command(cli) else {
            return Err("expected a unified report".to_string());
        };
        resolve(&mut unified, &argv, &config)?;
        Ok(unified)
    }

    fn unified_with_config(args: &[&str], config_contents: &str) -> AgentCommandArgs {
        try_unified_with_config(args, config_contents).unwrap()
    }

    fn unified(args: &[&str]) -> AgentCommandArgs {
        unified_with_config(args, CONFIG)
    }

    #[test]
    fn reads_each_source_through_its_own_config_sections() {
        let unified = unified(&["daily"]);

        assert_eq!(unified.shared.since.as_deref(), Some("20260101"));
        assert_eq!(unified.shared.mode, CostMode::Display);
        let codex = &unified.sources["codex"];
        assert_eq!(codex.shared.since.as_deref(), Some("20260301"));
        assert_eq!(codex.shared.mode, CostMode::Calculate);
        assert_eq!(codex.codex_speed, CodexSpeed::Fast);
        assert_eq!(
            unified.sources["pi"].pi_path.as_deref(),
            Some("/data/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"].open_claw_path.as_deref(),
            Some("/data/openclaw")
        );
        assert!(!unified.sources.contains_key("amp"));
    }

    #[test]
    fn resolves_muse_source_options_for_unified_reports() {
        let unified = unified_with_config(
            &["daily"],
            r#"{
                "muse": {
                    "defaults": {
                        "offline": true
                    },
                    "commands": {
                        "daily": {
                            "since": "20260401",
                            "timezone": "Europe/London"
                        }
                    }
                }
            }"#,
        );

        assert_eq!(
            unified
                .sources
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["muse"]
        );
        let muse = &unified.sources["muse"];
        assert!(muse.shared.offline);
        assert_eq!(muse.shared.since.as_deref(), Some("20260401"));
        assert_eq!(muse.shared.timezone.as_deref(), Some("Europe/London"));
    }

    #[test]
    fn applies_global_agent_defaults_to_relevant_unified_sources_for_each_report() {
        let cases = [
            ("bare default", &[][..], AgentReportKind::Daily),
            ("daily", &["daily"][..], AgentReportKind::Daily),
            ("weekly", &["weekly"][..], AgentReportKind::Weekly),
            ("monthly", &["monthly"][..], AgentReportKind::Monthly),
            ("session", &["session"][..], AgentReportKind::Session),
        ];

        for (name, args, kind) in cases {
            let unified = unified_with_config(args, GLOBAL_AGENT_CONFIG);

            assert_eq!(unified.kind, kind, "{name}");
            assert_eq!(
                unified
                    .sources
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["codex", "openclaw", "pi"],
                "{name}"
            );

            let codex = &unified.sources["codex"];
            assert_eq!(codex.codex_speed, CodexSpeed::Fast, "{name}");
            assert!(codex.pi_path.is_none(), "{name}");
            assert!(codex.open_claw_path.is_none(), "{name}");

            let pi = &unified.sources["pi"];
            assert_eq!(pi.pi_path.as_deref(), Some("/global/pi/sessions"), "{name}");
            assert_eq!(pi.codex_speed, CodexSpeed::Auto, "{name}");
            assert!(pi.open_claw_path.is_none(), "{name}");

            let openclaw = &unified.sources["openclaw"];
            assert_eq!(
                openclaw.open_claw_path.as_deref(),
                Some("/global/openclaw"),
                "{name}"
            );
            assert_eq!(openclaw.codex_speed, CodexSpeed::Auto, "{name}");
            assert!(openclaw.pi_path.is_none(), "{name}");
        }
    }

    #[test]
    fn source_agent_sections_override_global_defaults_and_cli_overrides_shared_defaults() {
        let config = r#"{
            "defaults": {
                "since": "20260101",
                "offline": false,
                "speed": "fast",
                "piPath": "/global/pi/sessions",
                "openClawPath": "/global/openclaw"
            },
            "codex": {
                "defaults": { "speed": "standard" },
                "commands": { "daily": { "speed": "flex" } }
            },
            "pi": { "defaults": { "piPath": "/source/pi/sessions" } },
            "openclaw": {
                "commands": { "daily": { "openClawPath": "/source/openclaw" } }
            }
        }"#;
        let unified = unified_with_config(&["daily", "--since", "20260401", "--offline"], config);

        assert_eq!(unified.shared.since.as_deref(), Some("20260401"));
        assert!(unified.shared.offline);
        assert_eq!(
            unified
                .sources
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["codex", "openclaw", "pi"]
        );
        for source in unified.sources.values() {
            assert_eq!(source.shared.since.as_deref(), Some("20260401"));
            assert!(source.shared.offline);
        }
        assert_eq!(unified.sources["codex"].codex_speed, CodexSpeed::Flex);
        assert_eq!(
            unified.sources["pi"].pi_path.as_deref(),
            Some("/source/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"].open_claw_path.as_deref(),
            Some("/source/openclaw")
        );
    }

    #[test]
    fn applies_global_report_level_agent_options_without_source_sections() {
        let config = r#"{
            "defaults": {
                "speed": "standard",
                "piPath": "/global/pi/sessions",
                "openClawPath": "/global/openclaw"
            },
            "commands": { "daily": { "speed": "flex" } }
        }"#;
        let unified = unified_with_config(&["daily"], config);

        assert_eq!(unified.sources["codex"].codex_speed, CodexSpeed::Flex);
        assert_eq!(
            unified.sources["pi"].pi_path.as_deref(),
            Some("/global/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"].open_claw_path.as_deref(),
            Some("/global/openclaw")
        );
        assert_eq!(
            unified
                .sources
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["codex", "openclaw", "pi"]
        );
    }

    #[test]
    fn keeps_command_line_flags_above_source_sections() {
        let unified = unified_with_config(
            &[
                "daily",
                "--since",
                "20260601",
                "--mode",
                "auto",
                "--timezone",
                "Pacific/Auckland",
                "--offline",
            ],
            REPORT_CONFIG,
        );

        let codex = &unified.sources["codex"];
        assert_eq!(codex.shared.since.as_deref(), Some("20260601"));
        assert_eq!(codex.shared.mode, CostMode::Auto);
        assert_eq!(codex.shared.timezone.as_deref(), Some("Pacific/Auckland"));
        assert!(codex.shared.offline);
        assert_eq!(codex.codex_speed, CodexSpeed::Fast);
    }

    #[test]
    fn applies_source_and_command_sections_for_each_unified_report() {
        let cases = [
            (
                "bare default",
                &[][..],
                AgentReportKind::Daily,
                "20260301",
                CostMode::Calculate,
            ),
            (
                "daily",
                &["daily"][..],
                AgentReportKind::Daily,
                "20260301",
                CostMode::Calculate,
            ),
            (
                "weekly",
                &["weekly"][..],
                AgentReportKind::Weekly,
                "20260302",
                CostMode::Display,
            ),
            (
                "monthly",
                &["monthly"][..],
                AgentReportKind::Monthly,
                "20260303",
                CostMode::Auto,
            ),
            (
                "session",
                &["session"][..],
                AgentReportKind::Session,
                "20260304",
                CostMode::Calculate,
            ),
        ];

        for (name, args, kind, since, mode) in cases {
            let unified = unified_with_config(args, REPORT_CONFIG);

            assert_eq!(unified.kind, kind, "{name}");
            let codex = &unified.sources["codex"];
            assert_eq!(codex.shared.since.as_deref(), Some(since), "{name}");
            assert_eq!(codex.shared.mode, mode, "{name}");
            assert_eq!(
                codex.shared.timezone.as_deref(),
                Some("Europe/London"),
                "{name}"
            );
            assert!(!codex.shared.offline, "{name}");
            assert_eq!(codex.codex_speed, CodexSpeed::Fast, "{name}");
        }
    }

    #[test]
    fn applies_colon_command_config_without_a_source_object() {
        let config = r#"{
            "commands": {
                "codex:daily": { "since": "20260602", "mode": "auto" }
            }
        }"#;
        let unified = unified_with_config(&["daily"], config);

        let codex = &unified.sources["codex"];
        assert_eq!(codex.shared.since.as_deref(), Some("20260602"));
        assert_eq!(codex.shared.mode, CostMode::Auto);
    }

    #[test]
    fn applies_spaced_command_config_without_a_source_object() {
        let config = r#"{
            "defaults": { "since": "20260101", "mode": "display" },
            "commands": {
                "codex daily": { "since": "20260602", "mode": "calculate" }
            }
        }"#;
        let unified = unified_with_config(&["daily"], config);

        assert_eq!(unified.shared.since.as_deref(), Some("20260101"));
        assert_eq!(unified.shared.mode, CostMode::Display);
        let codex = &unified.sources["codex"];
        assert_eq!(codex.shared.since.as_deref(), Some("20260602"));
        assert_eq!(codex.shared.mode, CostMode::Calculate);
    }

    #[test]
    fn resolves_sources_for_the_bare_command() {
        let unified = unified(&[]);

        assert_eq!(unified.kind, AgentReportKind::Daily);
        assert_eq!(
            unified.sources["codex"].shared.since.as_deref(),
            Some("20260301")
        );
        assert_eq!(
            unified.sources["pi"].pi_path.as_deref(),
            Some("/data/pi/sessions")
        );
    }

    #[test]
    fn reads_the_sections_of_the_invoked_report() {
        let unified = unified(&["session"]);

        assert_eq!(unified.kind, AgentReportKind::Session);
        assert_eq!(unified.sources["codex"].shared.mode, CostMode::Display);
        assert!(unified.sources["openclaw"].open_claw_path.is_none());
    }

    #[test]
    fn source_last_uses_the_unified_week_start_and_keeps_its_shared_options() {
        let config = r#"{
            "defaults": { "timezone": "UTC" },
            "commands": { "weekly": { "startOfWeek": "wednesday" } },
            "codex": {
                "defaults": { "timezone": "America/Los_Angeles" },
                "commands": { "weekly": { "startOfWeek": "sunday" } }
            }
        }"#;
        let unified = unified_with_config(&["weekly", "--last", "1"], config);
        let codex = &unified.sources["codex"];
        let today = format_date(utc_now(), codex.shared.timezone.as_deref());

        assert_eq!(unified.start_of_week, WeekDay::Wednesday);
        assert_eq!(
            codex.shared.timezone.as_deref(),
            Some("America/Los_Angeles")
        );
        assert_eq!(codex.shared.last, Some(1));
        assert_eq!(
            codex.shared.since,
            last_periods_since(PeriodUnit::Week, 1, &today, unified.start_of_week)
        );
    }

    #[test]
    fn cli_week_start_overrides_global_and_source_config_for_last() {
        let config = r#"{
            "defaults": { "timezone": "UTC" },
            "commands": { "weekly": { "startOfWeek": "monday" } },
            "codex": { "commands": { "weekly": { "startOfWeek": "sunday" } } }
        }"#;
        let unified = unified_with_config(
            &["weekly", "--last", "1", "--start-of-week", "friday"],
            config,
        );
        let codex = &unified.sources["codex"];
        let today = format_date(utc_now(), codex.shared.timezone.as_deref());

        assert_eq!(unified.start_of_week, WeekDay::Friday);
        assert_eq!(
            codex.shared.since,
            last_periods_since(PeriodUnit::Week, 1, &today, WeekDay::Friday)
        );
    }

    #[test]
    fn labels_source_last_conflicts_with_the_source_config() {
        let config = r#"{
            "codex": { "defaults": { "since": "20260301" } }
        }"#;

        let error = try_unified_with_config(&["daily", "--last", "1"], config)
            .err()
            .expect("expected the source config to conflict with --last");

        assert_eq!(
            error,
            "The --last option cannot be combined with --since or --until. (in codex config)"
        );
    }

    #[test]
    fn labels_invalid_source_timezones_with_the_source_config() {
        let config = r#"{
            "codex": { "defaults": { "timezone": "Not/AZone" } }
        }"#;

        let error = try_unified_with_config(&["daily"], config)
            .err()
            .expect("expected the source timezone to be rejected");

        assert_eq!(
            error,
            "Invalid value for --timezone 'Not/AZone'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone. (in codex config)"
        );
    }

    #[test]
    fn preserves_the_claude_daily_project_from_its_source_config() {
        let config = r#"{
            "claude": { "commands": { "daily": { "project": "repo" } } }
        }"#;

        let unified = unified_with_config(&["daily"], config);

        assert_eq!(
            unified.sources["claude"].claude_project.as_deref(),
            Some("repo")
        );
    }

    #[test]
    fn keeps_the_global_claude_project_without_a_source_section_and_allows_overrides() {
        let cases = [
            (
                r#"{ "commands": { "daily": { "project": "global" } } }"#,
                "global",
            ),
            (
                r#"{
                    "commands": { "daily": { "project": "global" } },
                    "claude": { "defaults": { "timezone": "UTC" } }
                }"#,
                "global",
            ),
            (
                r#"{
                    "commands": { "daily": { "project": "global" } },
                    "claude": { "commands": { "daily": { "project": "source" } } }
                }"#,
                "source",
            ),
        ];

        for (config, project) in cases {
            let unified = unified_with_config(&["daily"], config);

            assert_eq!(
                unified
                    .sources
                    .get("claude")
                    .and_then(|source| source.claude_project.as_deref()),
                Some(project),
                "{config}"
            );
        }
    }
}
