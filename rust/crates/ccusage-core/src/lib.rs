use crate::cli::{SharedArgs, environment::Environment};

pub mod cost;
pub mod date_utils;
pub mod diagnostics;
mod error;
pub mod fast;
pub mod model_aliases;
pub mod output;
pub mod parallel;
pub mod pricing;
pub mod progress;
pub mod project_names;
pub mod sort;
pub mod summary;
pub mod types;
pub mod window;

#[cfg(test)]
mod test_support;

pub mod cli {
    pub use ccusage_cli::*;
}

pub use cost::{stored_cost_for_mode, sum_costs};
pub use date_utils::*;
pub use diagnostics::{Diagnostics, Skip, SkipKind};
pub use error::{CliError, Context, Result, cli_error};
pub use output::{
    UsageTableOptions, agent_summary_json, attach_unpriced_models, first_column, format_currency,
    format_models_multiline, format_number, json_float, json_value_u64, non_empty_json_string,
    print_json_or_jq, print_missing_pricing_warnings_for_models, print_usage_table,
    print_usage_table_with_options, sanitize_terminal_text, should_use_compact_layout,
    summary_period, totals_json, wants_json,
};
pub use pricing::{Pricing, PricingMap};
pub(crate) use project_names::parse_project_aliases;
pub(crate) use project_names::short_model_name;
pub use project_names::{format_breakdown_model_label, format_project_name};
pub use summary::{SessionAccumulator, sort_summaries, summarize_by_key_target, week_start};
pub use types::*;
pub use window::ReportWindow;

pub use ccusage_terminal::{
    Align, Color, ColumnKind, SimpleTable, TerminalStyle, errln, outln, stdout, stdout_is_terminal,
    terminal_width,
};

pub const DEFAULT_RECENT_DAYS: i64 = 3;
pub const USAGE_COMPACT_WIDTH_THRESHOLD: usize = 100;

pub fn terminal_style(shared: &cli::SharedArgs) -> TerminalStyle {
    TerminalStyle {
        color: shared.color == cli::ColorChoice::Always,
        log_level: log_level(),
        no_color: shared.color == cli::ColorChoice::Never,
    }
}

pub fn color(shared: &cli::SharedArgs, value: impl AsRef<str>, color: Color) -> String {
    ccusage_terminal::color(terminal_style(shared), value, color)
}

pub fn print_box_title(title: &str, shared: &cli::SharedArgs) -> std::io::Result<()> {
    ccusage_terminal::print_box_title(title, terminal_style(shared))
}

pub fn debug_log(shared: &SharedArgs, message: impl AsRef<str>) {
    if shared.debug {
        ccusage_terminal::errln!("{}", message.as_ref());
    }
}

pub fn log_level() -> Option<u8> {
    Environment::current()
        .var("LOG_LEVEL")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
}
