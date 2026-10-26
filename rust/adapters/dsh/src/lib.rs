use ccusage_adapter_common::filter_loaded_entries_by_date;
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

pub use loader::load_entries;
pub use report::summarize_entries;

use crate::{
    Result, cli::AgentCommandArgs, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let kind = args.kind;
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&shared, &pricing)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, kind)?;
    sort_summaries(&mut rows, &shared.order, |row| summary_period(row));
    if wants_json(&shared) {
        return print_json_or_jq(
            report::report_from_rows(&rows, kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "DeepSeek Harness Token Usage Report",
        first_column(kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
