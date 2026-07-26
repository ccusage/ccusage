use std::{env, ffi::OsString, path::PathBuf, process};

use crate::arg_parser::ArgParser;
use crate::help::{print_help_and_exit, print_version_and_exit};
use crate::types::{OPENCODE_AGENT_REPORTS, STANDARD_AGENT_REPORTS};
use crate::{
    AgentCommandArgs, AgentReportKind, BlocksArgs, Cli, CliConfig, CodexSpeed, Command, CostMode,
    CostSource, DailyArgs, NoConfig, SessionArgs, SharedArgs, SortOrder, StatuslineArgs,
    VisualBurnRate, WeekDay, WeeklyArgs, normalize_date_bound,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlArg {
    Help,
    Version,
}

#[derive(Default)]
struct RootAllOptions {
    sections: Option<Vec<AgentReportKind>>,
    by_agent: bool,
    first_flag: Option<&'static str>,
}

impl RootAllOptions {
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
            shared,
            kind,
            sections: self.sections,
            by_agent: self.by_agent,
            pi_path: None,
            open_claw_path: None,
            codex_speed: CodexSpeed::Auto,
        }
    }
}

impl Cli {
    pub fn parse() -> Self {
        Self::parse_from(env::args_os()).unwrap_or_else(|message| {
            eprintln!("{message}");
            eprintln!("Run 'ccusage --help' for usage.");
            process::exit(2);
        })
    }

    pub fn parse_from<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = OsString>,
    {
        Self::parse_from_with_config(args, &NoConfig, 5.0, env!("CARGO_PKG_VERSION"))
    }

    pub fn parse_from_with_config<I>(
        args: I,
        config: &dyn CliConfig,
        default_session_duration_hours: f64,
        version: &'static str,
    ) -> Result<Self, String>
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
        let mut root_all_options = RootAllOptions::default();
        while let Some(arg) = parser.peek() {
            if is_command(arg) {
                break;
            }
            if !arg.starts_with('-') {
                return Err(format!("Unknown command '{arg}'"));
            }
            if parse_root_all_arg(&mut parser, &mut root_all_options)? {
                continue;
            }
            parse_shared_arg(&mut parser, &mut shared)?;
        }

