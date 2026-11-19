use std::{collections::BTreeMap, ffi::OsString, num::NonZeroU32};

use crate::arg_parser::ArgParser;
use crate::help::{print_help_and_exit, print_version_and_exit};
use ccusage_cli::{
    AgentCommand, AgentCommandArgs, AgentCommandKind, AgentId, AgentReportKind, BlocksArgs,
    BlocksOption, ClaudeStatuslineOption, CliConfig, CodexSpeed, CodexStatuslineArgs,
    CodexStatuslineOption, ColorChoice, Command, CostMode, CostSource, DATE_BOUND_FORMATS,
    DailyArgs, SessionArgs, SessionLength, SharedArgs, SortOrder, StandardAgent, StatuslineArgs,
    VisualBurnRate, WeekDay, WeeklyArgs, normalize_date_bound,
};
use ccusage_cli::{command_tokens, is_shared_flag};

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

pub fn parse_from_with_config<I>(
    args: I,
    config: &dyn CliConfig,
    version: &'static str,
) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut parser = ArgParser::new(args.into_iter().skip(1).collect())?;
    normalize_legacy_agent_command_args(&mut parser.args);
    match control_arg(&parser.args) {
        Some(ControlArg::Version) => print_version_and_exit(version),
        Some(ControlArg::Help) => print_help_and_exit(&parser.args),
        None => {}
    }
    if let Some(message) = report_flag_alias_error(&parser.args) {
        return Err(message);
    }
    parse_invocation(&mut parser, config)
        .map_err(|message| invocation_error(&parser.args).unwrap_or(message))
}

fn parse_invocation(parser: &mut ArgParser, config: &dyn CliConfig) -> Result<Command, String> {
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
    if let Some(message) = last_option_error(&command) {
        return Err(message);
    }
    validate_date_window(&mut command, parser, config)?;
    validate_timezone(&mut command, parser, config)?;
    Ok(command)
}

pub fn parse_report_args_with_config<I>(
    args: I,
    config: &dyn CliConfig,
    version: &'static str,
    start_of_week: WeekDay,
) -> Result<AgentCommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    match parse_from_with_config(args, config, version)? {
        Command::All(args) | Command::Agent(_, AgentCommand::Report(args)) => {
            Ok(AgentCommandArgs {
                start_of_week,
                ..args
            })
        }
        _ => Err("Expected a report that loads agent sources".to_string()),
    }
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

fn parse_agent_command(
    agent: AgentId,
    parser: &mut ArgParser,
    config: &dyn CliConfig,
    mut root_options: RootOptions,
) -> Result<Command, String> {
    let extra = parser
        .peek()
        .and_then(|name| agent.spec().extra_command(name));
    if let Some(command) = extra {
        parser.next();
        return parse_extra_command(command.kind, parser, config, root_options);
    }
    if root_options.is_used() {
        return Err(format!("Unknown option '{}'", root_options.first_flag()));
    }
    let shared = root_options.shared(config, false);
    let Some(agent) = agent.standard() else {
        return parse_claude_command(parser, shared, config);
    };
    match agent {
        StandardAgent::Codex => parse_codex_command(parser, shared, config),
        StandardAgent::OpenCode => parse_opencode_command(parser, shared, config),
        StandardAgent::Pi => parse_pi_command(parser, shared, config),
        StandardAgent::OpenClaw => parse_openclaw_command(parser, shared, config),
        _ => parse_basic_agent_command(parser, shared, agent, config),
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
    Ok(Command::Blocks(args))
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
    args.timezone = shared.timezone.clone();
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
            ClaudeStatuslineOption::Timezone => {
                args.timezone = Some(parser.value_for("--timezone")?)
            }
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
        return Ok(Command::Session(args));
    }

    Ok(Command::All(AgentCommandArgs {
        sections,
        by_agent,
        ..agent_command_args(args.shared, AgentReportKind::Session)
    }))
}

