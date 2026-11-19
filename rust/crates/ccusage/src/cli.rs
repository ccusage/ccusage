mod last_window;
mod sources;

use std::{env, ffi::OsString, process};

use ccusage_cli::{AgentCommand, AgentCommandArgs, Command, ReportClock, ReportTimezone, WeekDay};
use ccusage_cli_parser::{parse_from_with_config, parse_report_args_with_config};
use ccusage_config::ConfigContext;

pub(crate) fn parse() -> Command {
    let args = env::args_os().collect::<Vec<_>>();
    let arg_strings = args_to_strings(args.iter().skip(1).cloned()).unwrap_or_else(|message| {
        exit_with_usage(&message);
    });
    let config = ConfigContext::from_args(&arg_strings);
    let mut command = parse_from_with_config(args.clone(), &config, env!("CCUSAGE_VERSION"))
        .unwrap_or_else(|message| exit_with_usage(&message));
    let clock = ccusage::report_clock().unwrap_or_else(|message| {
        eprintln!("{message}");
        process::exit(2);
    });
    resolve_report_clock(&mut command, clock).unwrap_or_else(|message| exit_with_usage(&message));
    if ccusage_core::log_level() != Some(0) {
        for warning in config.warnings() {
            eprintln!("WARN {warning}");
        }
    }
    if let Command::All(unified) = &mut command
        && let Err(message) = sources::resolve(unified, &args, &config)
    {
        exit_with_usage(&message);
    }
    if let Command::Agent(ccusage_cli::StandardAgent::OpenCode, AgentCommand::Report(focused)) =
        &mut command
        && let Err(message) = sources::resolve_opencode_claims(focused, &args, &config)
    {
        exit_with_usage(&message);
    }
    command
}

fn resolve_report_clock(command: &mut Command, clock: ReportClock) -> Result<(), String> {
    let now = ReportClock::Fixed(clock.now());
    match &mut *command {
        Command::Statusline(args) => args.clock = now,
        Command::Agent(_, AgentCommand::Statusline(args)) if args.mcp => args.shared.clock = clock,
        command => {
            if let Some(shared) = command.shared_mut() {
                shared.clock = now;
            }
        }
    }
    last_window::resolve(command)
}

fn try_parse_report_with(
    args: Vec<OsString>,
    config: &ConfigContext,
    start_of_week: WeekDay,
    known_timezone: Option<&ReportTimezone>,
    clock: ReportClock,
) -> Result<AgentCommandArgs, String> {
    let mut report = parse_report_args_with_config(
        args,
        config,
        env!("CCUSAGE_VERSION"),
        start_of_week,
        known_timezone,
    )?;
    report.shared.clock = clock;
    if let Some(period) = report.period() {
        last_window::resolve_shared(&mut report.shared, period)?;
    }
    Ok(report)
}

fn exit_with_usage(message: &str) -> ! {
    eprintln!("{message}");
    eprintln!("Run 'ccusage --help' for usage.");
    process::exit(2);
}

fn args_to_strings<I>(args: I) -> Result<Vec<String>, String>
where
    I: IntoIterator<Item = OsString>,
{
    args.into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "Arguments must be valid UTF-8".to_string())
        })
        .collect()
}
