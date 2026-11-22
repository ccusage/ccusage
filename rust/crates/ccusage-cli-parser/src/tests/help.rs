use super::*;

#[test]
fn root_help_lists_agent_namespaces_without_nested_commands() {
    let help = help_text();
    let agents = [
        "claude", "codex", "opencode", "amp", "droid", "codebuff", "hermes", "pi", "goose", "kilo",
        "copilot", "gemini", "kimi", "qwen", "openclaw", "grok", "zcode", "cline", "dsh",
    ];

    for agent in agents {
        assert!(help.contains(&format!("\n  {agent} ")));
        assert!(!help.contains(&format!("\n  {agent} daily")));
    }
}

#[test]
fn root_help_lists_command_descriptions_and_follow_up_help_commands() {
    let help = help_text();

    assert!(help.contains("codex                      Show Codex token usage commands"));
    assert!(help.contains("For more info, run any command with the `--help` flag:"));
    assert!(help.contains("ccusage codex --help"));
    assert!(!help.contains("ccusage codex daily --help"));
}

#[test]
fn contextual_codex_help_lists_speed_choices() {
    let help = help_text_for_args(&[
        "codex".to_string(),
        "daily".to_string(),
        "--help".to_string(),
    ]);

    assert!(help.contains("Show Codex token usage grouped by day"));
    assert!(help.contains("USAGE:\n  ccusage codex daily <OPTIONS>"));
    assert!(help.contains("choices: auto | standard | flex | fast"));
}

#[test]
fn contextual_codex_session_help_lists_id_option() {
    let help = help_text_for_args(&["codex".to_string(), "session".to_string()]);

    assert!(help.contains("-i, --id <id>"));
}

#[test]
fn contextual_agent_help_lists_agent_subcommands() {
    let help = help_text_for_args(&["claude".to_string()]);

    assert!(help.contains("USAGE:\n  ccusage claude <COMMANDS>"));
    assert!(help.contains("daily       Show usage report grouped by date"));
    assert!(help.contains("statusline  Display compact status line for Claude Code hooks"));
    assert!(help.contains("ccusage claude statusline --help"));
    assert!(!help.contains("ccusage claude daily <OPTIONS>"));
}

#[test]
fn contextual_all_agent_help_lists_color_options() {
    let help = help_text_for_args(&["daily".to_string()]);

    assert!(help.contains("--color"));
    assert!(help.contains("--no-color"));
}

#[test]
fn contextual_root_session_help_lists_id_option() {
    let help = help_text_for_args(&["session".to_string()]);

    assert!(help.contains("--id"));
}

#[test]
fn contextual_root_weekly_help_lists_start_of_week_option() {
    let help = help_text_for_args(&["weekly".to_string()]);

    assert!(help.contains("-w, --start-of-week [start-of-week]"));
    assert!(help.contains("default: monday"));

    let daily_help = help_text_for_args(&["daily".to_string()]);
    assert!(!daily_help.contains("--start-of-week"));
}

#[test]
fn focused_weekly_help_lists_week_options_for_every_weekly_runner() {
    for agent in AGENTS.iter().filter(|agent| agent.accepts("weekly")) {
        let help = help_text_for_args(&[agent.name.to_string(), "weekly".to_string()]);
        assert!(help.contains("--start-of-week"), "{}", agent.name);
        assert!(help.contains("--last"), "{}", agent.name);
    }
}

#[test]
fn contextual_statusline_help_lists_choice_options() {
    let help = help_text_for_args(&["statusline".to_string()]);

    assert!(help.contains("choices: off | emoji | text | emoji-text"));
    assert!(help.contains("choices: auto | ccusage | cc | both"));
}

#[test]
fn grok_help_does_not_advertise_path_option() {
    let help = help_text_for_args(&["grok".to_string(), "daily".to_string()]);

    assert!(!help.contains("--grok-path"));
}

#[test]
fn snapshots_root_and_contextual_help_text() {
    insta::assert_snapshot!("root_help", help_text());
    insta::assert_snapshot!(
        "claude_agent_help",
        help_text_for_args(&["claude".to_string()])
    );
    insta::assert_snapshot!(
        "codex_daily_help",
        help_text_for_args(&["codex".to_string(), "daily".to_string(),])
    );
    insta::assert_snapshot!(
        "statusline_help",
        help_text_for_args(&["statusline".to_string()])
    );
}

#[test]
fn option_metadata_matches_every_documented_flag_and_arity() {
    use ccusage_cli::OptionArity;
    use std::collections::{BTreeMap, BTreeSet};
    let help: Value = serde_json::from_str(include_str!("../cli-help.json")).unwrap();
    let mut arities = BTreeMap::<&str, BTreeSet<bool>>::new();
    for group in help.as_object().unwrap().values() {
        for spec in group.as_array().unwrap() {
            let flags = spec["flags"].as_str().unwrap();
            let takes_value = flags.contains(['<', '[']);
            for alias in flags.split(',') {
                arities
                    .entry(alias.split_whitespace().next().unwrap())
                    .or_default()
                    .insert(takes_value);
            }
        }
    }
    for (flag, arities) in arities {
        let expected = match arities.into_iter().collect::<Vec<_>>().as_slice() {
            [false] => OptionArity::Switch,
            [true] => OptionArity::Value,
            [false, true] => OptionArity::CommandDependent,
            _ => panic!("missing arity"),
        };
        assert_eq!(ccusage_cli::option_arity(flag), Some(expected), "{flag}");
    }
}