fn parse_claude_daily_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let mut args = DailyArgs {
        shared,
        instances: false,
        project: None,
        project_aliases: None,
    };
    config.apply_daily_args(&mut args);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-i" | "--instances" => args.instances = true,
            "-p" | "--project" => args.project = Some(parser.value_for("--project")?),
            "--project-aliases" => {
                args.project_aliases = Some(parser.value_for("--project-aliases")?)
            }
            flag => return Err(format!("Unknown daily option '{flag}'")),
        }
    }
    Ok(Command::Daily(args))
}

fn parse_claude_monthly_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    _config: &dyn CliConfig,
) -> Result<Command, String> {
    while parser.peek().is_some() {
        parse_shared_arg(parser, &mut shared)?;
    }
    Ok(Command::Monthly(shared))
}

fn parse_claude_weekly_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let mut args = WeeklyArgs {
        shared,
        start_of_week: WeekDay::Sunday,
    };
    config.apply_weekly_args(&mut args);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-w" | "--start-of-week" => {
                args.start_of_week = parse_week_day(&parser.value_for("--start-of-week")?)?
            }
            flag => return Err(format!("Unknown weekly option '{flag}'")),
        }
    }
    Ok(Command::Weekly(args))
}

fn parse_claude_session_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    _config: &dyn CliConfig,
) -> Result<Command, String> {
    let mut args = SessionArgs { shared, id: None };
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-i" | "--id" => args.id = Some(parser.value_for("--id")?),
            flag => return Err(format!("Unknown session option '{flag}'")),
        }
    }
    Ok(Command::Session(args))
}

fn parse_claude_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let command = match parser.peek() {
        Some(command) if !command.starts_with('-') => {
            let command = command.to_string();
            parser.next();
            command
        }
        _ => "daily".to_string(),
    };
    match command.as_str() {
        "daily" => parse_claude_daily_command(parser, shared, config),
        "monthly" => parse_claude_monthly_command(parser, shared, config),
        "weekly" => parse_claude_weekly_command(parser, shared, config),
        "session" => parse_claude_session_command(parser, shared, config),
        command => Err(format!("Unknown claude command '{command}'")),
    }
}

fn weekly_start(config: &dyn CliConfig, kind: AgentReportKind) -> WeekDay {
    let mut weekly = WeeklyArgs {
        shared: SharedArgs::with_defaults(),
        start_of_week: WeekDay::Monday,
    };
    if kind == AgentReportKind::Weekly {
        config.apply_weekly_args(&mut weekly);
    }
    weekly.start_of_week
}

fn parse_basic_agent_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    agent: StandardAgent,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, agent)?;
    let mut args = agent_command_args(shared, kind);
    args.start_of_week = weekly_start(config, kind);
    while parser.peek().is_some() {
        if kind == AgentReportKind::Weekly
            && matches!(parser.peek_name(), Some("-w" | "--start-of-week"))
        {
            parser.next_flag()?;
            args.start_of_week = parse_week_day(&parser.value_for("--start-of-week")?)?;
        } else {
            parse_shared_arg(parser, &mut args.shared)?;
        }
    }
    Ok(Command::agent_report(agent, args))
}

fn parse_opencode_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, StandardAgent::OpenCode)?;
    let mut include_claude_agent = false;
    let mut start_of_week = weekly_start(config, kind);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--include-claude-agent" => include_claude_agent = true,
            "-w" | "--start-of-week" if kind == AgentReportKind::Weekly => {
                start_of_week = parse_week_day(&parser.value_for("--start-of-week")?)?;
            }
            flag => return Err(format!("Unknown opencode option '{flag}'")),
        }
    }
    Ok(Command::agent_report(
        StandardAgent::OpenCode,
        AgentCommandArgs {
            include_claude_agent,
            start_of_week,
            ..agent_command_args(shared, kind)
        },
    ))
}

fn parse_codex_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let agent = StandardAgent::Codex;
    let mut options = config.agent_options(agent.spec().name);
    let kind = parse_agent_report_kind(parser, agent)?;
    let mut session_id = None;
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--speed" => options.codex_speed = parse_codex_speed(&parser.value_for("--speed")?)?,
            "-i" | "--id" if kind == AgentReportKind::Session => {
                session_id = Some(parser.value_for("--id")?)
            }
            flag => return Err(format!("Unknown codex option '{flag}'")),
        }
    }
    Ok(Command::agent_report(
        StandardAgent::Codex,
        AgentCommandArgs {
            session_id,
            ..agent_command_args(shared, kind).with_options(options)
        },
    ))
}

