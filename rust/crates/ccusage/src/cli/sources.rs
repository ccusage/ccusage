use std::ffi::OsString;

use ccusage_cli::{AgentCommandArgs, CliConfig, SourceArgs};
use ccusage_config::ConfigContext;
use ccusage_core::cli::BUILT_IN_AGENT_NAMES;

pub(crate) fn resolve(
    unified: &mut AgentCommandArgs,
    args: &[OsString],
    config: &ConfigContext,
) -> Result<(), String> {
    for agent in BUILT_IN_AGENT_NAMES {
        let view = config.for_dependency(agent);
        let report = super::try_parse_report_with(
            args.to_vec(),
            &view,
            unified.start_of_week,
            unified.shared.timezone.as_ref(),
            unified.shared.clock,
        )
        .map_err(|message| format!("{message} (in {agent} config)"))?;
        let mut options = view.agent_options(agent);
        if *agent == "claude" {
            options.claude_project = report.options.claude_project.or(options.claude_project);
        }
        unified.sources.insert(
            agent.to_string(),
            SourceArgs::from_options(report.shared, options),
        );
    }
    Ok(())
}

pub(crate) fn resolve_opencode_claims(
    focused: &mut AgentCommandArgs,
    args: &[OsString],
    config: &ConfigContext,
) -> Result<(), String> {
    let view = config.for_dependency("claude");
    let parsed = super::try_parse_report_with(
        args.to_vec(),
        &view,
        focused.start_of_week,
        focused.shared.timezone.as_ref(),
        focused.shared.clock,
    )?;
    let options = view.agent_options("claude");
    focused.sources.insert(
        "claude".to_string(),
        SourceArgs::from_options(parsed.shared, options),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::last_window::{last_periods_since, last_periods_until};
    use ccusage_cli::{AgentReportKind, CodexSpeed, Command, CostMode, PeriodUnit, WeekDay};
    use ccusage_core::{date_at, resolve_tz};
    use ccusage_test_support::fs_fixture;

    use super::*;
    fn try_parse_with(args: Vec<OsString>, config: &ConfigContext) -> Result<Command, String> {
        let mut command = ccusage_cli_parser::parse_from_with_config(args, config)?;
        super::super::resolve_report_clock(&mut command, ccusage_cli::ReportClock::System)?;
        Ok(command)
    }

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
        let cli = try_parse_with(argv.clone(), &config)?;
        let Command::All(mut unified) = cli else {
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

        assert_eq!(
            unified.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260101")
        );
        assert_eq!(unified.shared.mode, CostMode::Display);
        let codex = &unified.sources["codex"];
        assert_eq!(
            codex.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260301")
        );
        assert_eq!(codex.shared.mode, CostMode::Calculate);
        assert_eq!(codex.options.codex_speed, CodexSpeed::Fast);
        assert_eq!(
            unified.sources["pi"].options.pi_path.as_deref(),
            Some("/data/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"]
                .options
                .open_claw_path
                .as_deref(),
            Some("/data/openclaw")
        );
        assert_eq!(unified.sources["amp"].shared.since, unified.shared.since);
    }

    #[test]
    fn focused_opencode_resolves_claude_claim_options_and_cli_precedence() {
        let fixture = fs_fixture!({
            "config.json": r#"{"opencode":{"defaults":{"timezone":"UTC","since":"20990101"}},"claude":{"defaults":{"timezone":"Asia/Tokyo","since":"20990103","project":"selected"}}}"#,
        });
        let config_path = fixture.path("config.json");
        let strings = vec![
            "opencode".to_string(),
            "daily".to_string(),
            "--config".to_string(),
            config_path.to_string_lossy().into_owned(),
            "--since".to_string(),
            "20990102".to_string(),
        ];
        let config = ConfigContext::from_args(&strings);
        let args = std::iter::once(OsString::from("ccusage"))
            .chain(strings.iter().map(OsString::from))
            .collect::<Vec<_>>();
        let Command::Agent(_, ccusage_cli::AgentCommand::Report(mut focused)) =
            try_parse_with(args.clone(), &config).unwrap()
        else {
            panic!("expected focused command")
        };
        resolve_opencode_claims(&mut focused, &args, &config).unwrap();
        assert_eq!(
            focused
                .shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("UTC")
        );
        let claude = &focused.sources["claude"];
        assert_eq!(
            claude
                .shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("Asia/Tokyo")
        );
        assert_eq!(
            claude.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20990102")
        );
        assert_eq!(claude.options.claude_project.as_deref(), Some("selected"));
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

            let codex = &unified.sources["codex"];
            assert_eq!(codex.options.codex_speed, CodexSpeed::Fast, "{name}");
            assert!(codex.options.pi_path.is_none(), "{name}");
            assert!(codex.options.open_claw_path.is_none(), "{name}");

            let pi = &unified.sources["pi"];
            assert_eq!(
                pi.options.pi_path.as_deref(),
                Some("/global/pi/sessions"),
                "{name}"
            );
            assert_eq!(pi.options.codex_speed, CodexSpeed::Auto, "{name}");
            assert!(pi.options.open_claw_path.is_none(), "{name}");

            let openclaw = &unified.sources["openclaw"];
            assert_eq!(
                openclaw.options.open_claw_path.as_deref(),
                Some("/global/openclaw"),
                "{name}"
            );
            assert_eq!(openclaw.options.codex_speed, CodexSpeed::Auto, "{name}");
            assert!(openclaw.options.pi_path.is_none(), "{name}");
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

        assert_eq!(
            unified.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260401")
        );
        assert!(unified.shared.offline);

        for source in unified.sources.values() {
            assert_eq!(
                source.shared.since.map(|date| date.to_string()).as_deref(),
                Some("20260401")
            );
            assert!(source.shared.offline);
        }
        assert_eq!(
            unified.sources["codex"].options.codex_speed,
            CodexSpeed::Flex
        );
        assert_eq!(
            unified.sources["pi"].options.pi_path.as_deref(),
            Some("/source/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"]
                .options
                .open_claw_path
                .as_deref(),
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

        assert_eq!(
            unified.sources["codex"].options.codex_speed,
            CodexSpeed::Flex
        );
        assert_eq!(
            unified.sources["pi"].options.pi_path.as_deref(),
            Some("/global/pi/sessions")
        );
        assert_eq!(
            unified.sources["openclaw"]
                .options
                .open_claw_path
                .as_deref(),
            Some("/global/openclaw")
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
        assert_eq!(
            codex.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260601")
        );
        assert_eq!(codex.shared.mode, CostMode::Auto);
        assert_eq!(
            codex
                .shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("Pacific/Auckland")
        );
        assert!(codex.shared.offline);
        assert_eq!(codex.options.codex_speed, CodexSpeed::Fast);
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
            assert_eq!(
                codex.shared.since.map(|date| date.to_string()).as_deref(),
                Some(since),
                "{name}"
            );
            assert_eq!(codex.shared.mode, mode, "{name}");
            assert_eq!(
                codex
                    .shared
                    .timezone
                    .as_ref()
                    .map(|timezone| timezone.name()),
                Some("Europe/London"),
                "{name}"
            );
            assert!(!codex.shared.offline, "{name}");
            assert_eq!(codex.options.codex_speed, CodexSpeed::Fast, "{name}");
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
        assert_eq!(
            codex.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260602")
        );
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

        assert_eq!(
            unified.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260101")
        );
        assert_eq!(unified.shared.mode, CostMode::Display);
        let codex = &unified.sources["codex"];
        assert_eq!(
            codex.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260602")
        );
        assert_eq!(codex.shared.mode, CostMode::Calculate);
    }

    #[test]
    fn applies_spaced_command_config_over_the_generic_report_section() {
        let config = r#"{
            "commands": {
                "daily": { "since": "20260101", "mode": "display" },
                "codex daily": { "since": "20260602", "mode": "calculate" }
            }
        }"#;
        let unified = unified_with_config(&["daily"], config);

        assert_eq!(
            unified.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260101")
        );
        assert_eq!(unified.shared.mode, CostMode::Display);
        let codex = &unified.sources["codex"];
        assert_eq!(
            codex.shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260602")
        );
        assert_eq!(codex.shared.mode, CostMode::Calculate);
    }

    #[test]
    fn resolves_sources_for_the_bare_command() {
        let unified = unified(&[]);

        assert_eq!(unified.kind, AgentReportKind::Daily);
        assert_eq!(
            unified.sources["codex"]
                .shared
                .since
                .map(|date| date.to_string())
                .as_deref(),
            Some("20260301")
        );
        assert_eq!(
            unified.sources["pi"].options.pi_path.as_deref(),
            Some("/data/pi/sessions")
        );
    }

    #[test]
    fn reads_the_sections_of_the_invoked_report() {
        let unified = unified(&["session"]);

        assert_eq!(unified.kind, AgentReportKind::Session);
        assert_eq!(unified.sources["codex"].shared.mode, CostMode::Display);
        assert!(unified.sources["openclaw"].options.open_claw_path.is_none());
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
        let today = date_at(
            codex.shared.clock.now().into(),
            &resolve_tz(codex.shared.timezone.as_ref()),
        );

        assert_eq!(unified.start_of_week, WeekDay::Wednesday);
        assert_eq!(
            codex
                .shared
                .timezone
                .as_ref()
                .map(|timezone| timezone.name()),
            Some("America/Los_Angeles")
        );
        assert_eq!(codex.shared.last, Some(std::num::NonZeroU32::MIN));
        assert_eq!(
            codex.shared.since,
            last_periods_since(
                PeriodUnit::Week,
                std::num::NonZeroU32::MIN,
                today,
                unified.start_of_week
            )
        );
        assert_eq!(
            codex.shared.until,
            last_periods_until(PeriodUnit::Week, today, unified.start_of_week)
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
        let today = date_at(
            codex.shared.clock.now().into(),
            &resolve_tz(codex.shared.timezone.as_ref()),
        );

        assert_eq!(unified.start_of_week, WeekDay::Friday);
        assert_eq!(
            codex.shared.since,
            last_periods_since(
                PeriodUnit::Week,
                std::num::NonZeroU32::MIN,
                today,
                WeekDay::Friday
            )
        );
        assert_eq!(
            codex.shared.until,
            last_periods_until(PeriodUnit::Week, today, WeekDay::Friday)
        );
    }

    #[test]
    fn source_last_replaces_the_date_window_of_the_source_config() {
        let config = r#"{
            "defaults": { "timezone": "UTC" },
            "codex": { "defaults": { "since": "20260301", "until": "20260331" } }
        }"#;
        let unified = unified_with_config(&["daily", "--last", "1"], config);
        let codex = &unified.sources["codex"];
        let today = date_at(
            codex.shared.clock.now().into(),
            &resolve_tz(codex.shared.timezone.as_ref()),
        );

        assert_eq!(
            codex.shared.since,
            last_periods_since(
                PeriodUnit::Day,
                std::num::NonZeroU32::MIN,
                today,
                unified.start_of_week
            )
        );
        assert_eq!(
            codex.shared.until,
            last_periods_until(PeriodUnit::Day, today, unified.start_of_week)
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

        assert!(error.starts_with("Invalid value for ccusage config field "));
        assert!(error.contains("ccusage.json: codex.defaults.timezone 'Not/AZone'."));
        assert!(error.ends_with("(in codex config)"));
    }

    #[test]
    fn preserves_the_claude_daily_project_from_its_source_config() {
        let config = r#"{
            "claude": { "commands": { "daily": { "project": "repo" } } }
        }"#;

        let unified = unified_with_config(&["daily"], config);

        assert_eq!(
            unified.sources["claude"].options.claude_project.as_deref(),
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
                    .and_then(|source| source.options.claude_project.as_deref()),
                Some(project),
                "{config}"
            );
        }
    }
}
