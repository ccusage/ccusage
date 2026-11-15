//! Golden matrix over the release binary: one test per agent, the unified
//! reports over every agent at once, and a guard that fails when the CLI
//! accepts an (agent, report) pair nothing here snapshots.
//!
//! Snapshots are `snapshots/golden__<agent|all>__<kind>__<variant>.snap`.

use std::collections::BTreeSet;

use ccusage_test_support::Fixture;

mod golden;

use golden::{ALL_REPORT_KINDS, AgentMatrix, Case, UNSUPPORTED_VARIANT, agent_cases, run_case};

/// Declares the agents of the matrix once, as both the table the coverage
/// guard checks against the CLI and one `#[test]` per agent, so an agent
/// cannot be in the table without being run.
macro_rules! agent_matrix {
    ($($agent:ident => $extras:expr),* $(,)?) => {
        const AGENTS: &[AgentMatrix] = &[
            $(AgentMatrix {
                agent: stringify!($agent),
                seed: golden::$agent::seed,
                extras: $extras,
            }),*
        ];

        mod agent {
            use super::golden;

            $(
                #[test]
                fn $agent() {
                    golden::run_matrix(stringify!($agent), golden::$agent::seed, $extras);
                }
            )*
        }
    };
}

agent_matrix! {
    claude => golden::claude::extra_cases,
    codex => golden::codex::extra_cases,
    opencode => golden::opencode::extra_cases,
    amp => golden::no_extras,
    droid => golden::no_extras,
    codebuff => golden::no_extras,
    hermes => golden::no_extras,
    pi => golden::pi::extra_cases,
    goose => golden::no_extras,
    kilo => golden::no_extras,
    copilot => golden::no_extras,
    gemini => golden::no_extras,
    antigravity => golden::antigravity::extra_cases,
    kimi => golden::no_extras,
    qwen => golden::no_extras,
    openclaw => golden::openclaw::extra_cases,
    grok => golden::no_extras,
    zcode => golden::no_extras,
    cline => golden::no_extras,
}

#[test]
fn all_agents_unified() {
    golden::all::run(AGENTS);
}

/// The names under `COMMANDS:` in a `--help` page.
fn help_commands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| line.trim() != "COMMANDS:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_accepted_agent_report_is_in_the_matrix() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let run = |args: &[&str]| run_case(root, &[], &Case::new("", "", args));

    let commands = help_commands(&run(&["--help"]).stdout);
    assert!(
        !commands.is_empty(),
        "`ccusage --help` lists no commands; the help layout this guard parses has changed"
    );

    let (kinds, agents): (BTreeSet<&str>, BTreeSet<&str>) = commands
        .iter()
        .map(String::as_str)
        .partition(|command| ALL_REPORT_KINDS.contains(command));
    assert_eq!(
        kinds,
        BTreeSet::from(ALL_REPORT_KINDS),
        "top-level report commands in `ccusage --help` differ from golden::ALL_REPORT_KINDS"
    );
    assert_eq!(
        agents,
        AGENTS.iter().map(|matrix| matrix.agent).collect(),
        "agent commands in `ccusage --help` differ from the agents in agent_matrix!; \
         a new agent needs a seed in tests/golden and an entry there"
    );

    let mut uncovered = Vec::new();
    for matrix in AGENTS {
        let snapshotted: BTreeSet<&str> = agent_cases(matrix.agent, matrix.extras, root)
            .iter()
            .filter(|case| {
                case.variant != UNSUPPORTED_VARIANT
                    && case.args.first().map(String::as_str) == Some(matrix.agent)
                    && case.args.get(1).map(String::as_str) == Some(case.kind)
            })
            .map(|case| case.kind)
            .collect();
        for kind in ALL_REPORT_KINDS {
            let probe = run(&[matrix.agent, kind, "--offline", "--json"]);
            let rejected = probe.exit == "2"
                && probe
                    .stderr
                    .starts_with(&format!("The \"{kind}\" report is "));
            if !rejected && !snapshotted.contains(kind) {
                uncovered.push(format!("{} {kind}", matrix.agent));
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "the CLI accepts these reports but the golden matrix does not snapshot them \
         (see golden::report_kinds): {uncovered:?}"
    );
}
