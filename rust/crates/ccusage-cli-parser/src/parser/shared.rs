use std::num::NonZeroU32;

use ccusage_cli::{ColorChoice, CostMode, DateBound, SharedArgs, SortOrder, is_shared_flag};

use super::values::{parse_cost_mode, parse_date_bound, parse_last_periods, parse_sort_order};
use crate::arg_parser::ArgParser;

pub(super) fn parse_shared_arg_for_command(
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

pub(super) fn parse_claude_report_arg(
    parser: &mut ArgParser,
    shared: &mut SharedArgs,
    project: &mut Option<String>,
) -> Result<bool, String> {
    if matches!(parser.peek_name(), Some("-p" | "--project")) {
        parser.next_flag()?;
        *project = Some(parser.value_for("--project")?);
        return Ok(true);
    }
    parse_shared_arg_for_command(parser, shared)
}

pub(super) enum SharedFlag {
    Since(DateBound),
    Until(DateBound),
    Last(NonZeroU32),
    Json,
    Mode(CostMode),
    Debug,
    DebugSamples(usize),
    Order(SortOrder),
    Breakdown,
    Offline(bool),
    Color(ColorChoice),
    Timezone,
    Jq(String),
    Config,
    Compact,
    SingleThread,
    NoCost,
}

impl SharedFlag {
    pub(super) fn apply(self, shared: &mut SharedArgs) {
        match self {
            Self::Since(value) => shared.since = Some(value),
            Self::Until(value) => shared.until = Some(value),
            Self::Last(value) => shared.last = Some(value),
            Self::Json => shared.json = true,
            Self::Mode(value) => shared.mode = value,
            Self::Debug => shared.debug = true,
            Self::DebugSamples(value) => shared.debug_samples = value,
            Self::Order(value) => shared.order = Some(value),
            Self::Breakdown => shared.breakdown = true,
            Self::Offline(value) => shared.select_offline(value),
            Self::Color(value) => shared.color = value,
            Self::Jq(value) => shared.jq = Some(value),
            Self::Config | Self::Timezone => {}
            Self::Compact => shared.compact = true,
            Self::SingleThread => shared.single_thread = true,
            Self::NoCost => shared.no_cost = true,
        }
    }
}

pub(super) fn read_shared_flag(parser: &mut ArgParser) -> Result<SharedFlag, String> {
    Ok(match parser.next_flag()?.as_str() {
        "-s" | "--since" => {
            SharedFlag::Since(parse_date_bound("--since", &parser.value_for("--since")?)?)
        }
        "-u" | "--until" => {
            SharedFlag::Until(parse_date_bound("--until", &parser.value_for("--until")?)?)
        }
        "--last" => SharedFlag::Last(parse_last_periods(&parser.value_for("--last")?)?),
        "-j" | "--json" => SharedFlag::Json,
        "-m" | "--mode" => SharedFlag::Mode(parse_cost_mode(&parser.value_for("--mode")?)?),
        "-d" | "--debug" => SharedFlag::Debug,
        "--debug-samples" => SharedFlag::DebugSamples(
            parser
                .value_for("--debug-samples")?
                .parse()
                .map_err(|_| "Invalid value for --debug-samples".to_string())?,
        ),
        "-o" | "--order" => SharedFlag::Order(parse_sort_order(&parser.value_for("--order")?)?),
        "-b" | "--breakdown" => SharedFlag::Breakdown,
        "-O" | "--offline" => SharedFlag::Offline(true),
        "--no-offline" => SharedFlag::Offline(false),
        "--color" => SharedFlag::Color(ColorChoice::Always),
        "--no-color" => SharedFlag::Color(ColorChoice::Never),
        "-z" | "--timezone" => {
            parser.read_timezone()?;
            SharedFlag::Timezone
        }
        "-q" | "--jq" => SharedFlag::Jq(parser.value_for("--jq")?),
        "--config" => {
            parser.value_for("--config")?;
            SharedFlag::Config
        }
        "--compact" => SharedFlag::Compact,
        "--single-thread" => SharedFlag::SingleThread,
        "--no-cost" => SharedFlag::NoCost,
        flag => return Err(format!("Unknown option '{flag}'")),
    })
}

pub(super) fn parse_shared_arg(
    parser: &mut ArgParser,
    shared: &mut SharedArgs,
) -> Result<(), String> {
    read_shared_flag(parser)?.apply(shared);
    Ok(())
}
