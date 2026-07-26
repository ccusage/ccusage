use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

pub struct Cli {
    pub command: Option<Command>,
    pub shared: SharedArgs,
}

pub enum Command {
    All(AgentCommandArgs),
    Daily(DailyArgs),
    Monthly(SharedArgs),
    Weekly(WeeklyArgs),
    Session(SessionArgs),
    Blocks(BlocksArgs),
    Statusline(StatuslineArgs),
    Codex(AgentCommandArgs),
    OpenCode(AgentCommandArgs),
    Amp(AgentCommandArgs),
    Droid(AgentCommandArgs),
    Codebuff(AgentCommandArgs),
    Hermes(AgentCommandArgs),
    Pi(AgentCommandArgs),
    Goose(AgentCommandArgs),
    Kilo(AgentCommandArgs),
    Copilot(AgentCommandArgs),
    Gemini(AgentCommandArgs),
    Kimi(AgentCommandArgs),
    Qwen(AgentCommandArgs),
    OpenClaw(AgentCommandArgs),
}

#[derive(Clone, Debug, Default)]
pub struct SharedArgs {
    pub since: Option<String>,
    pub until: Option<String>,
    pub json: bool,
    pub mode: CostMode,
    pub debug: bool,
    pub debug_samples: usize,
    pub order: SortOrder,
    pub breakdown: bool,
    pub offline: bool,
    pub no_offline: bool,
    pub color: bool,
    pub no_color: bool,
    pub timezone: Option<String>,
    pub jq: Option<String>,
    pub config: Option<PathBuf>,
    pub compact: bool,
    pub single_thread: bool,
    pub no_cost: bool,
    pub pricing_overrides: BTreeMap<String, PricingOverride>,
    pub pi_stores: Vec<NamedPiStore>,
}

impl SharedArgs {
    pub(crate) fn with_defaults() -> Self {
        Self {
            mode: CostMode::Auto,
            debug_samples: 5,
            order: SortOrder::Asc,
            ..Self::default()
        }
    }
}

pub fn normalize_date_bound(value: &str) -> String {
    value.replace('-', "")
}

#[derive(Clone)]
pub struct DailyArgs {
    pub shared: SharedArgs,
    pub instances: bool,
    pub project: Option<String>,
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
    pub token_limit: Option<String>,
    pub session_length: f64,
}

#[derive(Clone)]
pub struct StatuslineArgs {
    pub offline: bool,
    pub no_offline: bool,
    pub visual_burn_rate: VisualBurnRate,
    pub cost_source: CostSource,
    pub cache: bool,
    pub no_cache: bool,
    pub refresh_interval: u64,
    pub context_low_threshold: u8,
    pub context_medium_threshold: u8,
    pub timezone: Option<String>,
    pub config: Option<PathBuf>,
    pub debug: bool,
    pub model_label_aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub struct AgentCommandArgs {
    pub shared: SharedArgs,
    pub kind: AgentReportKind,
    pub sections: Option<Vec<AgentReportKind>>,
    pub by_agent: bool,
    pub pi_path: Option<String>,
    pub open_claw_path: Option<String>,
    pub codex_speed: CodexSpeed,
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

pub(crate) const STANDARD_AGENT_REPORTS: &[(&str, AgentReportKind)] = &[
    ("daily", AgentReportKind::Daily),
    ("monthly", AgentReportKind::Monthly),
    ("session", AgentReportKind::Session),
];

pub(crate) const OPENCODE_AGENT_REPORTS: &[(&str, AgentReportKind)] = &[
    ("daily", AgentReportKind::Daily),
    ("weekly", AgentReportKind::Weekly),
    ("monthly", AgentReportKind::Monthly),
    ("session", AgentReportKind::Session),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CodexSpeed {
    #[default]
    Auto,
    Standard,
    Fast,
}

impl Default for StatuslineArgs {
    fn default() -> Self {
        Self {
            offline: true,
            no_offline: false,
            visual_burn_rate: VisualBurnRate::Off,
            cost_source: CostSource::Auto,
            cache: true,
            no_cache: false,
            refresh_interval: 1,
            context_low_threshold: 50,
            context_medium_threshold: 80,
            timezone: None,
            config: None,
            debug: false,
            model_label_aliases: HashMap::new(),
        }
    }
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
    pub cache_read_input_token_cost: Option<f64>,
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub max_input_tokens: Option<u64>,
    pub fast_multiplier: Option<f64>,
}

pub trait CliConfig {
    fn config_error(&self) -> Option<&str> {
        None
    }

    fn apply_shared(&self, _shared: &mut SharedArgs) {}

    fn apply_daily_args(&self, _args: &mut DailyArgs) {}

    fn apply_weekly_args(&self, _args: &mut WeeklyArgs) {}

    fn apply_blocks_args(&self, _args: &mut BlocksArgs) {}

    fn apply_statusline_args(&self, _args: &mut StatuslineArgs) {}

    fn apply_agent_args(
        &self,
        _codex_speed: &mut CodexSpeed,
        _pi_path: Option<&mut Option<String>>,
        _open_claw_path: Option<&mut Option<String>>,
    ) {
    }
}

pub struct NoConfig;

impl CliConfig for NoConfig {}
