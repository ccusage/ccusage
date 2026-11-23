use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use ccusage_cli::{AGENTS, AgentReportKind};
use ccusage_environment::Environment;
use serde_json::{Value, json};

#[path = "src/help/codegen.rs"]
mod help_codegen;

const CLI_HELP_JSON: &str = "src/cli-help.json";
const CLI_COMMANDS_JSON: &str = "src/cli-commands.json";
const ROOT_NON_REPORT_COMMANDS: &[&str] = &["telemetry"];

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-changed={CLI_HELP_JSON}");
    println!("cargo:rerun-if-changed={CLI_COMMANDS_JSON}");
    generate_cli_help_rs()
}

fn out_dir_path(file_name: &str) -> Result<PathBuf, String> {
    let out_dir = Environment::current()
        .var_os("OUT_DIR")
        .ok_or_else(|| "OUT_DIR is set by cargo".to_string())?;
    Ok(PathBuf::from(out_dir).join(file_name))
}

fn generate_cli_help_rs() -> Result<(), String> {
    let option_source = fs::read_to_string(CLI_HELP_JSON)
        .map_err(|error| format!("read CLI help spec: {error}"))?;
    let command_source = fs::read_to_string(CLI_COMMANDS_JSON)
        .map_err(|error| format!("read CLI command spec: {error}"))?;
    let option_sets = serde_json::from_str::<Value>(&option_source)
        .map_err(|error| format!("parse CLI help spec: {error}"))?;
    let mut command_spec = serde_json::from_str::<Value>(&command_source)
        .map_err(|error| format!("parse CLI command spec: {error}"))?;
    add_focused_weekly_pages(&mut command_spec)?;
    check_agent_pages(&command_spec)?;
    let output = help_codegen::generate_cli_help_source(&option_sets, &command_spec)?;
    fs::write(out_dir_path("cli-help.rs")?, output)
        .map_err(|error| format!("write generated CLI help: {error}"))
}

fn add_focused_weekly_pages(command_spec: &mut Value) -> Result<(), String> {
    let pages = command_spec["pages"]
        .as_array_mut()
        .ok_or_else(|| "CLI command spec has no pages".to_string())?;
    for agent in AGENTS {
        let name = agent.name;
        if pages
            .iter()
            .any(|page| page["path"] == json!([name, "weekly"]))
        {
            continue;
        }
        let monthly = pages
            .iter()
            .find(|page| page["path"] == json!([name, "monthly"]))
            .ok_or_else(|| format!("missing monthly help for {name}"))?;
        let description = monthly["description"]
            .as_str()
            .and_then(|text| text.strip_suffix(" by month"))
            .ok_or_else(|| format!("missing month grouping description for {name}"))?;
        let description = format!("{description} by week");
        let options = monthly["options"]
            .as_str()
            .and_then(|name| name.strip_suffix("_period_options"))
            .ok_or_else(|| format!("missing period options for {name}"))?;
        let page = json!({
            "path": [name, "weekly"],
            "description": description,
            "usage": format!("ccusage {name} weekly <OPTIONS>"),
            "options": format!("{options}_weekly_options"),
        });
        let commands = pages
            .iter_mut()
            .find(|page| page["path"] == json!([name]))
            .and_then(|page| page["commands"].as_array_mut())
            .ok_or_else(|| format!("missing help commands for {name}"))?;
        let monthly_index = commands
            .iter()
            .position(|command| command["name"] == "monthly")
            .ok_or_else(|| format!("missing monthly command in help for {name}"))?;
        commands.insert(
            monthly_index,
            json!({ "name": "weekly", "description": description }),
        );
        pages.push(page);
    }
    Ok(())
}

fn page_path(page: &Value) -> Result<Vec<&str>, String> {
    page["path"]
        .as_array()
        .ok_or_else(|| "page has a path".to_string())?
        .iter()
        .map(|segment| {
            segment
                .as_str()
                .ok_or_else(|| "page path is text".to_string())
        })
        .collect()
}

fn command_names(commands: &[Value]) -> Result<impl Iterator<Item = &str>, String> {
    let names = commands
        .iter()
        .map(|command| {
            command["name"]
                .as_str()
                .ok_or_else(|| "command has a name".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names.into_iter())
}

fn check_agent_pages(command_spec: &Value) -> Result<(), String> {
    let pages = command_spec["pages"]
        .as_array()
        .ok_or_else(|| "CLI command spec has pages".to_string())?;
    let mut index_pages = BTreeMap::new();
    let mut leaf_pages = BTreeMap::<&str, BTreeSet<&str>>::new();
    for page in pages {
        match page_path(page)?[..] {
            [agent] if ROOT_NON_REPORT_COMMANDS.contains(&agent) => {}
            [agent] => {
                if let Some(commands) = page["commands"].as_array() {
                    index_pages.insert(agent, command_names(commands)?.collect::<BTreeSet<_>>());
                }
            }
            [agent, command] => {
                leaf_pages.entry(agent).or_default().insert(command);
            }
            _ => {}
        }
    }
    let table = AGENTS
        .iter()
        .map(|spec| {
            let commands = AgentReportKind::ALL
                .into_iter()
                .map(|kind| kind.name())
                .chain(spec.extra_commands.iter().map(|command| command.name))
                .collect::<BTreeSet<_>>();
            (spec.name, commands)
        })
        .collect::<BTreeMap<_, _>>();
    if index_pages != table {
        return Err(format!(
            "{CLI_COMMANDS_JSON} agent pages differ from ccusage_cli::AGENTS\n  help:  {index_pages:?}\n  table: {table:?}"
        ));
    }
    if leaf_pages != table {
        return Err(format!(
            "{CLI_COMMANDS_JSON} agent command pages differ from ccusage_cli::AGENTS\n  help:  {leaf_pages:?}\n  table: {table:?}"
        ));
    }

    let root = command_names(
        command_spec["root"]["commands"]
            .as_array()
            .ok_or_else(|| "CLI command spec has root commands".to_string())?,
    )?
    .collect::<Vec<_>>();
    let agent_names = AGENTS.iter().map(|spec| spec.name).collect::<BTreeSet<_>>();
    let root_agents = root
        .iter()
        .copied()
        .filter(|name| agent_names.contains(name))
        .collect::<BTreeSet<_>>();
    if root_agents != agent_names {
        return Err(format!(
            "{CLI_COMMANDS_JSON} root agent commands differ from ccusage_cli::AGENTS\n  help:  {root_agents:?}\n  table: {agent_names:?}"
        ));
    }
    let root_reports = root
        .iter()
        .copied()
        .filter(|name| !agent_names.contains(name) && !ROOT_NON_REPORT_COMMANDS.contains(name))
        .collect::<BTreeSet<_>>();
    let default_agent = &table[AGENTS[0].name];
    if &root_reports != default_agent {
        return Err(format!(
            "{CLI_COMMANDS_JSON} root report commands differ from {}'s in ccusage_cli::AGENTS\n  help:  {root_reports:?}\n  table: {default_agent:?}",
            AGENTS[0].name
        ));
    }
    Ok(())
}