        let command = match parser.next() {
            None if root_all_options.is_used() => Some(Command::All(
                root_all_options.into_agent_args(shared.clone(), AgentReportKind::Daily),
            )),
            None => None,
            Some(command) => Some(parse_command(
                &command,
                &mut parser,
                shared.clone(),
                config,
                default_session_duration_hours,
                root_all_options,
            )?),
        };
        if let Some(extra) = parser.next() {
            return Err(format!("Unexpected argument '{extra}'"));
        }
        Ok(Self { command, shared })
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
    root_all_options: RootAllOptions,
) -> Result<Command, String> {
    if root_all_options.is_used() && !accepts_root_all_options(command) {
        return Err(format!(
            "Unknown option '{}'",
            root_all_options.first_flag()
        ));
    }
    match command {
        "daily" => parse_all_command(
            parser,
            shared,
            AgentReportKind::Daily,
            config,
            root_all_options,
        ),
        "monthly" => parse_all_command(
            parser,
            shared,
            AgentReportKind::Monthly,
            config,
            root_all_options,
        ),
        "weekly" => parse_all_command(
            parser,
            shared,
            AgentReportKind::Weekly,
            config,
            root_all_options,
        ),
        "session" => parse_top_level_session_command(parser, shared, config, root_all_options),
        "blocks" => {
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
                    "-t" | "--token-limit" => {
                        args.token_limit = Some(parser.value_for("--token-limit")?)
                    }
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
        "statusline" => {
            let mut args = StatuslineArgs::default();
            config.apply_statusline_args(&mut args);
            while parser.peek().is_some() {
                match parser.next_flag()?.as_str() {
                    "-O" | "--offline" => args.offline = true,
                    "--no-offline" => args.no_offline = true,
                    "-B" | "--visual-burn-rate" => {
                        args.visual_burn_rate =
                            parse_visual_burn_rate(&parser.value_for("--visual-burn-rate")?)?
                    }
                    "--cost-source" => {
                        args.cost_source = parse_cost_source(&parser.value_for("--cost-source")?)?
                    }
                    "--cache" => args.cache = true,
                    "--no-cache" => args.no_cache = true,
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
                            .map_err(|_| {
                                "Invalid value for --context-medium-threshold".to_string()
                            })?
                    }
                    "-z" | "--timezone" => args.timezone = Some(parser.value_for("--timezone")?),
                    "--config" => args.config = Some(PathBuf::from(parser.value_for("--config")?)),
                    "--debug" => args.debug = true,
                    flag => return Err(format!("Unknown statusline option '{flag}'")),
                }
            }
            Ok(Command::Statusline(args))
        }
        "claude" => parse_claude_command(parser, shared, config, default_session_duration_hours),
        "codex" => parse_codex_command(parser, shared, config),
        "opencode" => parse_basic_agent_command(
            parser,
            shared,
            "opencode",
            OPENCODE_AGENT_REPORTS,
            Command::OpenCode,
        ),
        "amp" => {
            parse_basic_agent_command(parser, shared, "amp", STANDARD_AGENT_REPORTS, Command::Amp)
        }
        "droid" => parse_basic_agent_command(
            parser,
            shared,
            "droid",
            STANDARD_AGENT_REPORTS,
            Command::Droid,
        ),
        "codebuff" => parse_basic_agent_command(
            parser,
            shared,
            "codebuff",
            STANDARD_AGENT_REPORTS,
            Command::Codebuff,
        ),
        "hermes" => parse_basic_agent_command(
            parser,
            shared,
            "hermes",
            STANDARD_AGENT_REPORTS,
            Command::Hermes,
        ),
        "pi" => parse_pi_command(parser, shared, config),
        "goose" => parse_basic_agent_command(
            parser,
            shared,
            "goose",
            STANDARD_AGENT_REPORTS,
            Command::Goose,
        ),
        "kilo" => parse_basic_agent_command(
            parser,
            shared,
            "kilo",
            STANDARD_AGENT_REPORTS,
            Command::Kilo,
        ),
        "copilot" => parse_basic_agent_command(
            parser,
            shared,
            "copilot",
            STANDARD_AGENT_REPORTS,
            Command::Copilot,
        ),
        "gemini" => parse_basic_agent_command(
            parser,
            shared,
            "gemini",
            STANDARD_AGENT_REPORTS,
            Command::Gemini,
        ),
        "kimi" => parse_basic_agent_command(
            parser,
            shared,
            "kimi",
            STANDARD_AGENT_REPORTS,
            Command::Kimi,
        ),
        "qwen" => parse_basic_agent_command(
            parser,
            shared,
            "qwen",
            STANDARD_AGENT_REPORTS,
            Command::Qwen,
        ),
        "openclaw" => parse_openclaw_command(parser, shared, config),
        _ => Err(format!("Unknown command '{command}'")),
    }
}

fn accepts_root_all_options(command: &str) -> bool {
    matches!(command, "daily" | "monthly" | "weekly" | "session")
}

fn parse_root_all_arg(
    parser: &mut ArgParser,
    options: &mut RootAllOptions,
) -> Result<bool, String> {
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
    if matches!(parser.peek(), Some("--all")) {
        parser.next();
        return Ok(Some("--all"));
    }
    if matches!(parser.peek_name(), Some("--sections")) {
        parser.next_flag()?;
        *sections = Some(parse_report_sections(&parser.value_for("--sections")?)?);
        return Ok(Some("--sections"));
    }
    if matches!(parser.peek(), Some("--by-agent")) {
        parser.next();
        *by_agent = true;
        return Ok(Some("--by-agent"));
    }
    Ok(None)
}

fn parse_all_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    kind: AgentReportKind,
    _config: &dyn CliConfig,
    initial_options: RootAllOptions,
) -> Result<Command, String> {
    let mut sections = initial_options.sections;
    let mut by_agent = initial_options.by_agent;
    while parser.peek().is_some() {
        if parse_unified_report_arg(parser, &mut sections, &mut by_agent)?.is_some() {
            continue;
        }
        parse_shared_arg(parser, &mut shared)?;
    }
    Ok(Command::All(AgentCommandArgs {
        shared,
        kind,
        sections,
        by_agent,
        pi_path: None,
        open_claw_path: None,
        codex_speed: CodexSpeed::Auto,
    }))
}

