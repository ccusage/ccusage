use std::{collections::BTreeMap, ffi::OsString, num::NonZeroU32};

use crate::arg_parser::ArgParser;
use crate::help::{print_help_and_exit, print_version_and_exit};
use ccusage_cli::{
    AgentCommandArgs, AgentId, AgentReportKind, BlocksArgs, CliConfig, CodexSpeed, ColorChoice,
    Command, CostMode, CostSource, DATE_BOUND_FORMATS, DEFAULT_SESSION_DURATION_HOURS, DailyArgs,
    SessionArgs, SharedArgs, SortOrder, StandardAgent, StatuslineArgs, VisualBurnRate, WeekDay,
    WeeklyArgs, normalize_date_bound,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlArg {
    Help,
    Version,
}

#[derive(Default)]
struct RootOptions {
    offline_override: Option<bool>,
    sections: Option<Vec<AgentReportKind>>,
    by_agent: bool,
    first_flag: Option<&'static str>,
    statusline_unsupported: Option<String>,
}

impl RootOptions {
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
    default_session_duration_hours: f64,
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
    if let Some(message) = agent_filter_option_error(&parser.args) {
        return Err(message);
    }
    if let Some(message) = unsupported_agent_report_error(&parser.args) {
        return Err(message);
    }
    if let Some(message) = config.config_error() {
        return Err(message.to_string());
    }
    let mut shared = SharedArgs::with_defaults();
    config.apply_shared(&mut shared);
    let mut root_options = RootOptions::default();
    while let Some(arg) = parser.peek() {
        if is_command(arg) {
            break;
        }
        if !arg.starts_with('-') {
            return Err(format!("Unknown command '{arg}'"));
        }
        if parse_root_all_arg(&mut parser, &mut root_options)? {
            continue;
        }
        let offline_override = match parser.peek_name() {
            Some("-O" | "--offline") => Some(true),
            Some("--no-offline") => Some(false),
            _ => None,
        };
        if !matches!(
            parser.peek_name(),
            Some(
                "-O" | "--offline" | "--no-offline" | "-z" | "--timezone" | "--config" | "--debug"
            )
        ) {
            root_options
                .statusline_unsupported
                .get_or_insert_with(|| parser.peek_name().unwrap_or_default().to_string());
        }
        parse_shared_arg(&mut parser, &mut shared)?;
        if offline_override.is_some() {
            root_options.offline_override = offline_override;
        }
    }

    let command = match parser.next() {
        None => Command::All(root_options.into_agent_args(shared.clone(), AgentReportKind::Daily)),
        Some(command) => parse_command(
            &command,
            &mut parser,
            shared.clone(),
            config,
            default_session_duration_hours,
            root_options,
        )?,
    };
    parser.finish()?;
    if let Some(message) = last_option_error(&command, &shared) {
        return Err(message);
    }
    if let Some(message) = date_window_error(&command) {
        return Err(message);
    }
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
    match parse_from_with_config(args, config, DEFAULT_SESSION_DURATION_HOURS, version)? {
        Command::All(args) | Command::Agent(_, args) => Ok(AgentCommandArgs {
            start_of_week,
            ..args
        }),
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
    shared: SharedArgs,
    config: &dyn CliConfig,
    default_session_duration_hours: f64,
    root_options: RootOptions,
) -> Result<Command, String> {
    if root_options.is_used() && !accepts_root_options(command) {
        return Err(format!("Unknown option '{}'", root_options.first_flag()));
    }
    if (command == "statusline" || (command == "claude" && parser.peek() == Some("statusline")))
        && let Some(flag) = &root_options.statusline_unsupported
    {
        return Err(format!("Unknown statusline option '{flag}'"));
    }
    let offline_override = root_options.offline_override;
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
        "blocks" => parse_blocks_command(parser, shared, config, default_session_duration_hours),
        "statusline" => parse_statusline_command(parser, &shared, config, offline_override),
        _ => match AgentId::from_name(command) {
            Some(agent) => parse_agent_command(
                agent,
                parser,
                shared,
                config,
                default_session_duration_hours,
                offline_override,
            ),
            None => Err(format!("Unknown command '{command}'")),
        },
    }
}

fn parse_agent_command(
    agent: AgentId,
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
    default_session_duration_hours: f64,
    offline_override: Option<bool>,
) -> Result<Command, String> {
    let Some(agent) = agent.standard() else {
        return parse_claude_command(
            parser,
            shared,
            config,
            default_session_duration_hours,
            offline_override,
        );
    };
    match agent {
        StandardAgent::Codex => parse_codex_command(parser, shared, config),
        StandardAgent::OpenCode => parse_opencode_command(parser, shared, config),
        StandardAgent::Pi => parse_pi_command(parser, shared, config),
        StandardAgent::OpenClaw => parse_openclaw_command(parser, shared, config),
        _ => parse_basic_agent_command(parser, shared, agent, config),
    }
}

fn parse_blocks_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
    default_session_duration_hours: f64,
) -> Result<Command, String> {
    let mut args = BlocksArgs {
        shared,
        active: false,
        recent: false,
        token_limit: None,
        session_length: default_session_duration_hours,
    };
    config.apply_blocks_args(&mut args);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut args.shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-a" | "--active" => args.active = true,
            "-r" | "--recent" => args.recent = true,
            "-t" | "--token-limit" => args.token_limit = Some(parser.value_for("--token-limit")?),
            "-n" | "--session-length" => {
                args.session_length = parser
                    .value_for("--session-length")?
                    .parse()
                    .map_err(|_| "Invalid value for --session-length".to_string())?
            }
            flag => return Err(format!("Unknown blocks option '{flag}'")),
        }
    }
    Ok(Command::Blocks(args))
}

