use std::{collections::BTreeMap, ffi::OsString, ops::ControlFlow};

use crate::arg_parser::ArgParser;
use crate::help::help_text_for_args;
use ccusage_cli::{
    AgentCommand, AgentCommandArgs, AgentCommandKind, AgentId, AgentReportKind, BlocksArgs,
    BlocksOption, ClaudeReport, ClaudeStatuslineOption, CliConfig, Command, ReportTimezone,
    SessionArgs, SessionLength, SharedArgs, StandardAgent, StatuslineArgs, WeekDay, WeeklyArgs,
};

mod agents;
mod errors;
mod shared;
mod values;

use agents::{parse_agent_command, parse_codex_statusline};
use errors::{invocation_error, last_option_error, report_flag_alias_error};
use shared::{SharedFlag, parse_shared_arg, parse_shared_arg_for_command, read_shared_flag};
use values::{
    parse_cost_source, parse_report_sections, parse_visual_burn_rate, parse_week_day,
    replace_configured_window_with_last, resolve_timezone, validate_date_window,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlArg {
    Help,
    Version,
}

#[derive(Default)]
struct RootOptions {
    shared_flags: Vec<SharedFlag>,
    sections: Option<Vec<AgentReportKind>>,
    by_agent: bool,
    first_flag: Option<&'static str>,
}

impl RootOptions {
    fn shared(&mut self, config: &dyn CliConfig, offline_by_default: bool) -> SharedArgs {
        let mut shared = SharedArgs::with_defaults();
        shared.offline = offline_by_default;
        config.apply_shared(&mut shared);
        for flag in self.shared_flags.drain(..) {
            flag.apply(&mut shared);
        }
        shared
    }

    fn mark_used(&mut self, flag: &'static str) {
        self.first_flag.get_or_insert(flag);
    }

    fn is_used(&self) -> bool {
        self.first_flag.is_some()
    }

    fn first_flag(&self) -> &'static str {
        self.first_flag.unwrap_or("--sections")
    }

    fn into_agent_args(self, shared: SharedArgs, kind: AgentReportKind) -> AgentCommandArgs {
        AgentCommandArgs {
            sections: self.sections,
            by_agent: self.by_agent,
            ..agent_command_args(shared, kind)
        }
    }
}

pub fn parse_cli<I>(
    args: I,
    config: &dyn CliConfig,
    version: &str,
) -> Result<ControlFlow<String, Command>, String>
where
    I: IntoIterator<Item = OsString>,
{
    let parser = argument_parser(args)?;
    match control_arg(&parser.args) {
        Some(ControlArg::Version) => Ok(ControlFlow::Break(format!("ccusage {version}"))),
        Some(ControlArg::Help) => Ok(ControlFlow::Break(help_text_for_args(&parser.args))),
        None => parse_with_timezone(parser, config, None).map(ControlFlow::Continue),
    }
}

pub fn parse_from_with_config<I>(args: I, config: &dyn CliConfig) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    parse_with_timezone(argument_parser(args)?, config, None)
}

fn argument_parser(args: impl IntoIterator<Item = OsString>) -> Result<ArgParser, String> {
    let mut parser = ArgParser::new(args.into_iter().skip(1).collect())?;
    normalize_legacy_agent_command_args(&mut parser.args);
    Ok(parser)
}

fn parse_with_timezone(
    mut parser: ArgParser,
    config: &dyn CliConfig,
    known_timezone: Option<&ReportTimezone>,
) -> Result<Command, String> {
    if let Some(message) = report_flag_alias_error(&parser.args) {
        return Err(message);
    }
    parse_invocation(&mut parser, config, known_timezone)
        .map_err(|message| invocation_error(&parser.args).unwrap_or(message))
}

