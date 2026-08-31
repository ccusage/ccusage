use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

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
    Antigravity(AgentCommandArgs),
    Kimi(AgentCommandArgs),
    Qwen(AgentCommandArgs),
    OpenClaw(AgentCommandArgs),
    Grok(AgentCommandArgs),
    ZCode(AgentCommandArgs),
}

#[derive(Clone, Debug, Default)]
pub struct SharedArgs {
    pub since: Option<String>,
    pub until: Option<String>,
    /// Number of most recent report periods to keep, resolved into `since` by
    /// the binary once the report's calendar unit is known.
    pub last: Option<u32>,
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
    pub fn with_defaults() -> Self {
        Self {
            mode: CostMode::Auto,
            debug_samples: 5,
            order: SortOrder::Asc,
            ..Self::default()
        }
    }
}

/// The two documented spellings of a `--since` / `--until` bound.
pub const DATE_BOUND_FORMATS: &str = "YYYY-MM-DD or YYYYMMDD";

/// Normalizes a date bound into the `YYYYMMDD` form the report keys use.
///
/// Reports compare bounds against row keys as plain strings, so a value that is
/// not one of the two documented formats, or that is not a real calendar date,
/// would silently turn the filter into a no-op or drop every row. Returns
/// `None` for those values so callers can reject them.
pub fn normalize_date_bound(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let digits: [u8; 8] = match bytes.len() {
        8 => bytes.try_into().ok()?,
        10 if bytes[4] == b'-' && bytes[7] == b'-' => {
            let mut digits = [0u8; 8];
            digits[..4].copy_from_slice(&bytes[..4]);
            digits[4..6].copy_from_slice(&bytes[5..7]);
            digits[6..].copy_from_slice(&bytes[8..]);
            digits
        }
        _ => return None,
    };
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |slice: &[u8]| {
        slice
            .iter()
            .fold(0u32, |value, digit| value * 10 + u32::from(digit - b'0'))
    };
    let year = number(&digits[..4]) as i32;
    let month = number(&digits[4..6]);
    let day = number(&digits[6..]);
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    std::str::from_utf8(&digits).ok().map(str::to_string)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
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
    pub pricing_overrides: BTreeMap<String, PricingOverride>,
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

pub const STANDARD_AGENT_REPORTS: &[(&str, AgentReportKind)] = &[
    ("daily", AgentReportKind::Daily),
    ("monthly", AgentReportKind::Monthly),
    ("session", AgentReportKind::Session),
];

pub const OPENCODE_AGENT_REPORTS: &[(&str, AgentReportKind)] = &[
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
            pricing_overrides: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::normalize_date_bound;

    #[test]
    fn accepts_both_documented_formats() {
        assert_eq!(
            normalize_date_bound("2026-07-10").as_deref(),
            Some("20260710")
        );
        assert_eq!(
            normalize_date_bound("20260710").as_deref(),
            Some("20260710")
        );
    }

    #[test]
    fn accepts_leap_day_only_in_leap_years() {
        assert_eq!(
            normalize_date_bound("2024-02-29").as_deref(),
            Some("20240229")
        );
        assert_eq!(
            normalize_date_bound("2000-02-29").as_deref(),
            Some("20000229")
        );
        assert_eq!(normalize_date_bound("2026-02-29"), None);
        assert_eq!(normalize_date_bound("1900-02-29"), None);
    }

    #[test]
    fn rejects_impossible_calendar_dates() {
        assert_eq!(normalize_date_bound("2026-02-30"), None);
        assert_eq!(normalize_date_bound("2026-13-01"), None);
        assert_eq!(normalize_date_bound("2026-00-10"), None);
        assert_eq!(normalize_date_bound("2026-07-00"), None);
        assert_eq!(normalize_date_bound("2026-04-31"), None);
    }

    #[test]
    fn rejects_undocumented_spellings() {
        assert_eq!(normalize_date_bound("abc"), None);
        assert_eq!(normalize_date_bound(""), None);
        assert_eq!(normalize_date_bound("2026/07/10"), None);
        assert_eq!(normalize_date_bound("2026-7-10"), None);
        assert_eq!(normalize_date_bound("2026-07-10T00:00:00Z"), None);
        assert_eq!(normalize_date_bound("2026-07-1"), None);
        assert_eq!(normalize_date_bound("2026_07_10"), None);
    }

    #[test]
    fn rejects_non_ascii_values_without_panicking() {
        assert_eq!(normalize_date_bound("２０２６０７１０"), None);
        assert_eq!(normalize_date_bound("2026-07-1０"), None);
    }
}