fn parse_statusline_command(
    parser: &mut ArgParser,
    shared: &SharedArgs,
    config: &dyn CliConfig,
    offline_override: Option<bool>,
) -> Result<Command, String> {
    let mut args = StatuslineArgs::default();
    config.apply_statusline_args(&mut args);
    if let Some(offline) = offline_override {
        args.offline = offline;
    }
    args.timezone = shared.timezone.clone();
    args.debug = shared.debug;
    while parser.peek().is_some() {
        match parser.next_flag()?.as_str() {
            "-O" | "--offline" => args.offline = true,
            "--no-offline" => args.offline = false,
            "-B" | "--visual-burn-rate" => {
                args.visual_burn_rate =
                    parse_visual_burn_rate(&parser.value_for("--visual-burn-rate")?)?
            }
            "--cost-source" => {
                args.cost_source = parse_cost_source(&parser.value_for("--cost-source")?)?
            }
            "--cache" => args.cache = true,
            "--no-cache" => args.cache = false,
            "--refresh-interval" => {
                args.refresh_interval = parser
                    .value_for("--refresh-interval")?
                    .parse()
                    .map_err(|_| "Invalid value for --refresh-interval".to_string())?
            }
            "--context-low-threshold" => {
                args.context_low_threshold = parser
                    .value_for("--context-low-threshold")?
                    .parse()
                    .map_err(|_| "Invalid value for --context-low-threshold".to_string())?
            }
            "--context-medium-threshold" => {
                args.context_medium_threshold = parser
                    .value_for("--context-medium-threshold")?
                    .parse()
                    .map_err(|_| "Invalid value for --context-medium-threshold".to_string())?
            }
            "-z" | "--timezone" => args.timezone = Some(parser.value_for("--timezone")?),
            "--config" => {
                parser.value_for("--config")?;
            }
            "--debug" => args.debug = true,
            flag => return Err(format!("Unknown statusline option '{flag}'")),
        }
    }
    Ok(Command::Statusline(args))
}

