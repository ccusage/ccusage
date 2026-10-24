use std::ffi::OsString;

use ccusage_cli::{AgentCommandArgs, CliConfig, Command, SharedArgs, SourceArgs};
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
pub(crate) fn resolve(unified: &mut AgentCommandArgs, args: &[OsString], config: &ConfigContext) {
    for agent in BUILT_IN_AGENT_NAMES {
        let Some(view) = config.for_source(agent) else {
            continue;
        };
        let mut source = SourceArgs {
            shared: unified_shared(super::parse_with(args.to_vec(), &view)),
            ..SourceArgs::default()
        };
        view.apply_agent_args(
            &mut source.codex_speed,
            Some(&mut source.pi_path),
            Some(&mut source.open_claw_path),
        );
        unified.sources.insert(agent.to_string(), source);
    }
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
    use ccusage_cli::{AgentReportKind, CodexSpeed, CostMode};
    use ccusage_test_support::fs_fixture;

    use super::*;
    use crate::cli::{parse_with, unified_or_command};

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

    fn unified_with_config(args: &[&str], config_contents: &str) -> AgentCommandArgs {
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
        let Command::All(mut unified) = unified_or_command(parse_with(argv.clone(), &config))
        else {
            panic!("expected a unified report");
        };
        resolve(&mut unified, &argv, &config);
        unified
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
}