fn parse_codex_statusline(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    mut codex_speed: CodexSpeed,
    options: &[(&str, CodexStatuslineOption)],
) -> Result<Command, String> {
    let mut session_id = None;
    let mut mcp = false;
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match command_option(parser, options, "codex")? {
            CodexStatuslineOption::Speed => {
                codex_speed = parse_codex_speed(&parser.value_for("--speed")?)?
            }
            CodexStatuslineOption::Id => session_id = Some(parser.value_for("--id")?),
            CodexStatuslineOption::Mcp => mcp = true,
        }
    }
    shared.offline |= mcp;
    Ok(Command::Agent(
        StandardAgent::Codex,
        AgentCommand::Statusline(CodexStatuslineArgs {
            shared,
            session_id,
            codex_speed,
            mcp,
        }),
    ))
}

fn parse_pi_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, StandardAgent::Pi)?;
    let mut options = config.agent_options("pi");
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--pi-path" => options.pi_path = Some(parser.value_for("--pi-path")?),
            flag => return Err(format!("Unknown pi option '{flag}'")),
        }
    }
    Ok(Command::agent_report(
        StandardAgent::Pi,
        agent_command_args(shared, kind).with_options(options),
    ))
}

fn parse_openclaw_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, StandardAgent::OpenClaw)?;
    let mut options = config.agent_options("openclaw");
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--open-claw-path" => {
                options.open_claw_path = Some(parser.value_for("--open-claw-path")?)
            }
            flag => return Err(format!("Unknown openclaw option '{flag}'")),
        }
    }
    Ok(Command::agent_report(
        StandardAgent::OpenClaw,
        agent_command_args(shared, kind).with_options(options),
    ))
}