fn accepts_root_options(command: &str) -> bool {
    matches!(command, "daily" | "monthly" | "weekly" | "session")
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
    default_session_duration_hours: f64,
    offline_override: Option<bool>,
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
        "blocks" => parse_blocks_command(parser, shared, config, default_session_duration_hours),
        "statusline" => parse_statusline_command(parser, &shared, config, offline_override),
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
    Ok(Command::Agent(agent, args))
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
    Ok(Command::Agent(
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
    let kind = parse_agent_report_kind(parser, StandardAgent::Codex)?;
    let mut codex_speed = config.agent_options("codex").codex_speed;
    let mut session_id = None;
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--speed" => codex_speed = parse_codex_speed(&parser.value_for("--speed")?)?,
            "-i" | "--id" if kind == AgentReportKind::Session => {
                session_id = Some(parser.value_for("--id")?)
            }
            flag => return Err(format!("Unknown codex option '{flag}'")),
        }
    }
    Ok(Command::Agent(
        StandardAgent::Codex,
        AgentCommandArgs {
            session_id,
            codex_speed,
            ..agent_command_args(shared, kind)
        },
    ))
}

fn parse_pi_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, StandardAgent::Pi)?;
    let mut pi_path = config.agent_options("pi").pi_path;
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--pi-path" => pi_path = Some(parser.value_for("--pi-path")?),
            flag => return Err(format!("Unknown pi option '{flag}'")),
        }
    }
    Ok(Command::Agent(
        StandardAgent::Pi,
        AgentCommandArgs {
            pi_path,
            ..agent_command_args(shared, kind)
        },
    ))
}

