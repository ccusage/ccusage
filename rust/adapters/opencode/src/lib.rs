use ccusage_adapter_common::{collect_files_with_extension, filter_loaded_entries_by_date};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;
mod source;

pub use loader::{LinkedEntry, load_entries_with_native_sessions};
pub use report::{report_json, summarize_entries};
pub use source::SOURCE;

use crate::{Result, cli::AgentCommandArgs, print_json_or_jq, sort_summaries, wants_json};

/// Reports whether an OpenCode install has usage data, independent of the
/// requested date window.
///
/// The aggregate report lists which agents it found, and the loader narrows to
/// `--since`/`--until` as it reads, so an out-of-range query cannot answer that
/// question from the loaded entries alone.
pub fn has_data() -> bool {
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
    sort_summaries(&mut rows, &shared.order.unwrap_or_default(), |row| {
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