fn parse_top_level_session_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    _config: &dyn CliConfig,
    initial_options: RootAllOptions,
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
        shared: args.shared,
        kind: AgentReportKind::Session,
        sections,
        by_agent,
        pi_path: None,
        open_claw_path: None,
        codex_speed: CodexSpeed::Auto,
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
) -> Result<Command, String> {
    let command = match parser.peek() {
        Some(command @ ("daily" | "monthly" | "weekly" | "session" | "blocks" | "statusline")) => {
            let command = command.to_string();
            parser.next();
            command
        }
        Some(command) if !command.starts_with('-') => {
            return Err(format!("Unknown claude command '{command}'"));
        }
        _ => "daily".to_string(),
    };
    match command.as_str() {
        "daily" => parse_claude_daily_command(parser, shared, config),
        "monthly" => parse_claude_monthly_command(parser, shared, config),
        "weekly" => parse_claude_weekly_command(parser, shared, config),
        "session" => parse_claude_session_command(parser, shared, config),
        "blocks" | "statusline" => parse_command(
            &command,
            parser,
            shared,
            config,
            default_session_duration_hours,
            RootAllOptions::default(),
        ),
        _ => unreachable!("claude command is prevalidated"),
    }
}

fn parse_basic_agent_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    agent: &str,
    reports: &[(&str, AgentReportKind)],
    command: fn(AgentCommandArgs) -> Command,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, agent, reports)?;
    while parser.peek().is_some() {
        parse_shared_arg(parser, &mut shared)?;
    }
    Ok(command(agent_command_args(shared, kind)))
}

fn parse_codex_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, "codex", STANDARD_AGENT_REPORTS)?;
    let mut codex_speed = CodexSpeed::Auto;
    config.apply_agent_args(&mut codex_speed, None, None);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--speed" => codex_speed = parse_codex_speed(&parser.value_for("--speed")?)?,
            flag => return Err(format!("Unknown codex option '{flag}'")),
        }
    }
    Ok(Command::Codex(AgentCommandArgs {
        shared,
        kind,
        sections: None,
        by_agent: false,
        pi_path: None,
        open_claw_path: None,
        codex_speed,
    }))
}

fn parse_pi_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, "pi", STANDARD_AGENT_REPORTS)?;
    let mut pi_path = None;
    let mut codex_speed = CodexSpeed::Auto;
    config.apply_agent_args(&mut codex_speed, Some(&mut pi_path), None);
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--pi-path" => pi_path = Some(parser.value_for("--pi-path")?),
            flag => return Err(format!("Unknown pi option '{flag}'")),
        }
    }
    Ok(Command::Pi(AgentCommandArgs {
        shared,
        kind,
        sections: None,
        by_agent: false,
        pi_path,
        open_claw_path: None,
        codex_speed,
    }))
}

fn parse_openclaw_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let kind = parse_agent_report_kind(parser, "openclaw", STANDARD_AGENT_REPORTS)?;
    let mut open_claw_path = None;
    let mut codex_speed = CodexSpeed::Auto;
    config.apply_agent_args(&mut codex_speed, None, Some(&mut open_claw_path));
    while parser.peek().is_some() {
        if parse_shared_arg_for_command(parser, &mut shared)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "--open-claw-path" => open_claw_path = Some(parser.value_for("--open-claw-path")?),
            flag => return Err(format!("Unknown openclaw option '{flag}'")),
        }
    }
    Ok(Command::OpenClaw(AgentCommandArgs {
        shared,
        kind,
        sections: None,
        by_agent: false,
        pi_path: None,
        open_claw_path,
        codex_speed,
    }))
}

fn parse_agent_report_kind(
    parser: &mut ArgParser,
    agent: &str,
    reports: &[(&str, AgentReportKind)],
) -> Result<AgentReportKind, String> {
    let Some(command) = parser.peek() else {
        return Ok(AgentReportKind::Daily);
    };
    if let Some((_, kind)) = reports.iter().find(|(report, _)| *report == command) {
        parser.next();
        return Ok(*kind);
    }
    if !command.starts_with('-') {
        return Err(format!("Unknown {agent} command '{command}'"));
    }
    Ok(AgentReportKind::Daily)
}

