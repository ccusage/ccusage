use ccusage_adapter_common::{collect_files_with_extension, filter_loaded_entries_by_date};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, UsageSummary, cli, cost, date_at,
    date_range_bounds_ms, debug_log, pricing, print_json_or_jq, progress, sort_summaries,
    stored_cost_for_mode, summary_period, wants_json,
};

mod loader;
mod parser;
mod paths;
mod report;
mod source;

pub use loader::{LinkedEntry, load_entries_with_native_sessions};
pub use report::report_json;
pub(crate) use report::summarize_entries;
pub use source::SOURCE;

use ccusage_core::cli::AgentCommandArgs;

pub(crate) fn has_data() -> bool {
    paths::paths().is_ok_and(|paths| paths.iter().any(|path| loader::has_source(path)))
}

/// Prints the OpenCode report for already loaded entries.
pub fn print_report(args: &AgentCommandArgs, mut entries: Vec<LoadedEntry>) -> Result<()> {
    let kind = args.kind;
    let shared = &args.shared;
    filter_loaded_entries_by_date(&mut entries, shared);
    if wants_json(shared) {
        return print_json_or_jq(
            report_json(
                &entries,
                kind,
                &shared.order.unwrap_or_default(),
                args.start_of_week,
            )?,
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    let mut rows = summarize_entries(&entries, kind, args.start_of_week)?;
    sort_summaries(&mut rows, &shared.order.unwrap_or_default(), &|row| {
        summary_period(row)
    });
    ccusage_adapter_common::report::print_table(
        ccusage_adapter_common::source::EntryDescriptor::standard("opencode", "OpenCode"),
        &rows,
        kind,
        shared,
    )?;
    Ok(())
}
