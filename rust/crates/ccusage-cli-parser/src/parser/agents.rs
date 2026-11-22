use ccusage_cli::{
    AgentCommand, AgentCommandArgs, AgentId, AgentReportKind, ClaudeReport, CliConfig, CodexSpeed,
    CodexStatuslineArgs, CodexStatuslineOption, Command, DailyArgs, SessionArgs, SharedArgs,
    StandardAgent, WeekDay, WeeklyArgs,
};

use super::{
    RootOptions, agent_command_args, command_option, parse_extra_command,
    shared::{parse_claude_report_arg, parse_shared_arg, parse_shared_arg_for_command},
    values::{parse_codex_speed, parse_week_day},
};
use crate::arg_parser::ArgParser;

pub(super) fn parse_agent_command(
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
        StandardAgent::Amp
        | StandardAgent::Droid
        | StandardAgent::Codebuff
        | StandardAgent::Hermes
        | StandardAgent::Goose
        | StandardAgent::Kilo
        | StandardAgent::Copilot
        | StandardAgent::Gemini
        | StandardAgent::Antigravity
        | StandardAgent::Kimi
        | StandardAgent::Qwen
        | StandardAgent::Grok
        | StandardAgent::ZCode
        | StandardAgent::Cline
        | StandardAgent::Dsh => parse_basic_agent_command(parser, shared, agent, config),
    }
}

fn parse_claude_daily_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
    config: &dyn CliConfig,
) -> Result<Command, String> {
    let mut args = DailyArgs {
        shared,
        instances: false,
        project_aliases: None,
    };
    let mut project = None;
    config.apply_daily_args(&mut args);
    while parser.peek().is_some() {
        if parse_claude_report_arg(parser, &mut args.shared, &mut project)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-i" | "--instances" => args.instances = true,
            "--project-aliases" => {
                args.project_aliases = Some(parser.value_for("--project-aliases")?)
            }
            flag => return Err(format!("Unknown daily option '{flag}'")),
        }
    }
    Ok(Command::Claude {
        report: ClaudeReport::Daily(args),
        project,
    })
}

fn parse_claude_monthly_command(
    parser: &mut ArgParser,
    mut shared: SharedArgs,
) -> Result<Command, String> {
    let mut project = None;
    while parser.peek().is_some() {
        if !parse_claude_report_arg(parser, &mut shared, &mut project)? {
            parse_shared_arg(parser, &mut shared)?;
        }
    }
    Ok(Command::Claude {
        report: ClaudeReport::Monthly(shared),
        project,
    })
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
    let mut project = None;
    config.apply_weekly_args(&mut args);
    while parser.peek().is_some() {
        if parse_claude_report_arg(parser, &mut args.shared, &mut project)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-w" | "--start-of-week" => {
                args.start_of_week = parse_week_day(&parser.value_for("--start-of-week")?)?
            }
            flag => return Err(format!("Unknown weekly option '{flag}'")),
        }
    }
    Ok(Command::Claude {
        report: ClaudeReport::Weekly(args),
        project,
    })
}

fn parse_claude_session_command(
    parser: &mut ArgParser,
    shared: SharedArgs,
) -> Result<Command, String> {
    let mut args = SessionArgs { shared, id: None };
    let mut project = None;
    while parser.peek().is_some() {
        if parse_claude_report_arg(parser, &mut args.shared, &mut project)? {
            continue;
        }
        match parser.next_flag()?.as_str() {
            "-i" | "--id" => args.id = Some(parser.value_for("--id")?),
            flag => return Err(format!("Unknown session option '{flag}'")),
        }
    }
    Ok(Command::Claude {
        report: ClaudeReport::Session(args),
        project,
    })
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
        "monthly" => parse_claude_monthly_command(parser, shared),
        "weekly" => parse_claude_weekly_command(parser, shared, config),
        "session" => parse_claude_session_command(parser, shared),
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

pub(super) fn parse_codex_statusline(
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
            "--pi-agent-dir" => {
                options.pi_agent_dir = Some(parser.value_for("--pi-agent-dir")?);
            }
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
