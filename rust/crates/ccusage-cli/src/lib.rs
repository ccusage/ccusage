pub mod home;
mod types;

pub use types::{
    AgentCommandArgs, AgentReportKind, BlocksArgs, CliConfig, CodexSpeed, Command, CostMode,
    CostSource, DATE_BOUND_FORMATS, DailyArgs, NamedPiStore, NoConfig, OPENCODE_AGENT_REPORTS,
    PricingOverride, STANDARD_AGENT_REPORTS, SessionArgs, SharedArgs, SortOrder, SourceArgs,
    StatuslineArgs, VisualBurnRate, WeekDay, WeeklyArgs, normalize_date_bound,
};

/// Default length, in hours, of one billing block.
pub const DEFAULT_SESSION_DURATION_HOURS: f64 = 5.0;

/// Agent subcommands that ship in the binary, in report load order.
pub const BUILT_IN_AGENT_NAMES: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "amp",
    "droid",
    "codebuff",
    "hermes",
    "pi",
    "goose",
    "openclaw",
    "kilo",
    "copilot",
    "gemini",
    "antigravity",
    "kimi",
    "qwen",
    "grok",
    "zcode",
];