fn agent_command_args(shared: SharedArgs, kind: AgentReportKind) -> AgentCommandArgs {
    AgentCommandArgs {
        shared,
        kind,
        sections: None,
        by_agent: false,
        pi_path: None,
        open_claw_path: None,
        codex_speed: CodexSpeed::Auto,
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
            shared.since = Some(normalize_date_bound(&parser.value_for("--since")?))
        }
        "-u" | "--until" => {
            shared.until = Some(normalize_date_bound(&parser.value_for("--until")?))
        }
        "-j" | "--json" => shared.json = true,
        "-m" | "--mode" => shared.mode = parse_cost_mode(&parser.value_for("--mode")?)?,
        "-d" | "--debug" => shared.debug = true,
        "--debug-samples" => {
            shared.debug_samples = parser
                .value_for("--debug-samples")?
                .parse()
                .map_err(|_| "Invalid value for --debug-samples".to_string())?
        }
        "-o" | "--order" => shared.order = parse_sort_order(&parser.value_for("--order")?)?,
        "-b" | "--breakdown" => shared.breakdown = true,
        "-O" | "--offline" => shared.offline = true,
        "--no-offline" => shared.no_offline = true,
        "--color" => shared.color = true,
        "--no-color" => shared.no_color = true,
        "-z" | "--timezone" => shared.timezone = Some(parser.value_for("--timezone")?),
        "-q" | "--jq" => shared.jq = Some(parser.value_for("--jq")?),
        "--config" => shared.config = Some(PathBuf::from(parser.value_for("--config")?)),
        "--compact" => shared.compact = true,
        "--single-thread" => shared.single_thread = true,
        "--no-cost" => shared.no_cost = true,
        flag => return Err(format!("Unknown option '{flag}'")),
    }
    Ok(())
}

fn is_command(arg: &str) -> bool {
    matches!(
        arg,
        "daily"
            | "monthly"
            | "weekly"
            | "session"
            | "blocks"
            | "statusline"
            | "claude"
            | "codex"
            | "opencode"
            | "amp"
            | "droid"
            | "codebuff"
            | "hermes"
            | "pi"
            | "goose"
            | "openclaw"
            | "kilo"
            | "copilot"
            | "gemini"
            | "kimi"
            | "qwen"
    )
}

fn normalize_legacy_agent_command_args(args: &mut Vec<String>) {
    let Some(command) = args.first() else {
        return;
    };
    let Some((agent, report)) = command.split_once(':') else {
        return;
    };
    if !legacy_agent_report_supported(agent, report) {
        return;
    }
    args.splice(0..1, [agent.to_string(), report.to_string()]);
}

fn legacy_agent_report_supported(agent: &str, report: &str) -> bool {
    agent_report_supported(agent, report)
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
    if !is_agent_command(agent) || agent_report_supported(agent, report) {
        return None;
    }

    let display = agent_display_name(agent);
    let message = if matches!(report.as_str(), "blocks" | "statusline") {
        format!(
            "The \"{report}\" report is only available for Claude Code usage.\nUse \"ccusage {agent} daily\" for {display} usage reports."
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

fn is_agent_command(command: &str) -> bool {
    matches!(
        command,
        "claude"
            | "codex"
            | "opencode"
            | "amp"
            | "droid"
            | "codebuff"
            | "hermes"
            | "pi"
            | "goose"
            | "kilo"
            | "copilot"
            | "gemini"
            | "kimi"
            | "qwen"
            | "openclaw"
    )
}

fn agent_report_supported(agent: &str, report: &str) -> bool {
    match agent {
        "claude" => matches!(
            report,
            "daily" | "weekly" | "monthly" | "session" | "blocks" | "statusline"
        ),
        "codex" => matches!(report, "daily" | "monthly" | "session"),
        "opencode" => matches!(report, "daily" | "weekly" | "monthly" | "session"),
        "amp" | "droid" | "codebuff" | "hermes" | "pi" | "goose" | "kilo" | "copilot"
        | "gemini" | "kimi" | "qwen" | "openclaw" => {
            matches!(report, "daily" | "monthly" | "session")
        }
        _ => false,
    }
}

fn agent_display_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "amp" => "Amp",
        "droid" => "Droid",
        "codebuff" => "Codebuff",
        "hermes" => "Hermes",
        "pi" => "pi-agent",
        "goose" => "Goose",
        "kilo" => "Kilo",
        "copilot" => "GitHub Copilot CLI",
        "gemini" => "Gemini CLI",
        "kimi" => "Kimi",
        "qwen" => "Qwen",
        "openclaw" => "OpenClaw",
        _ => unreachable!("agent is prevalidated"),
    }
}

fn is_shared_flag(arg: &str) -> bool {
    matches!(
        arg.split_once('=').map_or(arg, |(name, _)| name),
        "-s" | "--since"
            | "-u"
            | "--until"
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
        let kind = match token {
            "daily" => AgentReportKind::Daily,
            "weekly" => AgentReportKind::Weekly,
            "monthly" => AgentReportKind::Monthly,
            "session" => AgentReportKind::Session,
            _ => {
                return Err(format!(
                    "Invalid --sections value '{token}'. Expected one or more of: daily, weekly, monthly, session."
                ));
            }
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
