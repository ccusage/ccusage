use crate::{ContextThresholds, DateBound, SessionLength, StandardAgent, TokenLimit};

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    num::NonZeroU32,
};

use jiff::tz::TimeZone;

#[derive(Clone, Debug)]
pub struct ReportTimezone {
    name: Cow<'static, str>,
    zone: TimeZone,
}

impl ReportTimezone {
    pub const UTC: Self = Self {
        name: Cow::Borrowed("UTC"),
        zone: TimeZone::UTC,
    };

    pub fn system_zone() -> TimeZone {
        TimeZone::system()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }
}

impl std::str::FromStr for ReportTimezone {
    type Err = jiff::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "UTC" => Ok(Self::UTC),
            "local" => Ok(Self {
                name: Cow::Borrowed("local"),
                zone: Self::system_zone(),
            }),
            name => Ok(Self {
                name: Cow::Owned(name.to_owned()),
                zone: TimeZone::get(name)?,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum ReportClock {
    #[default]
    System,
    Fixed(jiff::Timestamp),
}

impl ReportClock {
    pub fn now(self) -> jiff::Timestamp {
        match self {
            Self::System => jiff::Timestamp::now(),
            Self::Fixed(now) => now,
        }
    }
}

pub enum Command {
    All(AgentCommandArgs),
    Claude {
        report: ClaudeReport,
        project: Option<String>,
    },
    Statusline(StatuslineArgs),
    Agent(StandardAgent, AgentCommand),
    Telemetry(TelemetryAction),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryAction {
    Status,
    Enable,
    Disable,
}

pub enum ClaudeReport {
    Daily(DailyArgs),
    Monthly(SharedArgs),
    Weekly(WeeklyArgs),
    Session(SessionArgs),
    Blocks(BlocksArgs),
}

pub enum AgentCommand {
    Report(AgentCommandArgs),
    Statusline(CodexStatuslineArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeriodUnit {
    Day,
    Week,
    Month,
}

impl Command {
    pub fn claude_report(report: ClaudeReport) -> Self {
        Self::Claude {
            report,
            project: None,
        }
    }

    pub fn agent_report(agent: StandardAgent, args: AgentCommandArgs) -> Self {
        Self::Agent(agent, AgentCommand::Report(args))
    }

    pub fn shared(&self) -> Option<&SharedArgs> {
        match self {
            Self::All(args) | Self::Agent(_, AgentCommand::Report(args)) => Some(&args.shared),
            Self::Claude { report, .. } => Some(match report {
                ClaudeReport::Daily(args) => &args.shared,
                ClaudeReport::Monthly(shared) => shared,
                ClaudeReport::Weekly(args) => &args.shared,
                ClaudeReport::Session(args) => &args.shared,
                ClaudeReport::Blocks(args) => &args.shared,
            }),
            Self::Agent(_, AgentCommand::Statusline(args)) => Some(&args.shared),
            Self::Statusline(_) | Self::Telemetry(_) => None,
        }
    }

    pub fn shared_mut(&mut self) -> Option<&mut SharedArgs> {
        match self {
            Self::All(args) | Self::Agent(_, AgentCommand::Report(args)) => Some(&mut args.shared),
            Self::Claude { report, .. } => Some(match report {
                ClaudeReport::Daily(args) => &mut args.shared,
                ClaudeReport::Monthly(shared) => shared,
                ClaudeReport::Weekly(args) => &mut args.shared,
                ClaudeReport::Session(args) => &mut args.shared,
                ClaudeReport::Blocks(args) => &mut args.shared,
            }),
            Self::Agent(_, AgentCommand::Statusline(args)) => Some(&mut args.shared),
            Self::Statusline(_) | Self::Telemetry(_) => None,
        }
    }

    pub fn period(&self) -> Option<(PeriodUnit, WeekDay)> {
        match self {
            Self::All(args) | Self::Agent(_, AgentCommand::Report(args)) => args.period(),
            Self::Claude { report, .. } => match report {
                ClaudeReport::Daily(_) => Some((PeriodUnit::Day, WeekDay::Monday)),
                ClaudeReport::Monthly(_) => Some((PeriodUnit::Month, WeekDay::Monday)),
                ClaudeReport::Weekly(args) => Some((PeriodUnit::Week, args.start_of_week)),
                ClaudeReport::Session(_) | ClaudeReport::Blocks(_) => None,
            },
            Self::Statusline(_)
            | Self::Telemetry(_)
            | Self::Agent(_, AgentCommand::Statusline(_)) => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SharedArgs {
    pub clock: ReportClock,
    pub since: Option<DateBound>,
    pub until: Option<DateBound>,
    /// Number of most recent report periods to keep, resolved into `since` and
    /// `until` by the binary once the report's calendar unit is known.
    pub last: Option<NonZeroU32>,
    pub json: bool,
    pub mode: CostMode,
    pub debug: bool,
    pub debug_samples: usize,
    pub order: Option<SortOrder>,
    pub breakdown: bool,
    pub offline: bool,
    pub color: ColorChoice,
    pub timezone: Option<ReportTimezone>,
    pub jq: Option<String>,
    pub compact: bool,
    pub single_thread: bool,
    pub no_cost: bool,
    pub pricing_overrides: BTreeMap<String, PricingOverride>,
    pub pi_stores: Vec<NamedPiStore>,
}

impl SharedArgs {
    pub fn requested_order(&self) -> Option<SortOrder> {
        self.order
    }

    pub fn with_defaults() -> Self {
        Self {
            mode: CostMode::Auto,
            debug_samples: 5,
            ..Self::default()
        }
    }
}

pub fn session_cost_ordering(shared: &SharedArgs) -> SortOrder {
    shared.requested_order().unwrap_or(SortOrder::Desc)
}

/// The two documented spellings of a `--since` / `--until` bound.
pub const DATE_BOUND_FORMATS: &str = "YYYY-MM-DD or YYYYMMDD";

#[derive(Clone)]
pub struct DailyArgs {
    pub shared: SharedArgs,
    pub instances: bool,
    pub project_aliases: Option<String>,
}

#[derive(Clone)]
pub struct WeeklyArgs {
    pub shared: SharedArgs,
    pub start_of_week: WeekDay,
}

#[derive(Clone)]
pub struct SessionArgs {
    pub shared: SharedArgs,
    pub id: Option<String>,
}

#[derive(Clone)]
pub struct BlocksArgs {
    pub shared: SharedArgs,
    pub active: bool,
    pub recent: bool,
    pub token_limit: Option<TokenLimit>,
    pub session_length: SessionLength,
}

#[derive(Clone)]
pub struct StatuslineArgs {
    pub clock: ReportClock,
    pub color: ColorChoice,
    pub mode: CostMode,
    pub offline: bool,
    pub visual_burn_rate: VisualBurnRate,
    pub cost_source: CostSource,
    pub cache: bool,
    pub refresh_interval: u64,
    pub context_thresholds: ContextThresholds,
    pub timezone: Option<ReportTimezone>,
    pub debug: bool,
    pub model_label_aliases: HashMap<String, String>,
    pub pricing_overrides: BTreeMap<String, PricingOverride>,
}

#[derive(Clone)]
pub struct CodexStatuslineArgs {
    pub shared: SharedArgs,
    pub session_id: Option<String>,
    pub codex_speed: CodexSpeed,
    pub mcp: bool,
}

#[derive(Clone)]
pub struct AgentCommandArgs {
    pub shared: SharedArgs,
    pub kind: AgentReportKind,
    pub session_id: Option<String>,
    pub sections: Option<Vec<AgentReportKind>>,
    pub by_agent: bool,
    pub options: AgentOptions,
    // NOTE(2026-11-09, #2022): OpenCode records Claude agent turns that can also appear in Claude logs. Keep this opt-in so totals count matching turns once.
    pub include_claude_agent: bool,
    pub start_of_week: WeekDay,
    /// Per-source options of a unified report, keyed by agent name. A source
    /// without an entry loads with `shared`.
    pub sources: BTreeMap<String, SourceArgs>,
}

impl AgentCommandArgs {
    pub fn with_options(mut self, options: AgentOptions) -> Self {
        self.options = options;
        self
    }

    pub fn period(&self) -> Option<(PeriodUnit, WeekDay)> {
        let unit = match self.kind {
            AgentReportKind::Daily => PeriodUnit::Day,
            AgentReportKind::Weekly => PeriodUnit::Week,
            AgentReportKind::Monthly => PeriodUnit::Month,
            AgentReportKind::Session => return None,
        };
        Some((unit, self.start_of_week))
    }
}

/// The options a unified report loads one source with: the same command line
/// read through that source's config sections, so `ccusage daily` sees an agent
/// the way `ccusage <agent> daily` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentOptions {
    pub claude_project: Option<String>,
    pub codex_speed: CodexSpeed,
    pub pi_path: Option<String>,
    pub pi_agent_dir: Option<String>,
    pub open_claw_path: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SourceArgs {
    pub shared: SharedArgs,
    pub options: AgentOptions,
}

impl SourceArgs {
    pub fn from_options(shared: SharedArgs, options: AgentOptions) -> Self {
        Self { shared, options }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedPiStore {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentReportKind {
    Daily,
    Weekly,
    Monthly,
    Session,
}

impl AgentReportKind {
    pub const ALL: [Self; 4] = [Self::Daily, Self::Weekly, Self::Monthly, Self::Session];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Session => "session",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CodexSpeed {
    #[default]
    Auto,
    Standard,
    Fast,
    Flex,
}

impl Default for StatuslineArgs {
    fn default() -> Self {
        Self {
            clock: ReportClock::default(),
            color: ColorChoice::Auto,
            mode: CostMode::Auto,
            offline: crate::agents::CLAUDE_STATUSLINE.offline_by_default(),
            visual_burn_rate: VisualBurnRate::Off,
            cost_source: CostSource::Auto,
            cache: true,
            refresh_interval: 1,
            context_thresholds: ContextThresholds::default(),
            timezone: None,
            debug: false,
            model_label_aliases: HashMap::new(),
            pricing_overrides: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CostMode {
    #[default]
    Auto,
    Calculate,
    Display,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortOrder {
    Desc,
    #[default]
    Asc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeekDay {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualBurnRate {
    Off,
    Emoji,
    Text,
    EmojiText,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CostSource {
    Auto,
    Ccusage,
    Cc,
    Both,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PricingOverride {
    pub input_cost_per_token: Option<f64>,
    pub output_cost_per_token: Option<f64>,
    pub cache_creation_input_token_cost: Option<f64>,
    pub cache_creation_input_token_cost_above_1hr: Option<f64>,
    pub cache_read_input_token_cost: Option<f64>,
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub max_input_tokens: Option<u64>,
    pub fast_multiplier: Option<f64>,
    pub flex_multiplier: Option<f64>,
}

pub trait CliConfig {
    fn context_thresholds(
        &self,
        low: Option<u8>,
        medium: Option<u8>,
    ) -> Result<ContextThresholds, String> {
        let defaults = ContextThresholds::default();
        ContextThresholds::new(
            low.unwrap_or(defaults.low()),
            medium.unwrap_or(defaults.medium()),
        )
    }

    fn config_error(&self) -> Option<&str> {
        None
    }

    fn timezone(&self) -> Option<&str> {
        None
    }

    fn timezone_config_field(&self) -> Option<String> {
        None
    }

    fn recover_statusline_timezone(
        &self,
        _rejected: &str,
        error: String,
    ) -> Result<Option<ReportTimezone>, String> {
        Err(error)
    }

    fn recover_statusline_window(
        &self,
        _shared: &mut SharedArgs,
        _since_flag: bool,
        _until_flag: bool,
        _message: &str,
    ) -> bool {
        false
    }

    fn apply_shared(&self, _shared: &mut SharedArgs) {}

    fn apply_daily_args(&self, _args: &mut DailyArgs) {}

    fn apply_weekly_args(&self, _args: &mut WeeklyArgs) {}

    fn apply_blocks_args(&self, _args: &mut BlocksArgs) {}

    fn apply_statusline_args(&self, _args: &mut StatuslineArgs) {}

    fn agent_options(&self, _agent: &str) -> AgentOptions {
        AgentOptions::default()
    }
}

pub struct NoConfig;

impl CliConfig for NoConfig {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_projections_keep_the_report_options_and_week_start() {
        let mut command = Command::claude_report(ClaudeReport::Weekly(WeeklyArgs {
            shared: SharedArgs::with_defaults(),
            start_of_week: WeekDay::Sunday,
        }));
        assert_eq!(command.period(), Some((PeriodUnit::Week, WeekDay::Sunday)));
        command.shared_mut().unwrap().offline = true;
        assert!(command.shared().unwrap().offline);
        let statusline = Command::Statusline(StatuslineArgs::default());
        assert!(statusline.shared().is_none());
        assert!(statusline.period().is_none());
    }

    #[test]
    fn accepts_both_documented_formats() {
        assert_eq!(
            DateBound::parse("2026-07-10")
                .map(|date| date.to_string())
                .as_deref(),
            Some("20260710")
        );
        assert_eq!(
            DateBound::parse("20260710")
                .map(|date| date.to_string())
                .as_deref(),
            Some("20260710")
        );
    }

    #[test]
    fn accepts_leap_day_only_in_leap_years() {
        assert_eq!(
            DateBound::parse("2024-02-29")
                .map(|date| date.to_string())
                .as_deref(),
            Some("20240229")
        );
        assert_eq!(
            DateBound::parse("2000-02-29")
                .map(|date| date.to_string())
                .as_deref(),
            Some("20000229")
        );
        assert_eq!(DateBound::parse("2026-02-29"), None);
        assert_eq!(DateBound::parse("1900-02-29"), None);
    }

    #[test]
    fn rejects_impossible_calendar_dates() {
        assert_eq!(DateBound::parse("2026-02-30"), None);
        assert_eq!(DateBound::parse("2026-13-01"), None);
        assert_eq!(DateBound::parse("2026-00-10"), None);
        assert_eq!(DateBound::parse("2026-07-00"), None);
        assert_eq!(DateBound::parse("2026-04-31"), None);
    }

    #[test]
    fn rejects_undocumented_spellings() {
        assert_eq!(DateBound::parse("abc"), None);
        assert_eq!(DateBound::parse(""), None);
        assert_eq!(DateBound::parse("2026/07/10"), None);
        assert_eq!(DateBound::parse("2026-7-10"), None);
        assert_eq!(DateBound::parse("2026-07-10T00:00:00Z"), None);
        assert_eq!(DateBound::parse("2026-07-1"), None);
        assert_eq!(DateBound::parse("2026_07_10"), None);
    }

    #[test]
    fn rejects_non_ascii_values_without_panicking() {
        assert_eq!(DateBound::parse("２０２６０７１０"), None);
        assert_eq!(DateBound::parse("2026-07-1０"), None);
    }

    #[test]
    fn session_cost_order_defaults_to_descending_and_preserves_explicit_orders() {
        assert_eq!(
            session_cost_ordering(&SharedArgs::default()),
            SortOrder::Desc
        );
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let shared = SharedArgs {
                order: Some(order),
                ..SharedArgs::default()
            };
            assert_eq!(session_cost_ordering(&shared), order);
        }
    }
}
