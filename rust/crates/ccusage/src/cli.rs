use std::{env, ffi::OsString, process};

pub(crate) use ccusage_cli::*;

use crate::{DEFAULT_SESSION_DURATION_HOURS, config::ConfigContext};

pub(crate) fn parse() -> Cli {
    let args = env::args_os().collect::<Vec<_>>();
    let arg_strings = args_to_strings(args.iter().skip(1).cloned()).unwrap_or_else(|message| {
        eprintln!("{message}");
        eprintln!("Run 'ccusage --help' for usage.");
        process::exit(2);
    });
    let config = ConfigContext::from_args(&arg_strings);
    Cli::parse_from_with_config(
        args,
        &config,
        DEFAULT_SESSION_DURATION_HOURS,
        env!("CCUSAGE_VERSION"),
    )
    .unwrap_or_else(|message| {
        eprintln!("{message}");
        eprintln!("Run 'ccusage --help' for usage.");
        process::exit(2);
    })
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
