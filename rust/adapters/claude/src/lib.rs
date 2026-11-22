use ccusage_adapter_common::{read_discovered_paths, source::UsageLoadContext};
use ccusage_core::{
    Diagnostics, MILLIS_PER_DAY, ModelBreakdown, ReportWindow, Result, Speed, TimestampMs,
    TokenCounts, TokenUsageRaw, UsageSummary, cli, cli_error, cost, date_at, date_range_bounds_ms,
    fast, model_aliases, parse_ts_timestamp, pricing, resolve_tz,
};

mod advisor;
mod blocks;
mod commands;
mod daily;
mod dedupe;
mod diagnostics;
mod load;
mod mirror_claims;
mod paths;
mod source;
mod usage_wire;

pub use commands::{run_report, run_statusline};
pub use load::load_entries;
pub use source::SOURCE;

use advisor::AdvisorUsages;
use load::{
    ClaudeEntry, MainThreadSessions, load_daily_summaries,
    load_daily_summaries_with_detection_progress, load_entries_since,
    load_entries_since_with_diagnostics, load_windowed_entries, main_thread_sessions,
    retain_copied_entries,
};
use usage_wire::{GuardedUsage, IteratedTokenUsage, Nullable, Rejection, non_null, nullable};

pub(crate) use paths::{
    claude_paths, containing_projects_root, extract_session_parts, projects_roots,
    read_usage_files_since, visit_usage_files,
};

#[cfg(test)]
mod tests;
