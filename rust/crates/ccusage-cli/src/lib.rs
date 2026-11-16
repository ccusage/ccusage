mod agents;
pub mod home;
mod types;

pub use agents::{AGENTS, AgentId, AgentSpec, BUILT_IN_AGENT_NAMES};
pub use types::{
    AgentCommandArgs, AgentReportKind, BlocksArgs, CliConfig, CodexSpeed, Command, CostMode,
    CostSource, DATE_BOUND_FORMATS, DailyArgs, NamedPiStore, NoConfig, PricingOverride,
    SessionArgs, SharedArgs, SortOrder, SourceArgs, StatuslineArgs, VisualBurnRate, WeekDay,
    WeeklyArgs, normalize_date_bound,
};

/// Default length, in hours, of one billing block.
pub const DEFAULT_SESSION_DURATION_HOURS: f64 = 5.0;