fn parse_agent_report_kind(
    parser: &mut ArgParser,
    agent: StandardAgent,
) -> Result<AgentReportKind, String> {
    let Some(command) = parser.peek() else {
        return Ok(AgentReportKind::Daily);
    };
    let spec = agent.spec();
    if let Some(kind) =
        AgentReportKind::from_name(command).filter(|kind| spec.reports.contains(kind))
    {
        parser.next();
        return Ok(kind);
    }
    if !command.starts_with('-') {
        return Err(format!("Unknown {} command '{command}'", spec.name));
    }
    Ok(AgentReportKind::Daily)
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

fn parse_shared_arg_for_command(
    parser: &mut ArgParser,
    shared: &mut SharedArgs,
) -> Result<bool, String> {
    let Some(arg) = parser.peek() else {
        return Ok(false);
    };
    if is_shared_flag(arg) {
        parse_shared_arg(parser, shared)?;
        return Ok(true);
    }
    Ok(false)
}

enum SharedFlag {
    Since(String),
    Until(String),
    Last(NonZeroU32),
    Json,
    Mode(CostMode),
    Debug,
    DebugSamples(usize),
    Order(SortOrder),
    Breakdown,
    Offline(bool),
    Color(ColorChoice),
    Timezone(String),
    Jq(String),
    Config,
    Compact,
    SingleThread,
    NoCost,
}

impl SharedFlag {
    fn apply(self, shared: &mut SharedArgs) {
        match self {
            Self::Since(value) => shared.since = Some(value),
            Self::Until(value) => shared.until = Some(value),
            Self::Last(value) => shared.last = Some(value),
            Self::Json => shared.json = true,
            Self::Mode(value) => shared.mode = value,
            Self::Debug => shared.debug = true,
            Self::DebugSamples(value) => shared.debug_samples = value,
            Self::Order(value) => shared.order = Some(value),
            Self::Breakdown => shared.breakdown = true,
            Self::Offline(value) => shared.offline = value,
            Self::Color(value) => shared.color = value,
            Self::Timezone(value) => shared.timezone = Some(value),
            Self::Jq(value) => shared.jq = Some(value),
            Self::Config => {}
            Self::Compact => shared.compact = true,
            Self::SingleThread => shared.single_thread = true,
            Self::NoCost => shared.no_cost = true,
        }
    }
}

fn read_shared_flag(parser: &mut ArgParser) -> Result<SharedFlag, String> {
    Ok(match parser.next_flag()?.as_str() {
        "-s" | "--since" => {
            SharedFlag::Since(parse_date_bound("--since", &parser.value_for("--since")?)?)
        }
        "-u" | "--until" => {
            SharedFlag::Until(parse_date_bound("--until", &parser.value_for("--until")?)?)
        }
        "--last" => SharedFlag::Last(parse_last_periods(&parser.value_for("--last")?)?),
        "-j" | "--json" => SharedFlag::Json,
        "-m" | "--mode" => SharedFlag::Mode(parse_cost_mode(&parser.value_for("--mode")?)?),
        "-d" | "--debug" => SharedFlag::Debug,
        "--debug-samples" => SharedFlag::DebugSamples(
            parser
                .value_for("--debug-samples")?
                .parse()
                .map_err(|_| "Invalid value for --debug-samples".to_string())?,
        ),
        "-o" | "--order" => SharedFlag::Order(parse_sort_order(&parser.value_for("--order")?)?),
        "-b" | "--breakdown" => SharedFlag::Breakdown,
        "-O" | "--offline" => SharedFlag::Offline(true),
        "--no-offline" => SharedFlag::Offline(false),
        "--color" => SharedFlag::Color(ColorChoice::Always),
        "--no-color" => SharedFlag::Color(ColorChoice::Never),
        "-z" | "--timezone" => SharedFlag::Timezone(parser.value_for("--timezone")?),
        "-q" | "--jq" => SharedFlag::Jq(parser.value_for("--jq")?),
        "--config" => {
            parser.value_for("--config")?;
            SharedFlag::Config
        }
        "--compact" => SharedFlag::Compact,
        "--single-thread" => SharedFlag::SingleThread,
        "--no-cost" => SharedFlag::NoCost,
        flag => return Err(format!("Unknown option '{flag}'")),
    })
}

fn parse_shared_arg(parser: &mut ArgParser, shared: &mut SharedArgs) -> Result<(), String> {
    read_shared_flag(parser)?.apply(shared);
    Ok(())
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

fn report_flag_alias_error(args: &[String]) -> Option<String> {
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

fn invocation_error(args: &[String]) -> Option<String> {
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

fn parse_date_bound(flag: &str, value: &str) -> Result<String, String> {
    normalize_date_bound(value).ok_or_else(|| {
        format!("Invalid value for {flag} '{value}'. Expected {DATE_BOUND_FORMATS}.")
    })
}

fn parse_last_periods(value: &str) -> Result<NonZeroU32, String> {
    value.parse().map_err(|_| {
        format!(
            "Invalid value for --last '{value}'. Expected a whole number of periods, 1 or greater."
        )
    })
}

fn last_option_error(command: &Command) -> Option<String> {
    let shared = command.shared()?;
    shared.last?;
    if command.period().is_none() {
        return Some(
            "The --last option is only available for the daily, weekly, and monthly reports."
                .to_string(),
        );
    }
    if shared.since.is_some() || shared.until.is_some() {
        return Some("The --last option cannot be combined with --since or --until.".to_string());
    }
    if matches!(command, Command::All(args) if args.sections.is_some()) {
        return Some("The --last option cannot be used with --sections.".to_string());
    }
    None
}

fn validate_date_window(
    command: &mut Command,
    parser: &ArgParser,
    config: &dyn CliConfig,
) -> Result<(), String> {
    let Some((since, until)) = command
        .shared()
        .and_then(|shared| shared.since.as_deref().zip(shared.until.as_deref()))
    else {
        return Ok(());
    };
    if since <= until {
        return Ok(());
    }
    let message = format!("The --since date '{since}' is later than the --until date '{until}'.");
    if let Command::Agent(_, AgentCommand::Statusline(args)) = command {
        let since_flag = parser.saw_flag(&["-s", "--since"]);
        let until_flag = parser.saw_flag(&["-u", "--until"]);
        if !(since_flag && until_flag)
            && (config.recover_statusline_window(
                &mut args.shared,
                since_flag,
                until_flag,
                &message,
            ) || args.mcp
                || args.session_id.is_some())
        {
            return Ok(());
        }
    }
    Err(message)
}

fn validate_timezone(
    command: &mut Command,
    parser: &ArgParser,
    config: &dyn CliConfig,
) -> Result<(), String> {
    let statusline = matches!(
        command,
        Command::Statusline(_) | Command::Agent(_, AgentCommand::Statusline(_))
    );
    let timezone = match command {
        Command::Statusline(args) => &mut args.timezone,
        command => {
            let Some(shared) = command.shared_mut() else {
                return Ok(());
            };
            &mut shared.timezone
        }
    };
    let Some(value) = timezone.as_deref() else {
        return Ok(());
    };
    if valid_timezone(value) {
        return Ok(());
    }
    let message = format!(
        "Invalid value for --timezone '{value}'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
    );
    if statusline
        && !parser.saw_flag(&["-z", "--timezone"])
        && config.recover_statusline_timezone(timezone, valid_timezone)
    {
        return Ok(());
    }
    Err(message)
}

fn valid_timezone(value: &str) -> bool {
    matches!(value, "local" | "UTC") || jiff::tz::TimeZone::get(value).is_ok()
}

fn parse_cost_mode(value: &str) -> Result<CostMode, String> {
    match value {
        "auto" => Ok(CostMode::Auto),
        "calculate" => Ok(CostMode::Calculate),
        "display" => Ok(CostMode::Display),
        _ => Err(format!("Invalid cost mode '{value}'")),
    }
}

fn parse_sort_order(value: &str) -> Result<SortOrder, String> {
    match value {
        "asc" => Ok(SortOrder::Asc),
        "desc" => Ok(SortOrder::Desc),
        _ => Err(format!("Invalid sort order '{value}'")),
    }
}

fn parse_report_sections(value: &str) -> Result<Vec<AgentReportKind>, String> {
    let mut sections = Vec::new();
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some(kind) = AgentReportKind::from_name(token) else {
            return Err(format!(
                "Invalid --sections value '{token}'. Expected one or more of: daily, weekly, monthly, session."
            ));
        };
        if !sections.contains(&kind) {
            sections.push(kind);
        }
    }
    if sections.is_empty() {
        return Err(format!(
            "Invalid --sections value '{value}'. Expected one or more of: daily, weekly, monthly, session."
        ));
    }
    Ok(sections)
}

fn parse_week_day(value: &str) -> Result<WeekDay, String> {
    match value {
        "sunday" => Ok(WeekDay::Sunday),
        "monday" => Ok(WeekDay::Monday),
        "tuesday" => Ok(WeekDay::Tuesday),
        "wednesday" => Ok(WeekDay::Wednesday),
        "thursday" => Ok(WeekDay::Thursday),
        "friday" => Ok(WeekDay::Friday),
        "saturday" => Ok(WeekDay::Saturday),
        _ => Err(format!("Invalid week day '{value}'")),
    }
}

fn parse_codex_speed(value: &str) -> Result<CodexSpeed, String> {
    match value {
        "auto" => Ok(CodexSpeed::Auto),
        "standard" => Ok(CodexSpeed::Standard),
        "fast" => Ok(CodexSpeed::Fast),
        "flex" => Ok(CodexSpeed::Flex),
        _ => Err(format!("Invalid speed option '{value}'")),
    }
}

fn parse_visual_burn_rate(value: &str) -> Result<VisualBurnRate, String> {
    match value {
        "off" => Ok(VisualBurnRate::Off),
        "emoji" => Ok(VisualBurnRate::Emoji),
        "text" => Ok(VisualBurnRate::Text),
        "emoji-text" => Ok(VisualBurnRate::EmojiText),
        _ => Err(format!("Invalid visual burn rate '{value}'")),
    }
}

fn parse_cost_source(value: &str) -> Result<CostSource, String> {
    match value {
        "auto" => Ok(CostSource::Auto),
        "ccusage" => Ok(CostSource::Ccusage),
        "cc" => Ok(CostSource::Cc),
        "both" => Ok(CostSource::Both),
        _ => Err(format!("Invalid cost source '{value}'")),
    }
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
