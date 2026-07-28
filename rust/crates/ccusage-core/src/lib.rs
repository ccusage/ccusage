use std::{fmt, io};

pub mod agent_report;
pub mod cost;
pub mod date_utils;
pub mod fast;
pub mod home;
pub mod last_window;
pub mod logger;
pub mod model_aliases;
pub mod output;
pub mod path_utils;
pub mod pricing;
pub mod progress;
pub mod project_names;
pub mod summary;
pub mod types;
pub mod utils;

pub mod cli {
    pub use ccusage_cli::*;
}

pub use agent_report::{agent_summary_json, first_column, summary_period};
pub use cost::{
    calculate_cost, calculate_cost_for_usage, calculate_cost_from_pricing,
    missing_pricing_model_for_candidates, missing_pricing_model_for_token_total,
    missing_pricing_model_for_usage,
};
pub use date_utils::*;
pub use last_window::{PeriodUnit, last_periods_since};
pub use logger::{debug_log, log_level};
pub use output::{
    format_currency, format_models_multiline, format_number, group_project_output, json_float,
    print_json_or_jq, print_missing_pricing_warnings, print_missing_pricing_warnings_for_models,
    print_usage_table, session_summary_json, should_use_compact_layout, summary_json, totals_json,
    wants_json,
};
pub use pricing::{Pricing, PricingMap};
pub(crate) use project_names::parse_project_aliases;
pub use project_names::{format_project_name, short_model_name};
pub use summary::{
    BucketKind, SessionAccumulator, filter_and_sort_summaries, sort_summaries, summarize_by_key,
    summarize_summaries_by_bucket, week_start,
};
pub use types::*;
pub use utils::{
    apply_total_token_fallback, json_value_u64, non_empty_json_string, total_usage_tokens,
};

pub use ccusage_terminal::{Align, Color, SimpleTable, TerminalStyle, terminal_width};

pub const DEFAULT_SESSION_DURATION_HOURS: f64 = 5.0;
pub const DEFAULT_RECENT_DAYS: i64 = 3;
pub const USAGE_COMPACT_WIDTH_THRESHOLD: usize = 100;

pub const BUILT_IN_AGENT_NAMES: &[&str] = &[
    "claude", "codex", "opencode", "amp", "droid", "codebuff", "hermes", "pi", "goose", "openclaw",
    "kilo", "copilot", "gemini", "kimi", "qwen", "rovo",
];

pub type Result<T> = std::result::Result<T, CliError>;

#[derive(Debug)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

pub fn cli_error(message: impl Into<String>) -> CliError {
    CliError(message.into())
}

pub fn terminal_style(shared: &cli::SharedArgs) -> TerminalStyle {
    TerminalStyle {
        color: shared.color,
        log_level: log_level(),
        no_color: shared.no_color,
    }
}

pub fn color(shared: &cli::SharedArgs, value: impl AsRef<str>, color: Color) -> String {
    ccusage_terminal::color(terminal_style(shared), value, color)
}

pub fn print_box_title(title: &str, shared: &cli::SharedArgs) {
    ccusage_terminal::print_box_title(title, terminal_style(shared));
}

pub trait Context<T> {
    fn context(self, message: impl Into<String>) -> Result<T>;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: fmt::Display,
{
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|error| cli_error(format!("{}: {error}", message.into())))
    }
}
