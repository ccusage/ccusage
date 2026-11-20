//! Argument parsing and `--help` rendering for the ccusage CLI.

pub use parser::{parse_cli, parse_from_with_config, parse_report_args_with_config};

mod arg_parser;
mod help;
mod parser;

#[cfg(test)]
mod tests;
