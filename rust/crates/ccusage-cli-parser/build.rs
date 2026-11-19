use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use ccusage_cli::AGENTS;
use ccusage_environment::Environment;
use serde_json::Value;

#[path = "src/help_codegen.rs"]
mod help_codegen;

const CLI_HELP_JSON: &str = "src/cli-help.json";
const CLI_COMMANDS_JSON: &str = "src/cli-commands.json";

fn main() {
    println!("cargo:rerun-if-changed={CLI_HELP_JSON}");
    println!("cargo:rerun-if-changed={CLI_COMMANDS_JSON}");
    generate_cli_help_rs();
}

fn out_dir_path(file_name: &str) -> PathBuf {
    PathBuf::from(
        Environment::current()
            .var_os("OUT_DIR")
            .expect("OUT_DIR is set by cargo"),
    )
    .join(file_name)
}

fn generate_cli_help_rs() {
    let option_source = fs::read_to_string(CLI_HELP_JSON).expect("read CLI help spec");
    let command_source = fs::read_to_string(CLI_COMMANDS_JSON).expect("read CLI command spec");
    let option_sets = serde_json::from_str::<Value>(&option_source).expect("parse CLI help spec");
    let command_spec =
        serde_json::from_str::<Value>(&command_source).expect("parse CLI command spec");
    check_agent_pages(&command_spec);
    let output = help_codegen::generate_cli_help_source(&option_sets, &command_spec);
    fs::write(out_dir_path("cli-help.rs"), output).expect("write generated CLI help");
}

fn page_path(page: &Value) -> Vec<&str> {
    page["path"]
        .as_array()
        .expect("page has a path")
        .iter()
        .map(|segment| segment.as_str().expect("page path is text"))
        .collect()
}

fn check_agent_pages(command_spec: &Value) {
    let pages = command_spec["pages"]
        .as_array()
        .expect("CLI command spec has pages");
    let index_pages = pages
        .iter()
        .filter_map(|page| {
            let [agent] = page_path(page)[..] else {
                return None;
            };
            let commands = page["commands"]
                .as_array()?
                .iter()
                .map(|command| command["name"].as_str().expect("command has a name"))
                .collect::<BTreeSet<_>>();
            Some((agent, commands))
        })
        .collect::<BTreeMap<_, _>>();
    let mut leaf_pages = BTreeMap::<&str, BTreeSet<&str>>::new();
    for page in pages {
        if let [agent, command] = page_path(page)[..] {
            leaf_pages.entry(agent).or_default().insert(command);
        }
    }
    let table = AGENTS
        .iter()
        .map(|spec| {
            let commands = spec
                .reports
                .iter()
                .map(|kind| kind.name())
                .chain(spec.extra_commands.iter().map(|command| command.name))
                .collect::<BTreeSet<_>>();
            (spec.name, commands)
        })
        .collect::<BTreeMap<_, _>>();
    assert!(
        index_pages == table,
        "{CLI_COMMANDS_JSON} agent pages differ from ccusage_cli::AGENTS\n  help:  {index_pages:?}\n  table: {table:?}"
    );
    assert!(
        leaf_pages == table,
        "{CLI_COMMANDS_JSON} agent command pages differ from ccusage_cli::AGENTS\n  help:  {leaf_pages:?}\n  table: {table:?}"
    );

    let root = command_spec["root"]["commands"]
        .as_array()
        .expect("CLI command spec has root commands")
        .iter()
        .map(|command| command["name"].as_str().expect("command has a name"))
        .collect::<Vec<_>>();
    let agent_names = AGENTS.iter().map(|spec| spec.name).collect::<BTreeSet<_>>();
    let root_agents = root
        .iter()
        .copied()
        .filter(|name| agent_names.contains(name))
        .collect::<BTreeSet<_>>();
    assert!(
        root_agents == agent_names,
        "{CLI_COMMANDS_JSON} root agent commands differ from ccusage_cli::AGENTS\n  help:  {root_agents:?}\n  table: {agent_names:?}"
    );
    let root_reports = root
        .iter()
        .copied()
        .filter(|name| !agent_names.contains(name))
        .collect::<BTreeSet<_>>();
    let default_agent = &table[AGENTS[0].name];
    assert!(
        &root_reports == default_agent,
        "{CLI_COMMANDS_JSON} root report commands differ from {}'s in ccusage_cli::AGENTS\n  help:  {root_reports:?}\n  table: {default_agent:?}",
        AGENTS[0].name
    );
}