fn parse_invocation(
    parser: &mut ArgParser,
    config: &dyn CliConfig,
    known_timezone: Option<&ReportTimezone>,
) -> Result<Command, String> {
    if let Some(message) = config.config_error() {
        return Err(message.to_string());
    }
    let mut root_options = RootOptions::default();
    while let Some(arg) = parser.peek() {
        if !arg.starts_with('-') {
            break;
        }
        if parse_root_all_arg(parser, &mut root_options)? {
            continue;
        }
        root_options.shared_flags.push(read_shared_flag(parser)?);
    }

    let mut command = match parser.next() {
        None => {
            let shared = root_options.shared(config, false);
            Command::All(root_options.into_agent_args(shared, AgentReportKind::Daily))
        }
        Some(command) => parse_command(&command, parser, config, root_options)?,
    };
    parser.finish()?;
    if let Some(message) = last_option_error(&command, parser) {
        return Err(message);
    }
    replace_configured_window_with_last(&mut command);
    validate_date_window(&mut command, parser, config)?;
    resolve_timezone(&mut command, parser, config, known_timezone)?;
    if let Command::Claude { project, .. } = &mut command
        && project.is_none()
    {
        *project = config.agent_options("claude").claude_project;
    }
    Ok(command)
}

pub fn parse_report_args_with_config<I>(
    args: I,
    config: &dyn CliConfig,
    start_of_week: WeekDay,
    known_timezone: Option<&ReportTimezone>,
) -> Result<AgentCommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let (Command::All(args) | Command::Agent(_, AgentCommand::Report(args))) =
        parse_with_timezone(argument_parser(args)?, config, known_timezone)?
    else {
        return Err("Expected a report that loads agent sources".to_string());
    };
    Ok(AgentCommandArgs {
        start_of_week,
        ..args
    })
}

fn control_arg(args: &[String]) -> Option<ControlArg> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-v" | "-V" | "--version"))
    {
        return Some(ControlArg::Version);
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Some(ControlArg::Help);
    }
    None
}

fn parse_command(
    command: &str,
    parser: &mut ArgParser,
    config: &dyn CliConfig,
    mut root_options: RootOptions,
) -> Result<Command, String> {
    if let Some(agent) = AgentId::from_name(command) {
        return parse_agent_command(agent, parser, config, root_options);
    }
    if let Some(command) = AgentId::Claude.spec().extra_command(command) {
        return parse_extra_command(command.kind, parser, config, root_options);
    }
    let shared = root_options.shared(config, false);
    match command {
        "daily" => parse_all_command(parser, shared, AgentReportKind::Daily, config, root_options),
        "monthly" => parse_all_command(
            parser,
            shared,
            AgentReportKind::Monthly,
            config,
            root_options,
        ),
        "weekly" => parse_all_command(
            parser,
            shared,
            AgentReportKind::Weekly,
            config,
            root_options,
        ),
        "session" => parse_top_level_session_command(parser, shared, config, root_options),
        _ => Err(format!("Unknown command '{command}'")),
    }
}

fn parse_extra_command(
    kind: AgentCommandKind,
    parser: &mut ArgParser,
    config: &dyn CliConfig,
    mut root: RootOptions,
) -> Result<Command, String> {
    if root.is_used() && !matches!(kind, AgentCommandKind::ClaudeStatusline { .. }) {
        return Err(format!("Unknown option '{}'", root.first_flag()));
    }
    match kind {
        AgentCommandKind::Blocks(options) => {
            parse_blocks_command(parser, root.shared(config, false), config, options)
        }
        AgentCommandKind::ClaudeStatusline { options, .. } => {
            let offline_override = root.shared_flags.iter().rev().find_map(|flag| {
                if let SharedFlag::Offline(offline) = flag {
                    Some(*offline)
                } else {
                    None
                }
            });
            let shared = root.shared(config, false);
            parse_statusline_command(parser, &shared, config, offline_override, options)
        }
        AgentCommandKind::CodexStatusline {
            offline_by_default,
            options,
        } => {
            let shared = root.shared(config, offline_by_default);
            let speed = config
                .agent_options(StandardAgent::Codex.spec().name)
                .codex_speed;
            parse_codex_statusline(parser, shared, speed, options)
        }
    }
}

