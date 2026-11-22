use ccusage_cli::{AgentCommandKind, AgentId, Command, command_tokens};

use crate::arg_parser::ArgParser;

pub(super) fn report_flag_alias_error(args: &[String]) -> Option<String> {
    let flag = args.iter().find(|arg| {
        matches!(
            arg.as_str(),
            "--daily" | "--weekly" | "--monthly" | "--session" | "--blocks" | "--statusline"
        )
    })?;
    Some(format!(
        "Report flags like {flag} are not supported. Use \"ccusage {}\" instead.",
        flag.trim_start_matches("--")
    ))
}

pub(super) fn invocation_error(args: &[String]) -> Option<String> {
    let tokens = command_tokens(args);
    let extra = match tokens.as_slice() {
        [agent, report, ..] => AgentId::from_name(agent)
            .and_then(|agent| agent.spec().extra_command(report))
            .or_else(|| AgentId::Claude.spec().extra_command(agent)),
        [command] => AgentId::Claude.spec().extra_command(command),
        [] => None,
    };
    let blocks = extra.is_some_and(|command| matches!(command.kind, AgentCommandKind::Blocks(_)));
    if let Some(flag) = args.iter().find_map(|arg| {
        if arg == "--agent" || arg.starts_with("--agent=") {
            Some("--agent")
        } else if (arg == "-a" && !blocks) || arg.starts_with("-a=") {
            Some("-a")
        } else {
            None
        }
    }) {
        return Some(format!(
            "Agent filters like {flag} are not supported. Use \"ccusage <agent> <report>\", for example \"ccusage codex daily\"."
        ));
    }
    let [agent, report, ..] = tokens.as_slice() else {
        return None;
    };
    let agent = AgentId::from_name(agent)?;
    (!agent.spec().accepts(report)).then(|| unsupported_agent_report_error(agent, report))
}

fn unsupported_agent_report_error(agent: AgentId, report: &str) -> String {
    let spec = agent.spec();
    let agent = spec.name;
    let display = spec.label;
    let providers = ccusage_cli::AGENTS
        .iter()
        .filter(|provider| provider.extra_command(report).is_some())
        .map(|provider| provider.label)
        .collect::<Vec<_>>();
    if let Some((last, others)) = providers
        .split_last()
        .filter(|(_, others)| !others.is_empty())
    {
        format!(
            "The \"{report}\" command is available for {} and {last} usage.\nUse \"ccusage {agent} daily\" for {display} usage reports.",
            others.join(", ")
        )
    } else if let [only] = providers.as_slice() {
        format!(
            "The \"{report}\" report is only available for {only} usage.\nUse \"ccusage {agent} daily\" for {display} usage reports."
        )
    } else {
        format!(
            "The \"{report}\" report is not available for {display} usage.\nUse \"ccusage {agent} daily\" for {display} usage reports."
        )
    }
}

pub(super) fn last_option_error(command: &Command, parser: &ArgParser) -> Option<String> {
    let shared = command.shared()?;
    shared.last?;
    if command.period().is_none() {
        return Some(
            "The --last option is only available for the daily, weekly, and monthly reports."
                .to_string(),
        );
    }
    if parser.saw_flag(&["-s", "--since", "-u", "--until"]) {
        return Some("The --last option cannot be combined with --since or --until.".to_string());
    }
    if matches!(command, Command::All(args) if args.sections.is_some()) {
        return Some("The --last option cannot be used with --sections.".to_string());
    }
    None
}
