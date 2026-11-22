use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use ccusage_cli::{command_has_flag, command_tokens};

use super::{ConfigCommand, config_error, pi_stores::parse_named_pi_stores};

pub(super) fn load_config_value(
    path: Option<&Path>,
) -> (
    Option<crate::layers::ConfigFile>,
    Option<String>,
    Vec<String>,
) {
    load_config_paths(
        path.is_some(),
        match path {
            Some(path) => vec![path.to_path_buf()],
            None => discover_config_paths(),
        },
    )
}

pub(super) fn load_config_paths(
    explicit: bool,
    paths: Vec<PathBuf>,
) -> (
    Option<crate::layers::ConfigFile>,
    Option<String>,
    Vec<String>,
) {
    let mut warnings = Vec::new();
    for path in paths {
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                return (None, Some(message), warnings);
            }
        };
        let value = serde_json::from_str::<Value>(&content)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                if value.is_object() {
                    Ok(value)
                } else {
                    Err("$: expected an object".to_string())
                }
            });
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                if explicit {
                    return (None, Some(message), warnings);
                }
                warnings.push(message);
                continue;
            }
        };
        let file = if explicit {
            crate::layers::ConfigFile::parse(value)
        } else {
            crate::layers::ConfigFile::parse_discovered(value)
        };
        match file {
            Ok(mut file) => {
                for layer in file.layers.values_mut() {
                    if let Some(error) = &mut layer.date_error {
                        *error = config_error(format!("{}: {error}", path.display()));
                    }
                }
                match parse_named_pi_stores(&file.value) {
                    Ok(stores) => file.pi_stores = stores,
                    Err(error) => {
                        let detail = error
                            .strip_prefix("Invalid ccusage config: ")
                            .unwrap_or(&error);
                        let message = config_error(format!("{}: {detail}", path.display()));
                        if explicit {
                            return (None, Some(message), warnings);
                        }
                        file.pi_stores_error = Some(message);
                    }
                }
                file.path = path.clone();
                warnings.extend(
                    file.warnings
                        .drain(..)
                        .map(|warning| format!("{}: {warning}", path.display())),
                );
                return (Some(file), None, warnings);
            }
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                return (None, Some(message), warnings);
            }
        }
    }
    (None, None, warnings)
}

fn discover_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join(".ccusage").join("ccusage.json"));
    }
    paths.extend(
        ccusage_cli::home::claude_config_dirs()
            .into_dirs()
            .into_iter()
            .map(|dir| dir.join("ccusage.json")),
    );
    paths
}

pub(super) fn scan_config_path(args: &[String]) -> Option<PathBuf> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some((flag, value)) = arg.split_once('=') {
            if flag == "--config" && !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        } else if arg == "--config" {
            return args.get(index + 1).map(PathBuf::from);
        }
        index += 1;
    }
    None
}

pub(super) fn detect_config_command(args: &[String]) -> ConfigCommand {
    let tokens = command_tokens(args);
    let Some(first) = tokens.first() else {
        return ConfigCommand {
            raw: "daily".to_string(),
            agent: None,
            report: "daily".to_string(),
        };
    };
    if let Some((agent, report)) = ccusage_cli::AgentId::from_colon_command(first) {
        return ConfigCommand {
            raw: format!("{} {report}", agent.spec().name),
            agent: Some(agent.spec().name.to_string()),
            report: report.to_string(),
        };
    }
    if is_agent_command(first) {
        let report = tokens
            .get(1)
            .filter(|token| is_report_command(token))
            .cloned()
            .unwrap_or_else(|| "daily".to_string());
        return ConfigCommand {
            raw: format!("{first} {report}"),
            agent: Some(first.clone()),
            report,
        };
    }
    if first == "blocks" || (first == "session" && command_has_flag(args, &["--id", "-i"])) {
        return ConfigCommand {
            raw: format!("claude {first}"),
            agent: Some("claude".to_string()),
            report: first.clone(),
        };
    }
    ConfigCommand {
        raw: first.clone(),
        agent: None,
        report: first.clone(),
    }
}

fn is_agent_command(command: &str) -> bool {
    ccusage_cli::BUILT_IN_AGENT_NAMES.contains(&command)
}

fn is_report_command(command: &str) -> bool {
    matches!(
        command,
        "daily" | "monthly" | "weekly" | "session" | "blocks" | "statusline"
    )
}
