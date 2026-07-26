use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result, cli::AgentCommandArgs, print_json_or_jq, print_usage_table, sort_summaries,
    wants_json,
};

pub use loader::load_entries;
pub use report::{report_from_rows, summarize_entries};

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&shared, &pricing)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| {
        ccusage_core::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "Kimi Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
