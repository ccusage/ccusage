use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

pub use loader::load_entries;
pub use report::{report_json, summarize_entries};

use crate::{
    Result, cli::AgentCommandArgs, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

/// Reports whether an OpenCode install has usage data, independent of the
/// requested date window.
///
/// The aggregate report lists which agents it found, and the loader narrows to
/// `--since`/`--until` as it reads, so an out-of-range query cannot answer that
/// question from the loaded entries alone.
pub fn has_data() -> bool {
    paths::paths().is_ok_and(|paths| paths.iter().any(|path| loader::has_source(path)))
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let mut entries = loader::load_entries(&shared)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    if wants_json(&shared) {
        return print_json_or_jq(
            report_json(&entries, args.kind, &shared.order)?,
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| summary_period(row));
    print_usage_table(
        "OpenCode Token Usage Report",
        first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
