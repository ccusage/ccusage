mod last_window;
mod sources;
mod timezone;

use std::{env, ffi::OsString, process};

use ccusage_cli::{Command, DEFAULT_SESSION_DURATION_HOURS, WeekDay};
use ccusage_cli_parser::parse_from_with_config;
use ccusage_config::ConfigContext;

pub(crate) fn parse() -> Command {
    let args = env::args_os().collect::<Vec<_>>();
    let arg_strings = args_to_strings(args.iter().skip(1).cloned()).unwrap_or_else(|message| {
        exit_with_usage(&message);
    });
    let config = ConfigContext::from_args(&arg_strings);
    let mut command = try_parse_with(args.clone(), &config, None)
        .unwrap_or_else(|message| exit_with_usage(&message));
    if let Command::All(unified) = &mut command
        && let Err(message) = sources::resolve(unified, &args, &config)
    {
        exit_with_usage(&message);
    }
    command
}

/// Parses one command line against one view of the config, with the checks
/// and resolutions that need the parsed command.
fn try_parse_with(
    args: Vec<OsString>,
    config: &ConfigContext,
    unified_week_start: Option<WeekDay>,
) -> Result<Command, String> {
    let mut command = parse_from_with_config(
        args,
        config,
        DEFAULT_SESSION_DURATION_HOURS,
        env!("CCUSAGE_VERSION"),
        unified_week_start,
    )?;
    timezone::validate(&command)?;
    last_window::resolve(&mut command)?;
    Ok(command)
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