fn command_option<T: Copy>(
    parser: &mut ArgParser,
    options: &[(&str, T)],
    command: &str,
) -> Result<T, String> {
    let flag = parser.next_flag()?;
    options
        .iter()
        .find_map(|&(name, option)| (name == flag).then_some(option))
        .ok_or_else(|| format!("Unknown {command} option '{flag}'"))
}

fn parse_blocks_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
    options: &[(&str, BlocksOption)],
) -> Result<Command, String> {
    let mut args = BlocksArgs {
        shared,
        active: false,
        recent: false,
        token_limit: None,
        session_length: SessionLength::DEFAULT,
    };
    config.apply_blocks_args(&mut args);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match command_option(parser, options, "blocks")? {
            BlocksOption::Active => args.active = true,
            BlocksOption::Recent => args.recent = true,
            BlocksOption::TokenLimit => {
                args.token_limit = Some(
                    parser
                        .value_for("--token-limit")?
                        .parse()
                        .map_err(|error| format!("Invalid value for --token-limit: {error}"))?,
                )
            }
            BlocksOption::SessionLength => {
                args.session_length = parser
                    .value_for("--session-length")?
                    .parse()
                    .map_err(|error| format!("Invalid value for --session-length: {error}"))?
            }
        }
    }
    Ok(Command::claude_report(ClaudeReport::Blocks(args)))
}

fn parse_statusline_command(
    parser: &mut ArgParser,
    shared: &SharedArgs,
    config: &dyn CliConfig,
    offline_override: Option<bool>,
    options: &[(&str, ClaudeStatuslineOption)],
) -> Result<Command, String> {
    let mut args = StatuslineArgs::default();
    config.apply_statusline_args(&mut args);
    if let Some(offline) = offline_override {
        args.offline = offline;
    }
    args.debug = shared.debug;
    args.color = shared.color;
    args.mode = shared.mode;
    args.pricing_overrides = shared.pricing_overrides.clone();
    let mut low = None;
    let mut medium = None;
    while parser.peek().is_some() {
        match command_option(parser, options, "statusline")? {
            ClaudeStatuslineOption::Offline => args.offline = true,
            ClaudeStatuslineOption::NoOffline => args.offline = false,
            ClaudeStatuslineOption::VisualBurnRate => {
                args.visual_burn_rate =
                    parse_visual_burn_rate(&parser.value_for("--visual-burn-rate")?)?
            }
            ClaudeStatuslineOption::CostSource => {
                args.cost_source = parse_cost_source(&parser.value_for("--cost-source")?)?
            }
            ClaudeStatuslineOption::Cache => args.cache = true,
            ClaudeStatuslineOption::NoCache => args.cache = false,
            ClaudeStatuslineOption::RefreshInterval => {
                args.refresh_interval = parser
                    .value_for("--refresh-interval")?
                    .parse()
                    .map_err(|_| "Invalid value for --refresh-interval".to_string())?
            }
            ClaudeStatuslineOption::ContextLowThreshold => {
                low = Some(
                    parser
                        .value_for("--context-low-threshold")?
                        .parse()
                        .map_err(|_| "Invalid value for --context-low-threshold".to_string())?,
                )
            }
            ClaudeStatuslineOption::ContextMediumThreshold => {
                medium = Some(
                    parser
                        .value_for("--context-medium-threshold")?
                        .parse()
                        .map_err(|_| "Invalid value for --context-medium-threshold".to_string())?,
                )
            }
            ClaudeStatuslineOption::Timezone => parser.read_timezone()?,
            ClaudeStatuslineOption::Config => {
                parser.value_for("--config")?;
            }
            ClaudeStatuslineOption::Debug => args.debug = true,
        }
    }
    args.context_thresholds = config.context_thresholds(low, medium)?;
    Ok(Command::Statusline(args))
}

fn parse_root_all_arg(parser: &mut ArgParser, options: &mut RootOptions) -> Result<bool, String> {
    if let Some(flag) =
        parse_unified_report_arg(parser, &mut options.sections, &mut options.by_agent)?
    {
        options.mark_used(flag);
        return Ok(true);
    }
    Ok(false)
}

