mod last_window;
mod sources;
mod timezone;

use std::{collections::BTreeMap, env, ffi::OsString, process};

pub(crate) use ccusage_cli::*;
pub(crate) use ccusage_cli_parser::Cli;

use ccusage_config::ConfigContext;

use crate::DEFAULT_SESSION_DURATION_HOURS;

pub(crate) fn parse() -> Command {
    let args = env::args_os().collect::<Vec<_>>();
    let arg_strings = args_to_strings(args.iter().skip(1).cloned()).unwrap_or_else(|message| {
        exit_with_usage(&message);
    });
    let config = ConfigContext::from_args(&arg_strings);
    let mut command = unified_or_command(parse_with(args.clone(), &config));
    if let Command::All(unified) = &mut command {
        sources::resolve(unified, &args, &config);
    }
    command
}

/// Parses one command line against one view of the config, with the checks
/// and resolutions that need the parsed command.
fn parse_with(args: Vec<OsString>, config: &ConfigContext) -> Cli {
    let mut cli = Cli::parse_from_with_config(
        args,
        config,
        DEFAULT_SESSION_DURATION_HOURS,
        env!("CCUSAGE_VERSION"),
    )
    .unwrap_or_else(|message| exit_with_usage(&message));
    if let Err(message) = timezone::validate(&cli) {
        exit_with_usage(&message);
    }
    if let Err(message) = last_window::resolve(&mut cli) {
        exit_with_usage(&message);
    }
    cli
}

/// A bare `ccusage` is the unified daily report over the root options.
fn unified_or_command(cli: Cli) -> Command {
    let Cli { command, shared } = cli;
    command.unwrap_or_else(|| {
        Command::All(AgentCommandArgs {
            shared,
            kind: AgentReportKind::Daily,
            session_id: None,
            sections: None,
            by_agent: false,
            pi_path: None,
            open_claw_path: None,
            codex_speed: CodexSpeed::Auto,
            start_of_week: WeekDay::Monday,
            sources: BTreeMap::new(),
        })
    })
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
