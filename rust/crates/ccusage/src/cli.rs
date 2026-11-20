mod last_window;
mod sources;

use std::{env, ffi::OsString, ops::ControlFlow, process::ExitCode};

use ccusage_cli::{AgentCommand, AgentCommandArgs, Command, ReportClock, ReportTimezone, WeekDay};
use ccusage_cli_parser::{parse_cli, parse_report_args_with_config};
use ccusage_config::ConfigContext;

pub(crate) fn parse() -> Result<Option<Command>, ExitCode> {
    let args = env::args_os().collect::<Vec<_>>();
    let arg_strings = args_to_strings(args.iter().skip(1).cloned()).map_err(usage_error)?;
    let config = ConfigContext::from_args(&arg_strings);
    let mut command =
        match parse_cli(args.clone(), &config, env!("CCUSAGE_VERSION")).map_err(usage_error)? {
            ControlFlow::Continue(command) => command,
            ControlFlow::Break(output) => {
                ccusage_core::outln!("{output}").map_err(|_| ExitCode::FAILURE)?;
                return Ok(None);
            }
        };
    let clock = ccusage::report_clock().map_err(|message| {
        ccusage_core::errln!("{message}");
        ExitCode::from(2)
    })?;
    resolve_report_clock(&mut command, clock).map_err(usage_error)?;
    if ccusage_core::log_level() != Some(0) {
        for warning in config.warnings() {
            ccusage_core::errln!("WARN {warning}");
        }
    }
    if let Command::All(unified) = &mut command {
        sources::resolve(unified, &args, &config).map_err(usage_error)?;
    }
    if let Command::Agent(ccusage_cli::StandardAgent::OpenCode, AgentCommand::Report(focused)) =
        &mut command
    {
        sources::resolve_opencode_claims(focused, &args, &config).map_err(usage_error)?;
    }
    Ok(Some(command))
}

fn resolve_report_clock(command: &mut Command, clock: ReportClock) -> Result<(), String> {
    let now = ReportClock::Fixed(clock.now());
    if let Command::Statusline(args) = command {
        args.clock = now;
    } else if let Command::Agent(_, AgentCommand::Statusline(args)) = command
        && args.mcp
    {
        args.shared.clock = clock;
    } else if let Some(shared) = command.shared_mut() {
        shared.clock = now;
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
    let mut report = parse_report_args_with_config(args, config, start_of_week, known_timezone)?;
    report.shared.clock = clock;
    if let Some(period) = report.period() {
        last_window::resolve_shared(&mut report.shared, period)?;
    }
    Ok(report)
}

fn usage_error(message: String) -> ExitCode {
    ccusage_core::errln!("{message}");
    ccusage_core::errln!("Run 'ccusage --help' for usage.");
    ExitCode::from(2)
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