fn parse_openclaw_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, StandardAgent::OpenClaw)?;
    let mut open_claw_path = config.agent_options("openclaw").open_claw_path;
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--open-claw-path" => open_claw_path = Some(parser.value_for("--open-claw-path")?),
            flag => return Err(format!("Unknown openclaw option '{flag}'")),
        }
    }
    Ok(Command::Agent(
        StandardAgent::OpenClaw,
        AgentCommandArgs {
            open_claw_path,
            ..agent_command_args(shared, kind)
        },
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
        shared,
        kind,
        session_id: None,
        sections: None,
        by_agent: false,
        pi_path: None,
        open_claw_path: None,
        codex_speed: CodexSpeed::Auto,
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

fn parse_shared_arg(parser: &mut ArgParser, shared: &mut SharedArgs) -> Result<(), String> {
    match parser.next_flag()?.as_str() {
        "-s" | "--since" => {
            shared.since = Some(parse_date_bound("--since", &parser.value_for("--since")?)?)
        }
        "-u" | "--until" => {
            shared.until = Some(parse_date_bound("--until", &parser.value_for("--until")?)?)
        }
        "--last" => shared.last = Some(parse_last_periods(&parser.value_for("--last")?)?),
        "-j" | "--json" => shared.json = true,
        "-m" | "--mode" => shared.mode = parse_cost_mode(&parser.value_for("--mode")?)?,
        "-d" | "--debug" => shared.debug = true,
        "--debug-samples" => {
            shared.debug_samples = parser
                .value_for("--debug-samples")?
                .parse()
                .map_err(|_| "Invalid value for --debug-samples".to_string())?
        }
        "-o" | "--order" => {
            shared.order = parse_sort_order(&parser.value_for("--order")?)?;
            shared.order_explicit = true;
        }
        "-b" | "--breakdown" => shared.breakdown = true,
        "-O" | "--offline" => shared.offline = true,
        "--no-offline" => shared.offline = false,
        "--color" => shared.color = ColorChoice::Always,
        "--no-color" => shared.color = ColorChoice::Never,
        "-z" | "--timezone" => shared.timezone = Some(parser.value_for("--timezone")?),
        "-q" | "--jq" => shared.jq = Some(parser.value_for("--jq")?),
        "--config" => {
            parser.value_for("--config")?;
        }
        "--compact" => shared.compact = true,
        "--single-thread" => shared.single_thread = true,
        "--no-cost" => shared.no_cost = true,
        flag => return Err(format!("Unknown option '{flag}'")),
    }
    Ok(())
}

const TOP_LEVEL_COMMANDS: &[&str] = &[
    "daily",
    "monthly",
    "weekly",
    "session",
    "blocks",
    "statusline",
];

fn is_command(arg: &str) -> bool {
    TOP_LEVEL_COMMANDS.contains(&arg) || AgentId::from_name(arg).is_some()
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

fn agent_filter_option_error(args: &[String]) -> Option<String> {
    let allows_short_active = blocks_command_tokens(args);
    let flag = args.iter().find_map(|arg| {
        if arg == "--agent" || arg.starts_with("--agent=") {
            return Some("--agent");
        }
        if (arg == "-a" && !allows_short_active) || arg.starts_with("-a=") {
            return Some("-a");
        }
        None
    })?;
    Some(format!(
        "Agent filters like {flag} are not supported. Use \"ccusage <agent> <report>\", for example \"ccusage codex daily\"."
    ))
}

fn blocks_command_tokens(args: &[String]) -> bool {
    let tokens = command_tokens(args);
    matches!(
        tokens.as_slice(),
        [command, ..] if command == "blocks"
    ) || matches!(
        tokens.as_slice(),
        [agent, command, ..] if agent == "claude" && command == "blocks"
    )
}

fn unsupported_agent_report_error(args: &[String]) -> Option<String> {
    let tokens = command_tokens(args);
    let [agent, report, ..] = tokens.as_slice() else {
        return None;
    };
    let spec = AgentId::from_name(agent)?.spec();
    if spec.accepts(report) {
        return None;
    }

    let display = spec.label;
    let claude = AgentId::Claude.spec();
    let message = if claude.extra_commands.contains(&report.as_str()) {
        format!(
            "The \"{report}\" report is only available for {} usage.\nUse \"ccusage {agent} daily\" for {display} usage reports.",
            claude.label
        )
    } else {
        format!(
            "The \"{report}\" report is not available for {display} usage.\nUse \"ccusage {agent} daily\" for {display} usage reports."
        )
    };
    Some(message)
}

pub(crate) fn command_tokens(args: &[String]) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg.starts_with('-') {
            if option_takes_value(arg) && !arg.contains('=') {
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        tokens.push(arg.clone());
        index += 1;
    }
    tokens
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-s" | "--since"
            | "-u"
            | "--until"
            | "--last"
            | "-m"
            | "--mode"
            | "--debug-samples"
            | "-o"
            | "--order"
            | "-z"
            | "--timezone"
            | "-q"
            | "--jq"
            | "--config"
            | "-p"
            | "--project"
            | "--project-aliases"
            | "-w"
            | "--start-of-week"
            | "-i"
            | "--id"
            | "-t"
            | "--token-limit"
            | "-n"
            | "--session-length"
            | "-B"
            | "--visual-burn-rate"
            | "--cost-source"
            | "--refresh-interval"
            | "--context-low-threshold"
            | "--context-medium-threshold"
            | "--speed"
            | "--pi-path"
            | "--open-claw-path"
            | "--sections"
    )
}

fn is_shared_flag(arg: &str) -> bool {
    matches!(
        arg.split_once('=').map_or(arg, |(name, _)| name),
        "-s" | "--since"
            | "-u"
            | "--until"
            | "--last"
            | "-j"
            | "--json"
            | "-m"
            | "--mode"
            | "-d"
            | "--debug"
            | "--debug-samples"
            | "-o"
            | "--order"
            | "-b"
            | "--breakdown"
            | "-O"
            | "--offline"
            | "--no-offline"
            | "--color"
            | "--no-color"
            | "-z"
            | "--timezone"
            | "-q"
            | "--jq"
            | "--config"
            | "--compact"
            | "--single-thread"
            | "--no-cost"
    )
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

fn last_option_error(command: &Command, root_shared: &SharedArgs) -> Option<String> {
    let shared = command.shared().unwrap_or(root_shared);
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

/// Both bounds are already normalized to `YYYYMMDD`, so a plain string comparison
/// orders them. A reversed window would otherwise load everything and print an
/// empty report that looks like missing data.
fn date_window_error(command: &Command) -> Option<String> {
    let shared = command.shared()?;
    let since = shared.since.as_deref()?;
    let until = shared.until.as_deref()?;
    (since > until)
        .then(|| format!("The --since date '{since}' is later than the --until date '{until}'."))
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
