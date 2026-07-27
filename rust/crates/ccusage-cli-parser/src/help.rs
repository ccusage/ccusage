use std::{path::Path, process};

use crate::parser::command_tokens;

struct HelpPage {
    path: &'static [&'static str],
    description: &'static str,
    usage: &'static str,
    options: Option<&'static str>,
    commands: &'static [(&'static str, &'static str)],
}

include!(concat!(env!("OUT_DIR"), "/cli-help.rs"));

pub(crate) fn print_version_and_exit(version: &str) -> ! {
    println!("ccusage {version}");
    process::exit(0);
}

pub(crate) fn print_help_and_exit(args: &[String]) -> ! {
    println!("{}", help_text_for_args(args));
    process::exit(0);
}

#[cfg(test)]
pub(crate) fn help_text() -> String {
    root_help_text()
}

pub(crate) fn help_text_for_args(args: &[String]) -> String {
    let args = strip_program_name(args);
    let tokens = command_tokens(args);
    help_text_for_tokens(&tokens)
}

fn strip_program_name(args: &[String]) -> &[String] {
    if args.first().is_some_and(|arg| is_program_name(arg)) {
        &args[1..]
    } else {
        args
    }
}

fn is_program_name(arg: &str) -> bool {
    let name = Path::new(arg)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(arg)
        .rsplit('\\')
        .next()
        .unwrap_or(arg);
    matches!(name, "ccusage" | "ccusage.exe")
}

fn help_text_for_tokens(tokens: &[String]) -> String {
    find_help_page(tokens).map_or_else(root_help_text, render_help_page)
}

fn find_help_page(tokens: &[String]) -> Option<&'static HelpPage> {
    HELP_PAGES.iter().find(|page| {
        page.path.len() == tokens.len()
            && page
                .path
                .iter()
                .zip(tokens)
                .all(|(expected, actual)| *expected == actual)
    })
}

fn root_help_text() -> String {
    let mut lines = vec!["USAGE:".to_string()];
    for usage in ROOT_USAGE {
        lines.push(format!("  {usage}"));
    }
    lines.push(String::new());
    lines.push("COMMANDS:".to_string());
    lines.extend(render_command_lines(ROOT_COMMANDS, 26));
    lines.push(String::new());
    lines.push("For more info, run any command with the `--help` flag:".to_string());
    for (command, _) in ROOT_COMMANDS {
        lines.push(format!("  ccusage {command} --help"));
    }
    lines.push(String::new());
    lines.push(all_agent_options().to_string());
    lines.join("\n")
}

fn render_help_page(page: &HelpPage) -> String {
    if page.commands.is_empty() {
        return command_help(
            page.description,
            page.usage,
            page.options
                .expect("command help pages with no subcommands require options"),
        );
    }

    let mut lines = vec![
        page.description.to_string(),
        String::new(),
        "USAGE:".to_string(),
        format!("  {}", page.usage),
        String::new(),
        "COMMANDS:".to_string(),
    ];
    lines.extend(render_command_lines(page.commands, 11));
    lines.push(String::new());
    lines.push("For more info, run any command with the `--help` flag:".to_string());
    let prefix = page.usage.trim_end_matches(" <COMMANDS>");
    for (command, _) in page.commands {
        lines.push(format!("  {prefix} {command} --help"));
    }
    lines.join("\n")
}

fn render_command_lines(commands: &[(&str, &str)], min_width: usize) -> Vec<String> {
    let width = commands
        .iter()
        .map(|(command, _)| command.len())
        .max()
        .unwrap_or(min_width)
        .max(min_width);
    commands
        .iter()
        .map(|(command, description)| format!("  {command:<width$} {description}"))
        .collect()
}

fn command_help(description: &str, usage: &str, options: &str) -> String {
    [
        description,
        "",
        "USAGE:",
        &format!("  {usage}"),
        "",
        options,
    ]
    .join("\n")
}
