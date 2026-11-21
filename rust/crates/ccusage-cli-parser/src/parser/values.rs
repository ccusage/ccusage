use std::num::NonZeroU32;

use ccusage_cli::{
    AgentCommand, AgentReportKind, CliConfig, CodexSpeed, Command, CostMode, CostSource,
    DATE_BOUND_FORMATS, DateBound, ReportTimezone, SortOrder, VisualBurnRate, WeekDay,
};

use crate::arg_parser::ArgParser;

pub(super) fn parse_date_bound(flag: &str, value: &str) -> Result<DateBound, String> {
    DateBound::parse(value).ok_or_else(|| {
        format!("Invalid value for {flag} '{value}'. Expected {DATE_BOUND_FORMATS}.")
    })
}

pub(super) fn parse_last_periods(value: &str) -> Result<NonZeroU32, String> {
    value.parse().map_err(|_| {
        format!(
            "Invalid value for --last '{value}'. Expected a whole number of periods, 1 or greater."
        )
    })
}

pub(super) fn replace_configured_window_with_last(command: &mut Command) {
    if let Some(shared) = command.shared_mut()
        && shared.last.is_some()
    {
        shared.since = None;
        shared.until = None;
    }
}

pub(super) fn validate_date_window(
    command: &mut Command,
    parser: &ArgParser,
    config: &dyn CliConfig,
) -> Result<(), String> {
    let Some((since, until)) = command
        .shared()
        .and_then(|shared| shared.since.zip(shared.until))
    else {
        return Ok(());
    };
    if since <= until {
        return Ok(());
    }
    let message = format!("The --since date '{since}' is later than the --until date '{until}'.");
    if let Command::Agent(_, AgentCommand::Statusline(args)) = command {
        let since_flag = parser.saw_flag(&["-s", "--since"]);
        let until_flag = parser.saw_flag(&["-u", "--until"]);
        if !(since_flag && until_flag)
            && (config.recover_statusline_window(
                &mut args.shared,
                since_flag,
                until_flag,
                &message,
            ) || args.mcp
                || args.session_id.is_some())
        {
            return Ok(());
        }
    }
    Err(message)
}

pub(super) fn resolve_timezone(
    command: &mut Command,
    parser: &ArgParser,
    config: &dyn CliConfig,
    known_timezone: Option<&ReportTimezone>,
) -> Result<(), String> {
    let statusline = matches!(
        command,
        Command::Statusline(_) | Command::Agent(_, AgentCommand::Statusline(_))
    );
    let timezone = if let Some(value) = parser.timezone.as_deref().or_else(|| config.timezone()) {
        let resolved = known_timezone
            .filter(|timezone| timezone.name() == value)
            .map_or_else(
                || value.parse::<ReportTimezone>(),
                |timezone| Ok(timezone.clone()),
            );
        match resolved {
            Ok(timezone) => Some(timezone),
            Err(_) => {
                let field = if parser.timezone.is_some() {
                    "--timezone".to_owned()
                } else {
                    config.timezone_config_field().map_or_else(
                        || "ccusage config field timezone".to_owned(),
                        |field| format!("ccusage config field {field}"),
                    )
                };
                let error = format!(
                    "Invalid value for {field} '{value}'. Expected an IANA timezone name such as UTC or America/New_York, or 'local' for the system timezone."
                );
                if !statusline || parser.timezone.is_some() {
                    return Err(error);
                }
                config.recover_statusline_timezone(value, error)?
            }
        }
    } else {
        None
    };
    if let Command::Statusline(args) = command {
        args.timezone = timezone;
    } else if let Some(shared) = command.shared_mut() {
        shared.timezone = timezone;
    }
    Ok(())
}

pub(super) fn parse_cost_mode(value: &str) -> Result<CostMode, String> {
    match value {
        "auto" => Ok(CostMode::Auto),
        "calculate" => Ok(CostMode::Calculate),
        "display" => Ok(CostMode::Display),
        _ => Err(format!("Invalid cost mode '{value}'")),
    }
}

pub(super) fn parse_sort_order(value: &str) -> Result<SortOrder, String> {
    match value {
        "asc" => Ok(SortOrder::Asc),
        "desc" => Ok(SortOrder::Desc),
        _ => Err(format!("Invalid sort order '{value}'")),
    }
}

pub(super) fn parse_report_sections(value: &str) -> Result<Vec<AgentReportKind>, String> {
    let mut sections = Vec::new();
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some(kind) = AgentReportKind::from_name(token) else {
            return Err(format!(
                "Invalid --sections value '{token}'. Expected one or more of: daily, weekly, monthly, session."
            ));
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

pub(super) fn parse_week_day(value: &str) -> Result<WeekDay, String> {
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

pub(super) fn parse_codex_speed(value: &str) -> Result<CodexSpeed, String> {
    match value {
        "auto" => Ok(CodexSpeed::Auto),
        "standard" => Ok(CodexSpeed::Standard),
        "fast" => Ok(CodexSpeed::Fast),
        "flex" => Ok(CodexSpeed::Flex),
        _ => Err(format!("Invalid speed option '{value}'")),
    }
}

pub(super) fn parse_visual_burn_rate(value: &str) -> Result<VisualBurnRate, String> {
    match value {
        "off" => Ok(VisualBurnRate::Off),
        "emoji" => Ok(VisualBurnRate::Emoji),
        "text" => Ok(VisualBurnRate::Text),
        "emoji-text" => Ok(VisualBurnRate::EmojiText),
        _ => Err(format!("Invalid visual burn rate '{value}'")),
    }
}

pub(super) fn parse_cost_source(value: &str) -> Result<CostSource, String> {
    match value {
        "auto" => Ok(CostSource::Auto),
        "ccusage" => Ok(CostSource::Ccusage),
        "cc" => Ok(CostSource::Cc),
        "both" => Ok(CostSource::Both),
        _ => Err(format!("Invalid cost source '{value}'")),
    }
}
