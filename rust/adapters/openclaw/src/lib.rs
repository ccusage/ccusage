use ccusage_adapter_common::{filter_loaded_entries_by_date, read_files_parallel};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result, cli::AgentCommandArgs, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub use loader::load_entries;
pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let pricing = crate::PricingMap::load_with_overrides(
        args.shared.offline,
        crate::log_level() != Some(0),
        args.shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&args.shared, args.open_claw_path.as_deref(), Some(&pricing))?;
    filter_loaded_entries_by_date(&mut entries, &args.shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &args.shared.order, |row| {
        ccusage_core::summary_period(row)
    });
    if wants_json(&args.shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            args.shared.jq.as_deref(),
            args.shared.no_cost,
        );
    }
    print_usage_table(
        "OpenClaw Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &args.shared,
        false,
        None,
    )?;
    Ok(())
}
