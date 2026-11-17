//! Argument parsing and `--help` rendering for the ccusage CLI.
//!
//! Split from `ccusage-cli`, which holds the plain argument types the runtime
//! crates need: the parser embeds the generated help tables through a build
//! script, so keeping it here means editing help text does not invalidate
//! ccusage-core or any adapter.
use ccusage_cli::Command;

pub use parser::parse_from_with_config;

mod arg_parser;
mod help;
mod parser;

pub struct Cli {
    pub command: Option<Command>,
}

#[cfg(test)]
mod help_codegen;

#[cfg(test)]
mod tests;