fn parse_unified_report_arg(
    parser: &mut ArgParser,
    sections: &mut Option<Vec<AgentReportKind>>,
    by_agent: &mut bool,
) -> Result<Option<&'static str>, String> {
    if matches!(parser.peek_name(), Some("--all")) {
        parser.next_flag()?;
        return Ok(Some("--all"));
    }
    if matches!(parser.peek_name(), Some("--sections")) {
        parser.next_flag()?;
        *sections = Some(parse_report_sections(&parser.value_for("--sections")?)?);
        return Ok(Some("--sections"));
    }
    if matches!(parser.peek_name(), Some("--by-agent")) {
        parser.next_flag()?;
        *by_agent = true;
        return Ok(Some("--by-agent"));
    }
    Ok(None)
}

fn parse_all_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    kind: AgentReportKind,
    config: &dyn CliConfig,
    initial_options: RootOptions,
) -> Result<Command, String> {
    let mut sections = initial_options.sections;
    let mut by_agent = initial_options.by_agent;
    let mut start_of_week = WeekDay::Monday;
    if kind == AgentReportKind::Weekly {
        let mut weekly = WeeklyArgs {
            shared: SharedArgs::with_defaults(),
            start_of_week,
        };
        config.apply_weekly_args(&mut weekly);
        start_of_week = weekly.start_of_week;
    }
    while parser.peek().is_some() {
        if parse_unified_report_arg(parser, &mut sections, &mut by_agent)?.is_some() {
            continue;
        }
        if kind == AgentReportKind::Weekly
            && matches!(parser.peek_name(), Some("-w" | "--start-of-week"))
        {
            parser.next_flag()?;
            start_of_week = parse_week_day(&parser.value_for("--start-of-week")?)?;
            continue;
        }
        parse_shared_arg(parser, &mut shared)?;
    }
    Ok(Command::All(AgentCommandArgs {
        sections,
        by_agent,
        start_of_week,
        ..agent_command_args(shared, kind)
    }))
}

fn parse_top_level_session_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    _config: &dyn CliConfig,
    initial_options: RootOptions,
) -> Result<Command, String> {
    let mut args = SessionArgs { shared, id: None };
    let mut sections = initial_options.sections;
    let mut by_agent = initial_options.by_agent;
    while parser.peek().is_some() {
        if parse_unified_report_arg(parser, &mut sections, &mut by_agent)?.is_some() {
            continue;
        }
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-i" | "--id" => args.id = Some(parser.value_for("--id")?),
            flag => return Err(format!("Unknown session option '{flag}'")),
        }
    }

    if args.id.is_some() {
        if sections.is_some() || by_agent {
            return Err(
                "The --sections and --by-agent options cannot be used with session --id."
                    .to_string(),
            );
        }
        return Ok(Command::claude_report(ClaudeReport::Session(args)));
    }

    Ok(Command::All(AgentCommandArgs {
        sections,
        by_agent,
        ..agent_command_args(args.shared, AgentReportKind::Session)
    }))
}

fn agent_command_args(shared: SharedArgs, kind: AgentReportKind) -> AgentCommandArgs {
    AgentCommandArgs {
        options: Default::default(),
        shared,
        kind,
        session_id: None,
        sections: None,
        by_agent: false,

        include_claude_agent: false,
        start_of_week: WeekDay::Monday,
        sources: BTreeMap::new(),
    }
}

fn normalize_legacy_agent_command_args(args: &mut Vec<String>) {
    let Some(command) = args.first() else {
        return;
    };
    let Some((agent, report)) = AgentId::from_colon_command(command) else {
        return;
    };
    args.splice(0..1, [agent.spec().name.to_string(), report.to_string()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn detects_help_before_semantic_validation() {
        assert_eq!(
            control_arg(&args(&["--help", "--daily"])),
            Some(ControlArg::Help)
        );
    }

    #[test]
    fn version_takes_precedence_over_help() {
        assert_eq!(
            control_arg(&args(&["--help", "--version"])),
            Some(ControlArg::Version)
        );
    }
}
